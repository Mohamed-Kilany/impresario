//! One analysis pass of the workspace: parse (open editor buffers first),
//! validate against the configured devices and convert everything into LSP
//! diagnostics grouped by file (spec section 13).
//!
//! This module is pure and synchronous so it can run on a blocking thread and
//! be unit-tested without a language-server client.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use impresario_core::config;
use impresario_core::diag::{Code, Diagnostic as CoreDiagnostic, Diagnostics, Severity};
use impresario_core::parser::{parse_play, parse_scene};
use impresario_core::settings;
use impresario_core::span::Span;
use impresario_core::validation::{self, DeviceRef};
use impresario_core::workspace::Workspace;
use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

use super::index::Index;
use super::position::{Doc, Encoding};
use super::{is_play_file, is_scene_file};

/// An unsaved editor buffer (the server advertises full sync).
#[derive(Debug, Clone)]
pub struct Overlay {
    pub text: String,
    pub version: i32,
}

/// Everything one analysis pass produces.
pub struct Analysis {
    pub index: Index,
    /// LSP diagnostics per file (absolute paths).
    pub by_file: BTreeMap<PathBuf, Vec<Diagnostic>>,
    /// The exact text every diagnostic was converted against (overlay content
    /// takes precedence over disk).
    pub sources: BTreeMap<PathBuf, String>,
    pub scenes_dir: PathBuf,
    pub plays_dir: PathBuf,
    pub devices: Vec<DeviceRef>,
}

type RawDiags = BTreeMap<PathBuf, Vec<(CoreDiagnostic, Span)>>;

/// Analyse the workspace rooted at `base`.
#[must_use]
pub fn analyze(
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    overlays: &BTreeMap<PathBuf, Overlay>,
    encoding: Encoding,
) -> Analysis {
    let mut raw: RawDiags = BTreeMap::new();
    let mut sources: BTreeMap<PathBuf, String> = BTreeMap::new();

    let cfg_outcome = settings::load_default_layers(config_path, base);
    sources.insert(config_path.to_path_buf(), read_or_empty(config_path));
    absorb(&mut raw, &cfg_outcome.diagnostics, Some(config_path));

    let devices_outcome = config::load_devices(devices_path);
    sources.insert(devices_path.to_path_buf(), read_or_empty(devices_path));
    absorb(&mut raw, &devices_outcome.diagnostics, Some(devices_path));
    let devices: Vec<DeviceRef> = match &devices_outcome.config {
        Some(devices) => devices
            .devices
            .iter()
            .map(|device| DeviceRef::new(device.name.clone(), device.platform))
            .collect(),
        None => Vec::new(),
    };

    let scenes_dir = resolve(
        base,
        cfg_outcome
            .config
            .as_ref()
            .map_or_else(|| PathBuf::from("./scenes"), |cfg| cfg.scenes_dir.clone()),
    );
    let plays_dir = resolve(
        base,
        cfg_outcome
            .config
            .as_ref()
            .map_or_else(|| PathBuf::from("./plays"), |cfg| cfg.plays_dir.clone()),
    );

    let mut files: BTreeSet<PathBuf> = BTreeSet::new();
    collect_files(&scenes_dir, &mut files);
    collect_files(&plays_dir, &mut files);
    for path in overlays.keys() {
        if is_scene_file(path) || is_play_file(path) {
            files.insert(path.clone());
        }
    }

    let mut scenes = Vec::new();
    let mut plays = Vec::new();
    for path in &files {
        let scene = is_scene_file(path);
        if !scene && !is_play_file(path) {
            continue;
        }
        let text = match overlays.get(path) {
            Some(overlay) => overlay.text.clone(),
            None => match fs::read_to_string(path) {
                Ok(text) => text,
                Err(err) => {
                    let diag = CoreDiagnostic::error(
                        Code::CfgInvalidValue,
                        format!("cannot read {}: {err}", path.display()),
                    );
                    raw.entry(path.clone())
                        .or_default()
                        .push((diag, Span::new(path, 1, 1, 0)));
                    continue;
                }
            },
        };
        if scene {
            let outcome = parse_scene(&text, path);
            if let Some(ast) = outcome.ast {
                scenes.push(ast);
            }
            absorb(&mut raw, &outcome.diagnostics, Some(path));
        } else {
            let outcome = parse_play(&text, path);
            if let Some(ast) = outcome.ast {
                plays.push(ast);
            }
            absorb(&mut raw, &outcome.diagnostics, Some(path));
        }
        sources.insert(path.clone(), text);
    }

    let workspace = Workspace { scenes, plays };
    let semantic = validation::validate(&workspace, &devices);
    absorb(&mut raw, &semantic, None);

    let index = Index::build(&workspace.scenes, &workspace.plays);

    let mut by_file: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
    for (file, entries) in raw {
        let text = sources.get(&file).map_or("", String::as_str);
        let doc = Doc::new(text);
        by_file.insert(
            file,
            entries
                .iter()
                .map(|(diag, span)| to_lsp(&doc, encoding, diag, span))
                .collect(),
        );
    }

    Analysis {
        index,
        by_file,
        sources,
        scenes_dir,
        plays_dir,
        devices,
    }
}

/// Attach diagnostics to their file, synthesising a position at the start of
/// `fallback` when a diagnostic carries no span (missing files, for example).
fn absorb(raw: &mut RawDiags, diagnostics: &Diagnostics, fallback: Option<&Path>) {
    for diag in diagnostics.iter() {
        let span = match (&diag.span, fallback) {
            (Some(span), _) => span.clone(),
            (None, Some(path)) => Span::new(path, 1, 1, 0),
            (None, None) => {
                tracing::warn!("lsp: {} has no span, skipped", diag.code.as_str());
                continue;
            }
        };
        raw.entry(span.file.clone())
            .or_default()
            .push((diag.clone(), span));
    }
}

fn to_lsp(doc: &Doc<'_>, encoding: Encoding, diag: &CoreDiagnostic, span: &Span) -> Diagnostic {
    let mut message = diag.message.clone();
    if let Some(help) = &diag.help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }
    Diagnostic {
        range: doc.range(encoding, span),
        severity: Some(match diag.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(diag.code.as_str().to_string())),
        code_description: None,
        source: Some("impresario".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Resolve a workspace-relative path against `base`, dropping `.`
/// components so every file has one canonical, matchable path (the default
/// directories are `./scenes` and `./plays`).
pub(super) fn resolve(base: &Path, path: PathBuf) -> PathBuf {
    let joined = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    let cleaned: PathBuf = joined.components().collect();
    if cleaned.as_os_str().is_empty() {
        base.to_path_buf()
    } else {
        cleaned
    }
}

fn read_or_empty(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn collect_files(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| !is_hidden(path))
        .collect();
    children.sort();
    for child in children {
        let Ok(file_type) = child.symlink_metadata() else {
            continue;
        };
        if file_type.is_dir() {
            collect_files(&child, out);
        } else if file_type.is_file() {
            out.insert(child);
        }
    }
}
