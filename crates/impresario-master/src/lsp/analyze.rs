//! One analysis pass of the workspace: parse (open editor buffers first),
//! validate against the configured devices and convert everything into LSP
//! diagnostics grouped by file (spec section 13).
//!
//! This module is pure and synchronous so it can run on a blocking thread and
//! be unit-tested without a language-server client.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use impresario_core::apps;
use impresario_core::config;
use impresario_core::diag::{Code, Diagnostic as CoreDiagnostic, Diagnostics, Severity};
use impresario_core::parser::{parse_play, parse_scene, parse_theatre};
use impresario_core::settings;
use impresario_core::span::Span;
use impresario_core::validation::{self, DeviceRef};
use impresario_core::workspace::Workspace;
use impresario_language::diagnostics::DiagnosticInfo;
use impresario_language::model::{AppInfo, SourceFile as EditorSourceFile};
use impresario_language::service::Language as EditorLanguage;
use tower_lsp_server::ls_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Uri,
};

use super::position::{Doc, Encoding};
use super::{is_play_file, is_scene_file, is_theatre_file};

/// An unsaved editor buffer (the server advertises full sync).
#[derive(Debug, Clone)]
pub struct Overlay {
    pub text: String,
    pub version: i32,
}

/// Everything one analysis pass produces.
pub struct Analysis {
    /// LSP diagnostics per file (absolute paths).
    pub by_file: BTreeMap<PathBuf, Vec<Diagnostic>>,
    /// The exact text every diagnostic was converted against (overlay content
    /// takes precedence over disk).
    pub sources: BTreeMap<PathBuf, String>,
    pub scenes_dir: PathBuf,
    pub theatres_dir: PathBuf,
    pub plays_dir: PathBuf,
    pub apps_path: PathBuf,
    /// The shared editor service used by every feature handler.
    pub language: EditorLanguage,
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

    let devices_outcome = config::load_devices_merged(devices_path);
    sources.insert(devices_path.to_path_buf(), read_or_empty(devices_path));
    absorb(&mut raw, &devices_outcome.diagnostics, Some(devices_path));

    let apps_path = base.join("impresario.apps.config");
    let apps_outcome = apps::load_apps_merged(&apps_path);
    sources.insert(apps_path.clone(), read_or_empty(&apps_path));
    absorb(&mut raw, &apps_outcome.diagnostics, Some(&apps_path));

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
    let theatres_dir = resolve(
        base,
        cfg_outcome.config.as_ref().map_or_else(
            || PathBuf::from("./theatres"),
            |cfg| cfg.theatres_dir.clone(),
        ),
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
    collect_files(&theatres_dir, &mut files);
    collect_files(&plays_dir, &mut files);
    for path in overlays.keys() {
        if is_scene_file(path) || is_theatre_file(path) || is_play_file(path) {
            files.insert(path.clone());
        }
    }

    let mut scenes = Vec::new();
    let mut theatres = Vec::new();
    let mut plays = Vec::new();
    for path in &files {
        let scene = is_scene_file(path);
        let theatre = is_theatre_file(path);
        if !scene && !theatre && !is_play_file(path) {
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
        } else if theatre {
            let outcome = parse_theatre(&text, path);
            if let Some(ast) = outcome.ast {
                theatres.push(ast);
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

    let workspace = Workspace {
        scenes,
        theatres,
        plays,
    };
    let semantic = validation::validate(&workspace, &devices);
    absorb(&mut raw, &semantic, None);

    if let Some(app_list) = &apps_outcome.config {
        let mut app_diags = Diagnostics::new();
        for play in &workspace.plays {
            let Some(named) = &play.app else {
                continue;
            };
            if app_list.apps.iter().any(|app| app.name == named.name) {
                continue;
            }
            let help = if app_list.apps.is_empty() {
                "add an app definition or run `impresario app add`".to_string()
            } else {
                let names: Vec<&str> = app_list.apps.iter().map(|app| app.name.as_str()).collect();
                format!("available apps: {}", names.join(", "))
            };
            app_diags.push(
                CoreDiagnostic::error(Code::AppUnknown, format!("unknown app '{}'", named.name))
                    .with_span(named.span.clone())
                    .with_help(help),
            );
        }
        absorb(&mut raw, &app_diags, None);
    }

    let app_infos = apps_outcome
        .config
        .as_ref()
        .map(|config| {
            config
                .apps
                .iter()
                .map(|app| AppInfo {
                    name: app.name.clone(),
                    description: app.description.clone(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let apps_authoritative = apps_outcome.config.is_some();
    let editor_sources = sources
        .iter()
        .filter(|(path, _)| is_scene_file(path) || is_theatre_file(path) || is_play_file(path))
        .map(|(path, text)| EditorSourceFile {
            path: path.clone(),
            text: text.clone(),
        })
        .collect::<Vec<_>>();
    let language = if apps_authoritative {
        EditorLanguage::analyze_with_apps(
            &editor_sources,
            &devices,
            &app_infos,
            Some(base.to_path_buf()),
        )
    } else {
        EditorLanguage::analyze(
            &editor_sources,
            &devices,
            &app_infos,
            Some(base.to_path_buf()),
        )
    };

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
    merge_editor_diagnostics(&mut by_file, &language, &sources, encoding);

    Analysis {
        by_file,
        sources,
        scenes_dir,
        theatres_dir,
        plays_dir,
        apps_path,
        language,
    }
}

/// Add the language service's workspace-wide diagnostics (notably duplicate
/// declarations with related information on every occurrence). Parser and
/// validation diagnostics are already present from the analysis pass above,
/// so only the service's extra codes are merged and exact duplicates ignored.
fn merge_editor_diagnostics(
    by_file: &mut BTreeMap<PathBuf, Vec<Diagnostic>>,
    language: &EditorLanguage,
    sources: &BTreeMap<PathBuf, String>,
    encoding: Encoding,
) {
    const EXTRA: &[&str] = &[
        "IMP-DSL-006",
        "IMP-DSL-007",
        "IMP-DSL-016",
        "IMP-DSL-030",
        "IMP-DSL-033",
        "IMP-DSL-040",
        "IMP-DSL-035",
        "IMP-DSL-038",
        "IMP-DSL-048",
    ];
    for (path, diagnostics) in &language.diagnostics {
        for info in diagnostics {
            if !EXTRA.contains(&info.code.as_str()) {
                continue;
            }
            let doc = Doc::new(sources.get(path).map_or("", String::as_str));
            let diagnostic = info_to_lsp(&doc, encoding, info, sources);
            let target = by_file.entry(path.clone()).or_default();
            if target.iter().any(|existing| {
                existing.code == diagnostic.code && existing.range == diagnostic.range
            }) {
                continue;
            }
            target.push(diagnostic);
        }
    }
    for diagnostics in by_file.values_mut() {
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.range.start.line,
                diagnostic.range.start.character,
            )
        });
    }
}

fn info_to_lsp(
    doc: &Doc<'_>,
    encoding: Encoding,
    info: &DiagnosticInfo,
    sources: &BTreeMap<PathBuf, String>,
) -> Diagnostic {
    Diagnostic {
        range: doc.range(encoding, &info.span),
        severity: Some(if info.severity == "warning" {
            DiagnosticSeverity::WARNING
        } else {
            DiagnosticSeverity::ERROR
        }),
        code: Some(NumberOrString::String(info.code.clone())),
        code_description: None,
        source: Some("impresario".to_string()),
        message: info.message.clone(),
        related_information: if info.related.is_empty() {
            None
        } else {
            Some(
                info.related
                    .iter()
                    .filter_map(|related| {
                        let uri = Uri::from_file_path(&related.span.file)?;
                        let related_doc =
                            Doc::new(sources.get(&related.span.file).map_or("", String::as_str));
                        Some(DiagnosticRelatedInformation {
                            location: Location {
                                uri,
                                range: related_doc.range(encoding, &related.span),
                            },
                            message: related.message.clone(),
                        })
                    })
                    .collect(),
            )
        },
        tags: None,
        data: None,
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
