//! Diagnostics shared by the LSP and the standalone extension.
//!
//! Parser and validation diagnostics come straight from `impresario-core`.
//! This module adds the workspace-wide duplicate rule with related
//! information on *every* occurrence, which the parser cannot do because it
//! sees one file at a time (and drops later duplicates from the AST).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use impresario_core::ast::{PlayFile, SceneFile, TheatreFile};
use impresario_core::diag::{Code, Diagnostic as CoreDiagnostic, Diagnostics, Severity};
use impresario_core::parser::{parse_play, parse_scene, parse_theatre};
use impresario_core::scanner;
use impresario_core::span::Span;
use impresario_core::validation::{self, DeviceRef};
use impresario_core::workspace::Workspace;
use serde::{Deserialize, Serialize};

use crate::model::SourceFile;

/// One diagnostic in editor-neutral form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticInfo {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub help: Option<String>,
    pub span: Span,
    pub related: Vec<RelatedInfo>,
}

/// A related location shown by the editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedInfo {
    pub span: Span,
    pub message: String,
}

/// Add unknown-`App:` diagnostics when an apps file is authoritative.
///
/// `apps` is the parsed `impresario.apps.config`; an empty list still means
/// every `App:` name is unknown, exactly like the LSP analysis pass.
pub fn add_app_diagnostics(
    workspace: &Workspace,
    apps: &[crate::model::AppInfo],
    by_file: &mut BTreeMap<PathBuf, Vec<DiagnosticInfo>>,
) {
    let names: Vec<&str> = apps.iter().map(|app| app.name.as_str()).collect();
    for play in &workspace.plays {
        let Some(named) = &play.app else {
            continue;
        };
        if names.contains(&named.name.as_str()) {
            continue;
        }
        let help = if names.is_empty() {
            "add an app definition or run `impresario app add`".to_string()
        } else {
            format!("available apps: {}", names.join(", "))
        };
        let diagnostic =
            CoreDiagnostic::error(Code::AppUnknown, format!("unknown app '{}'", named.name))
                .with_span(named.span.clone())
                .with_help(help);
        push_core(by_file, &diagnostic, &named.span.file);
    }
    dedupe(by_file);
}

/// Parse and validate every source file, then add duplicate diagnostics.
#[must_use]
pub fn analyze_sources(
    sources: &[SourceFile],
    devices: &[DeviceRef],
) -> (
    Workspace,
    Diagnostics,
    BTreeMap<PathBuf, Vec<DiagnosticInfo>>,
) {
    let mut workspace = Workspace::default();
    let mut all = Diagnostics::new();
    let mut by_file: BTreeMap<PathBuf, Vec<DiagnosticInfo>> = BTreeMap::new();
    for source in sources {
        let path = source.path.as_path();
        let (ast, diagnostics) = match extension(path) {
            "scene" => {
                let outcome = parse_scene(&source.text, path);
                (outcome.ast.map(WorkspacePart::Scene), outcome.diagnostics)
            }
            "theatre" => {
                let outcome = parse_theatre(&source.text, path);
                (outcome.ast.map(WorkspacePart::Theatre), outcome.diagnostics)
            }
            "play" => {
                let outcome = parse_play(&source.text, path);
                (outcome.ast.map(WorkspacePart::Play), outcome.diagnostics)
            }
            _ => (None, Diagnostics::new()),
        };
        if let Some(part) = ast {
            match part {
                WorkspacePart::Scene(scene) => workspace.scenes.push(scene),
                WorkspacePart::Theatre(theatre) => workspace.theatres.push(theatre),
                WorkspacePart::Play(play) => workspace.plays.push(play),
            }
        }
        for diagnostic in &diagnostics {
            push_core(&mut by_file, diagnostic, path);
        }
        all.extend(diagnostics);
    }
    let semantic = validation::validate(&workspace, devices);
    for diagnostic in &semantic {
        if let Some(span) = &diagnostic.span {
            push_core(&mut by_file, diagnostic, span.file());
        } else {
            push_core(&mut by_file, diagnostic, Path::new(""));
        }
    }
    all.extend(semantic.clone());
    for diagnostic in impresario_core::workspace::stage_implementation_diagnostics(&workspace) {
        if let Some(span) = &diagnostic.span {
            push_core(&mut by_file, &diagnostic, span.file());
        } else {
            push_core(&mut by_file, &diagnostic, Path::new(""));
        }
        all.push(diagnostic);
    }
    add_duplicates(&workspace, sources, &mut by_file);
    dedupe(&mut by_file);
    (workspace, all, by_file)
}

enum WorkspacePart {
    Scene(SceneFile),
    Theatre(TheatreFile),
    Play(PlayFile),
}

fn extension(path: &Path) -> &str {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
}

fn push_core(
    by_file: &mut BTreeMap<PathBuf, Vec<DiagnosticInfo>>,
    diagnostic: &CoreDiagnostic,
    fallback: &Path,
) {
    let span = diagnostic
        .span
        .clone()
        .unwrap_or_else(|| Span::new(fallback, 1, 1, 0));
    let info = DiagnosticInfo {
        code: diagnostic.code.as_str().to_string(),
        severity: match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
        .to_string(),
        message: diagnostic.message.clone(),
        help: diagnostic.help.clone(),
        span,
        related: Vec::new(),
    };
    by_file
        .entry(info.span.file.clone())
        .or_default()
        .push(info);
}

fn dedupe(by_file: &mut BTreeMap<PathBuf, Vec<DiagnosticInfo>>) {
    for diagnostics in by_file.values_mut() {
        let mut deduped: Vec<DiagnosticInfo> = Vec::new();
        for diagnostic in diagnostics.drain(..) {
            if let Some(existing) = deduped.iter_mut().find(|existing| {
                existing.code == diagnostic.code
                    && existing.span == diagnostic.span
                    && existing.message == diagnostic.message
            }) {
                if diagnostic.related.len() > existing.related.len() {
                    *existing = diagnostic;
                }
            } else {
                deduped.push(diagnostic);
            }
        }
        deduped.sort_by(|left, right| {
            (left.span.line, left.span.col, &left.code).cmp(&(
                right.span.line,
                right.span.col,
                &right.code,
            ))
        });
        *diagnostics = deduped;
    }
}

/// A name that must be unique within one scope.
#[derive(Debug, Clone)]
struct Name {
    name: String,
    span: Span,
    code: Code,
    scope: String,
    severity: Severity,
    help: String,
}

fn add_duplicates(
    workspace: &Workspace,
    sources: &[SourceFile],
    by_file: &mut BTreeMap<PathBuf, Vec<DiagnosticInfo>>,
) {
    let source_map: BTreeMap<&Path, &str> = sources
        .iter()
        .map(|source| (source.path.as_path(), source.text.as_str()))
        .collect();
    let mut names: Vec<Name> = Vec::new();

    for scene in &workspace.scenes {
        names.push(Name {
            name: scene.name.clone(),
            span: scene.name_span.clone(),
            code: Code::DslDuplicateScene,
            scope: "workspace scenes".to_string(),
            severity: Severity::Error,
            help: "scene names must be unique across the workspace".to_string(),
        });
        for piece in &scene.pieces {
            names.push(Name {
                name: piece.name.clone(),
                span: piece.name_span.clone(),
                code: Code::DslDuplicatePiece,
                scope: format!("scene:{} pieces", scene.name),
                severity: Severity::Error,
                help: "piece names must be unique within one scene".to_string(),
            });
        }
    }
    for theatre in &workspace.theatres {
        names.push(Name {
            name: theatre.name.clone(),
            span: theatre.name_span.clone(),
            code: Code::DslDuplicateTheatre,
            scope: "workspace theatres".to_string(),
            severity: Severity::Error,
            help: "theatre names must be unique across the workspace".to_string(),
        });
        for stage in &theatre.stages {
            // `(stage name, audience platform)` uniqueness is validated by the
            // core workspace check; a duplicate name alone is legal when the
            // audiences are disjoint.
            push_params(&mut names, &stage.params, &stage.name, "stage");
        }
    }
    for play in &workspace.plays {
        for var in &play.var_decls {
            names.push(Name {
                name: var.name.clone(),
                span: var.name_span.clone(),
                code: Code::DslLintShadowedVar,
                scope: format!("play:{} vars", play.path.display()),
                severity: Severity::Warning,
                help: "the last declaration wins; rename or remove one".to_string(),
            });
        }
        for act in &play.acts {
            names.push(Name {
                name: act.name.clone(),
                span: act.name_span.clone(),
                code: Code::DslDuplicateActStage,
                scope: format!("play:{} acts", play.path.display()),
                severity: Severity::Error,
                help: "act names must be unique within one play".to_string(),
            });
            push_params(&mut names, &act.params, &act.name, "act");
            if let Some(examples) = &act.examples {
                for column in &examples.columns {
                    names.push(Name {
                        name: column.name.clone(),
                        span: column.span.clone(),
                        code: Code::DslDuplicateExampleColumn,
                        scope: format!("act:{} examples", act.name),
                        severity: Severity::Error,
                        help: "example columns must be unique within one Examples table"
                            .to_string(),
                    });
                }
            }
        }
    }

    // The AST deliberately drops duplicate headers while parsing. Scan the
    // raw lines too so every occurrence gets a diagnostic and related link.
    for source in sources {
        let Some(text) = source_map.get(source.path.as_path()) else {
            continue;
        };
        scan_duplicate_headers(source, text, &mut names);
    }

    // The AST and the raw-line scan both report the same declarations. A
    // declaration can only occur once per physical line, so de-duplicate on
    // (scope, name, file, line) before grouping; otherwise every valid name
    // would look duplicated.
    names.sort_by(|left, right| {
        (
            left.scope.clone(),
            left.name.clone(),
            left.span.file.clone(),
            left.span.line,
        )
            .cmp(&(
                right.scope.clone(),
                right.name.clone(),
                right.span.file.clone(),
                right.span.line,
            ))
    });
    names.dedup_by(|left, right| {
        left.scope == right.scope
            && left.name == right.name
            && left.span.file == right.span.file
            && left.span.line == right.span.line
    });

    let mut groups: BTreeMap<(String, String), Vec<Name>> = BTreeMap::new();
    for name in names {
        if name.name.is_empty() {
            continue;
        }
        groups
            .entry((name.scope.clone(), name.name.clone()))
            .or_default()
            .push(name);
    }
    for entries in groups.values() {
        if entries.len() < 2 {
            continue;
        }
        let locations = entries
            .iter()
            .map(|entry| RelatedInfo {
                span: entry.span.clone(),
                message: format!("'{}' also declared here", entry.name),
            })
            .collect::<Vec<_>>();
        for entry in entries {
            let index = by_file.entry(entry.span.file.clone()).or_default();
            if index.iter().any(|diagnostic| {
                diagnostic.code == entry.code.as_str()
                    && diagnostic.span == entry.span
                    && !diagnostic.related.is_empty()
            }) {
                continue;
            }
            let related = locations
                .iter()
                .filter(|related| related.span != entry.span)
                .cloned()
                .collect::<Vec<_>>();
            index.push(DiagnosticInfo {
                code: entry.code.as_str().to_string(),
                severity: match entry.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                }
                .to_string(),
                message: format!(
                    "duplicate {} '{}' ({} occurrences)",
                    entry.scope,
                    entry.name,
                    entries.len()
                ),
                help: Some(entry.help.clone()),
                span: entry.span.clone(),
                related,
            });
        }
    }
}

fn push_params(
    names: &mut Vec<Name>,
    params: &[impresario_core::ast::ParamDecl],
    owner: &str,
    kind: &str,
) {
    for param in params {
        names.push(Name {
            name: param.name.clone(),
            span: param.name_span.clone(),
            code: Code::DslDuplicateParameter,
            scope: format!("{kind}:{owner} parameters"),
            severity: Severity::Error,
            help: "parameters must be unique in one signature".to_string(),
        });
    }
}

/// Duplicate headers the AST cannot retain, plus table columns and rows.
fn scan_duplicate_headers(source: &SourceFile, text: &str, names: &mut Vec<Name>) {
    let path = source.path.as_path();
    let scanned = scanner::scan(text, path);
    let mut current_act: Option<String> = None;
    let mut in_examples = false;
    let mut examples_seen_header = false;
    let mut in_table = false;
    let mut table_header_seen = false;
    let mut table_separator_seen = false;
    let mut scene_name: Option<String> = None;
    for (index, line) in scanned.lines.iter().enumerate() {
        let content = line.content.as_str();
        match extension(path) {
            "scene" => {
                if line.depth == 0 {
                    if let Some(name) = content.strip_prefix("Scene:") {
                        let name = name.trim();
                        if !name.is_empty() {
                            let span = line.span_of(path, name).unwrap_or_else(|| {
                                Span::new(path, line.line, line.col + 6, name.chars().count())
                            });
                            add_header(
                                name,
                                span,
                                Code::DslDuplicateScene,
                                "workspace scenes",
                                names,
                            );
                            scene_name = Some(name.to_string());
                        }
                    }
                    if content.starts_with('|') {
                        let cells = cells(line, path);
                        if !in_table {
                            in_table = true;
                            table_header_seen = true;
                            for (cell_index, (cell_text, span)) in cells.iter().enumerate() {
                                if cell_index > 0 {
                                    let platform = cell_text.trim();
                                    if !platform.is_empty() && platform != "Piece" {
                                        names.push(Name {
                                            name: platform.to_string(),
                                            span: span.clone(),
                                            code: Code::DslDuplicatePiece,
                                            scope: format!(
                                                "scene:{} columns",
                                                scene_name.as_deref().unwrap_or("*")
                                            ),
                                            severity: Severity::Error,
                                            help: "a scene column may appear only once".to_string(),
                                        });
                                    }
                                }
                            }
                        } else if table_header_seen && !table_separator_seen {
                            table_separator_seen = true;
                        } else if let Some((name, span)) = cells.first() {
                            let name = name.trim();
                            if !name.is_empty() && name != "-" {
                                names.push(Name {
                                    name: name.to_string(),
                                    span: span.clone(),
                                    code: Code::DslDuplicatePiece,
                                    scope: format!(
                                        "scene:{} pieces",
                                        scene_name.as_deref().unwrap_or("*")
                                    ),
                                    severity: Severity::Error,
                                    help: "piece names must be unique within one scene".to_string(),
                                });
                            }
                        }
                    }
                    continue;
                }
                if line.depth == 1 && in_pieces_section(&scanned.lines, index) {
                    if let Some((name, span)) = legacy_piece(line, path) {
                        names.push(Name {
                            name: name.to_string(),
                            span,
                            code: Code::DslDuplicatePiece,
                            scope: format!("scene:{} pieces", scene_name.as_deref().unwrap_or("*")),
                            severity: Severity::Error,
                            help: "piece names must be unique within one scene".to_string(),
                        });
                    }
                }
            }
            "theatre" => {
                if line.depth != 0 {
                    continue;
                }
                if let Some(name) = content.strip_prefix("Theatre:") {
                    let name = name.trim();
                    if !name.is_empty() {
                        let span = line.span_of(path, name).unwrap_or_else(|| {
                            Span::new(path, line.line, line.col + 8, name.chars().count())
                        });
                        add_header(
                            name,
                            span,
                            Code::DslDuplicateTheatre,
                            "workspace theatres",
                            names,
                        );
                    }
                } else if let Some(name) = content.strip_prefix("Stage:") {
                    let name = name.trim();
                    let header = stage_header_name(name);
                    if !header.is_empty() {
                        // Audience-aware stage uniqueness is checked by
                        // `workspace::stage_implementation_diagnostics`.
                        scan_params(line, header, "stage", path, names);
                    }
                }
            }
            "play" => {
                if line.depth == 0 {
                    in_examples = false;
                    examples_seen_header = false;
                    if let Some(name) = content.strip_prefix("Act:") {
                        let name = name.trim();
                        let header = stage_header_name(name);
                        if !header.is_empty() {
                            let span = line.span_of(path, header).unwrap_or_else(|| {
                                Span::new(path, line.line, line.col + 4, header.chars().count())
                            });
                            add_header(
                                header,
                                span,
                                Code::DslDuplicateActStage,
                                &format!("play:{} acts", path.display()),
                                names,
                            );
                            current_act = Some(header.to_string());
                            scan_params(line, header, "act", path, names);
                        }
                    } else if let Some(name) = content.strip_prefix("Var:") {
                        let name = name.split('=').next().unwrap_or("").trim();
                        if !name.is_empty() {
                            let span = line.span_of(path, name).unwrap_or_else(|| {
                                Span::new(path, line.line, line.col + 4, name.chars().count())
                            });
                            names.push(Name {
                                name: name.to_string(),
                                span,
                                code: Code::DslLintShadowedVar,
                                scope: format!("play:{} vars", path.display()),
                                severity: Severity::Warning,
                                help: "the last declaration wins; rename or remove one".to_string(),
                            });
                        }
                    } else if content.starts_with("Stage:") {
                        let name = content.trim_start_matches("Stage:").trim();
                        let header = stage_header_name(name);
                        if !header.is_empty() {
                            let scope = current_act.as_deref().map_or_else(
                                || format!("play:{}", path.display()),
                                |act| format!("act:{act} stages"),
                            );
                            let span = line.span_of(path, header).unwrap_or_else(|| {
                                Span::new(path, line.line, line.col + 6, header.chars().count())
                            });
                            names.push(Name {
                                name: header.to_string(),
                                span,
                                code: Code::DslDuplicateActStage,
                                scope,
                                severity: Severity::Error,
                                help: "stage names must be unique within one Act".to_string(),
                            });
                        }
                    }
                } else if line.depth == 1 && content.starts_with("Examples:") {
                    in_examples = true;
                    examples_seen_header = false;
                } else if in_examples && line.depth == 2 && content.starts_with('|') {
                    let cells = cells(line, path);
                    if !examples_seen_header {
                        examples_seen_header = true;
                        for (name, span) in &cells {
                            let name = name.trim();
                            if name.is_empty() || name.chars().all(|ch| ch == '-' || ch == ':') {
                                continue;
                            }
                            names.push(Name {
                                name: name.to_string(),
                                span: span.clone(),
                                code: Code::DslDuplicateExampleColumn,
                                scope: format!(
                                    "act:{} examples",
                                    current_act.as_deref().unwrap_or("*")
                                ),
                                severity: Severity::Error,
                                help: "example columns must be unique within one Examples table"
                                    .to_string(),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn in_pieces_section(lines: &[scanner::LogicalLine], index: usize) -> bool {
    for line in lines.iter().take(index).rev() {
        if line.depth == 0 {
            return line.content.starts_with("Pieces:");
        }
    }
    false
}

/// The identifier of an `Act:`/`Stage:` header, without its parameter list.
fn stage_header_name(header: &str) -> &str {
    header.split('(').next().unwrap_or(header).trim()
}

fn add_header(name: &str, span: Span, code: Code, scope: &str, names: &mut Vec<Name>) {
    names.push(Name {
        name: name.to_string(),
        span,
        code,
        scope: scope.to_string(),
        severity: Severity::Error,
        help: "this name must be unique in its scope".to_string(),
    });
}

fn legacy_piece<'a>(line: &'a scanner::LogicalLine, path: &Path) -> Option<(&'a str, Span)> {
    let colon = line.content.find(':')?;
    let name = line.content.get(..colon)?.trim();
    if name.is_empty() {
        return None;
    }
    let offset = line.content.get(..colon)?.find(name).unwrap_or(0);
    let char_offset = line.content.get(..offset)?.chars().count();
    Some((
        name,
        Span::new(
            path,
            line.line,
            line.col + char_offset,
            name.chars().count(),
        ),
    ))
}

fn cells(line: &scanner::LogicalLine, path: &Path) -> Vec<(String, Span)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = line.content.as_bytes();
    let mut pipe_positions = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'|' {
            pipe_positions.push(index);
        }
    }
    if pipe_positions.len() < 2 {
        return out;
    }
    for pair in pipe_positions.windows(2) {
        let Some((left, right)) = pair.first().zip(pair.get(1)) else {
            continue;
        };
        let raw = line.content.get(left + 1..*right).unwrap_or("");
        let trimmed = raw.trim();
        let trim_start = raw.find(trimmed).unwrap_or(0);
        let char_offset = line
            .content
            .get(..left + 1)
            .map(|prefix| prefix.chars().count())
            .unwrap_or(0)
            + raw
                .get(..trim_start)
                .map(|prefix| prefix.chars().count())
                .unwrap_or(0);
        out.push((
            trimmed.to_string(),
            Span::new(
                path,
                line.line,
                line.col + char_offset,
                trimmed.chars().count(),
            ),
        ));
        start = right + 1;
    }
    let _ = start;
    out
}

fn scan_params(
    line: &scanner::LogicalLine,
    header: &str,
    kind: &str,
    path: &Path,
    names: &mut Vec<Name>,
) {
    let Some(open) = line.content.find('(') else {
        return;
    };
    let Some(close) = line.content.rfind(')') else {
        return;
    };
    if close <= open {
        return;
    }
    let inner = line.content.get(open + 1..close).unwrap_or("");
    let mut offset = 0usize;
    for part in inner.split(',') {
        let trimmed = part.trim();
        let name = trimmed.split('=').next().unwrap_or("").trim();
        if !name.is_empty() {
            let extra = part.find(trimmed).unwrap_or(0) + trimmed.find(name).unwrap_or(0);
            let char_offset = line
                .content
                .get(..open + 1)
                .map(|prefix| prefix.chars().count())
                .unwrap_or(0)
                + inner
                    .get(..offset + extra)
                    .map(|prefix| prefix.chars().count())
                    .unwrap_or(0);
            names.push(Name {
                name: name.to_string(),
                span: Span::new(
                    path,
                    line.line,
                    line.col + char_offset,
                    name.chars().count(),
                ),
                code: Code::DslDuplicateParameter,
                scope: format!("{kind}:{header} parameters"),
                severity: Severity::Error,
                help: "parameters must be unique in one signature".to_string(),
            });
        }
        offset += part.len() + 1;
    }
}
