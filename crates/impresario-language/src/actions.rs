//! Quick fixes and refactorings (TASK-2 section 9.1).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use impresario_core::diag::suggest;
use impresario_core::span::Span;

use crate::diagnostics::DiagnosticInfo;
use crate::service::{CodeAction, CreateFile, Language, TextEdit, WorkspaceEdit};
use crate::text::{Doc, Position};

impl Language {
    /// Quick fixes and refactorings that apply to a selection.
    #[must_use]
    pub fn code_actions(&self, path: &Path, start: Position, end: Position) -> Vec<CodeAction> {
        let Some(text) = self.source(path) else {
            return Vec::new();
        };
        let doc = Doc::new(text);
        let from = start.line.min(end.line).max(1);
        let to = start.line.max(end.line).max(1);
        let mut actions = Vec::new();
        let mut seen = BTreeSet::new();
        for diagnostic in self.diagnostics_for(path) {
            let line = u32::try_from(diagnostic.span.line).unwrap_or(u32::MAX);
            if line < from || line > to {
                continue;
            }
            let key = (
                diagnostic.code.clone(),
                diagnostic.span.file.display().to_string(),
                diagnostic.span.line,
                diagnostic.span.col,
                diagnostic.span.len,
            );
            if !seen.insert(key) {
                continue;
            }
            self.actions_for_diagnostic(path, &doc, diagnostic, &mut actions);
        }
        if let Some(action) = self.extract_action(path, &doc, from, to) {
            actions.push(action);
        }
        actions
    }

    fn actions_for_diagnostic(
        &self,
        path: &Path,
        doc: &Doc<'_>,
        diagnostic: &DiagnosticInfo,
        actions: &mut Vec<CodeAction>,
    ) {
        let line = u32::try_from(diagnostic.span.line).unwrap_or(u32::MAX);
        if let Some(suggestion) = did_you_mean(diagnostic) {
            let mut edit = WorkspaceEdit::default();
            edit.add(TextEdit {
                span: diagnostic.span.clone(),
                new_text: suggestion,
            });
            actions.push(CodeAction {
                title: "Use the suggested spelling".to_string(),
                kind: "quickfix".to_string(),
                diagnostics: vec![diagnostic.code.clone()],
                edit: Some(edit),
                command: None,
            });
        }
        match diagnostic.code.as_str() {
            "IMP-DSL-008" => {
                if let Some(name) = quoted_name(&diagnostic.message) {
                    let directory = self.root_for(path).join("scenes");
                    let file = directory.join(format!("{name}.scene"));
                    actions.push(CodeAction {
                        title: format!("Create scene '{name}'"),
                        kind: "quickfix".to_string(),
                        diagnostics: vec![diagnostic.code.clone()],
                        edit: Some(WorkspaceEdit {
                            create_files: vec![CreateFile {
                                path: file.display().to_string(),
                                text: format!(
                                    "Scene: {name}\nPlatform: cross-platform\n\nPieces:\n"
                                ),
                                overwrite: false,
                            }],
                            ..WorkspaceEdit::default()
                        }),
                        command: None,
                    });
                }
            }
            "IMP-DSL-031" => {
                if let Some(name) = quoted_name(&diagnostic.message) {
                    let (theatre, stage) = name.split_once('.').map_or(
                        ("Workspace".to_string(), name.as_str()),
                        |(theatre, stage)| (theatre.to_string(), stage),
                    );
                    let directory = self.root_for(path).join("theatres");
                    let file = directory.join(format!("{stage}.theatre"));
                    actions.push(CodeAction {
                        title: format!("Create stage '{name}'"),
                        kind: "quickfix".to_string(),
                        diagnostics: vec![diagnostic.code.clone()],
                        edit: Some(WorkspaceEdit {
                            create_files: vec![CreateFile {
                                path: file.display().to_string(),
                                text: format!(
                                    "Theatre: {theatre}\n\nStage: {stage}\n    Description: TODO\n"
                                ),
                                overwrite: false,
                            }],
                            ..WorkspaceEdit::default()
                        }),
                        command: None,
                    });
                }
            }
            "IMP-DSL-009" => {
                if let Some(action) = self.create_piece_action(path, diagnostic) {
                    actions.push(action);
                }
            }
            "IMP-DSL-012" => {
                if let Some(action) = self.add_platform_column_action(path, doc, diagnostic) {
                    actions.push(action);
                }
            }
            "IMP-DSL-006" | "IMP-DSL-007" | "IMP-DSL-030" | "IMP-DSL-033" | "IMP-DSL-035"
            | "IMP-DSL-038" => {
                let name = self
                    .index
                    .occurrence_at(path, diagnostic.span.line, diagnostic.span.col)
                    .map(|occurrence| self.index.key_name(&occurrence.key))
                    .or_else(|| quoted_name(&diagnostic.message))
                    .unwrap_or_else(|| "Item".to_string());
                let unique = unique_name(&name, |candidate| self.name_exists(path, candidate));
                let mut edit = WorkspaceEdit::default();
                edit.add(TextEdit {
                    span: diagnostic.span.clone(),
                    new_text: unique.clone(),
                });
                actions.push(CodeAction {
                    title: format!("Make duplicate unique ('{unique}')"),
                    kind: "quickfix".to_string(),
                    diagnostics: vec![diagnostic.code.clone()],
                    edit: Some(edit),
                    command: None,
                });
            }
            "IMP-DSL-032" => {
                // The core renders declared parameters into the help text:
                // "parameters: user, password". Offer the nearest one.
                if let Some(list) = diagnostic
                    .help
                    .as_deref()
                    .and_then(|help| help.strip_prefix("parameters: "))
                {
                    let candidates = list
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .collect::<Vec<_>>();
                    let actual = last_quoted(&diagnostic.message).unwrap_or_default();
                    if let Some(suggestion) = suggest(&actual, &candidates)
                        .and_then(|hint| quoted_names(&hint).into_iter().next())
                    {
                        let mut edit = WorkspaceEdit::default();
                        edit.add(TextEdit {
                            span: diagnostic.span.clone(),
                            new_text: suggestion.clone(),
                        });
                        actions.push(CodeAction {
                            title: format!("Change parameter to '{suggestion}'"),
                            kind: "quickfix".to_string(),
                            diagnostics: vec![diagnostic.code.clone()],
                            edit: Some(edit),
                            command: None,
                        });
                    }
                }
            }
            "IMP-DSL-042" | "IMP-DSL-043" | "IMP-DSL-044" | "IMP-DSL-045" | "IMP-DSL-046"
            | "IMP-DSL-047" => {
                let mut edit = WorkspaceEdit::default();
                if matches!(diagnostic.code.as_str(), "IMP-DSL-042" | "IMP-DSL-045") {
                    edit.delete_files.push(path.display().to_string());
                } else if matches!(diagnostic.code.as_str(), "IMP-DSL-044") {
                    delete_block(doc, path, line, &mut edit);
                } else {
                    edit.add(TextEdit {
                        span: line_span_with_newline(doc, path, line),
                        new_text: String::new(),
                    });
                }
                actions.push(CodeAction {
                    title: "Remove unused declaration".to_string(),
                    kind: "quickfix".to_string(),
                    diagnostics: vec![diagnostic.code.clone()],
                    edit: Some(edit),
                    command: None,
                });
            }
            _ => {}
        }
    }

    fn create_piece_action(&self, path: &Path, diagnostic: &DiagnosticInfo) -> Option<CodeAction> {
        let occurrence =
            self.index
                .occurrence_at(path, diagnostic.span.line, diagnostic.span.col)?;
        let crate::model::SymbolKey::Piece { scene, name: _ } = &occurrence.key else {
            return None;
        };
        let scene_info = self.index.scenes.get(scene)?;
        let scene_name = scene_info.name.clone();
        let scene_path = scene_info.path.clone();
        let scene_text = self.source(&scene_path)?;
        let scene_doc = Doc::new(scene_text);
        let name = quoted_name(&diagnostic.message)?;
        let mut edit = WorkspaceEdit::default();
        if let Some(column_line) = table_header_line(&scene_doc, &scene_path) {
            let data_end = table_data_end(&scene_doc, column_line);
            let col = u32::try_from(scene_doc.line(data_end).chars().count()).unwrap_or(0);
            let insert = if scene_info.columns.is_empty() {
                format!("\n| {name} |")
            } else {
                format!(
                    "\n| {name} |{}",
                    " - |".repeat(scene_info.columns.len().saturating_sub(1))
                )
            };
            edit.add(TextEdit {
                span: Span::new(&scene_path, data_end as usize, col as usize, 0),
                new_text: insert,
            });
        } else if let Some(pieces_line) = line_starting_with(&scene_doc, "Pieces:") {
            let col = u32::try_from(scene_doc.line(pieces_line).chars().count()).unwrap_or(0) + 1;
            edit.add(TextEdit {
                span: Span::new(&scene_path, pieces_line as usize, col as usize, 0),
                new_text: format!("\n    {name}: web=\"#todo\""),
            });
        } else {
            let col = u32::try_from(scene_doc.line(scene_doc.end_line()).chars().count())
                .unwrap_or(0)
                + 1;
            edit.add(TextEdit {
                span: Span::new(&scene_path, scene_doc.end_line() as usize, col as usize, 0),
                new_text: format!("\nPieces:\n    {name}: web=\"#todo\""),
            });
        }
        Some(CodeAction {
            title: format!("Create piece '{name}' in scene '{scene_name}'"),
            kind: "quickfix".to_string(),
            diagnostics: vec![diagnostic.code.clone()],
            edit: Some(edit),
            command: None,
        })
    }

    fn add_platform_column_action(
        &self,
        path: &Path,
        doc: &Doc<'_>,
        diagnostic: &DiagnosticInfo,
    ) -> Option<CodeAction> {
        let platform = PlatformName::from_message(&diagnostic.message)?;
        let header = table_header_line(doc, path)?;
        let data_end = table_data_end(doc, header);
        let mut edit = WorkspaceEdit::default();
        for line in header..=data_end {
            let content = doc.line(line);
            if !content.trim_start().starts_with('|') {
                continue;
            }
            let Some(pipe) = content.rfind('|') else {
                continue;
            };
            let col = u32::try_from(content.get(..pipe)?.chars().count()).unwrap_or(0) + 1;
            let cell = if line == header {
                format!(" {} ", platform.as_str())
            } else if content
                .chars()
                .all(|ch| ch == '|' || ch == '-' || ch == ':' || ch == ' ')
            {
                "---".to_string()
            } else {
                " - ".to_string()
            };
            edit.add(TextEdit {
                span: Span::new(path, line as usize, col as usize, 0),
                new_text: format!("{cell}|"),
            });
        }
        if edit.changes.is_empty() {
            return None;
        }
        Some(CodeAction {
            title: format!("Add missing '{}' column", platform.as_str()),
            kind: "quickfix".to_string(),
            diagnostics: vec![diagnostic.code.clone()],
            edit: Some(edit),
            command: None,
        })
    }

    fn extract_action(&self, path: &Path, doc: &Doc<'_>, from: u32, to: u32) -> Option<CodeAction> {
        if self.play(path).is_none() || from >= to {
            return None;
        }
        let selected = (from..=to).map(|line| doc.line(line)).collect::<Vec<_>>();
        if !selected.iter().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("Action:") || trimmed.starts_with("Check:")
        }) {
            return None;
        }
        let root = self.root_for(path);
        let mut name = "ExtractedStage".to_string();
        let mut suffix = 2u32;
        while self.index.stages.contains_key(&name) {
            name = format!("ExtractedStage{suffix}");
            suffix += 1;
        }
        let indent = doc
            .line(from)
            .chars()
            .take_while(|ch| *ch == ' ')
            .collect::<String>();
        let body = selected
            .iter()
            .map(|line| {
                line.strip_prefix(&indent)
                    .map_or_else(|| line.to_string(), |line| format!("    {line}"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let theatre = root.join("theatres").join(format!("{name}.theatre"));
        let mut edit = WorkspaceEdit::default();
        edit.create_files.push(CreateFile {
            path: theatre.display().to_string(),
            text: format!("Theatre: Extracted\n\nStage: {name}\n{body}\n"),
            overwrite: false,
        });
        edit.add(TextEdit {
            span: block_span(path, doc, from, to),
            new_text: format!("{indent}Perform: {name}"),
        });
        Some(CodeAction {
            title: format!("Extract selected steps to stage '{name}'"),
            kind: "refactor.extract".to_string(),
            diagnostics: Vec::new(),
            edit: Some(edit),
            command: None,
        })
    }

    fn name_exists(&self, _path: &Path, name: &str) -> bool {
        self.index.scenes.contains_key(name)
            || self.index.stages.contains_key(name)
            || self.index.theatres.contains_key(name)
            || self.workspace.plays.iter().any(|play| {
                play.name == name
                    || play.var_decls.iter().any(|var| var.name == name)
                    || play.acts.iter().any(|act| act.name == name)
            })
    }
}

fn did_you_mean(diagnostic: &DiagnosticInfo) -> Option<String> {
    let help = diagnostic.help.as_deref()?;
    let lower = help.to_ascii_lowercase();
    let marker = lower.find("did you mean")?;
    let rest = help.get(marker..)?;
    let start = rest.find('\'')?;
    let rest = rest.get(start + 1..)?;
    let end = rest.find('\'')?;
    rest.get(..end).map(ToString::to_string)
}

fn quoted_name(message: &str) -> Option<String> {
    quoted_names(message).into_iter().next()
}

fn last_quoted(message: &str) -> Option<String> {
    quoted_names(message).into_iter().next_back()
}

fn quoted_names(message: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = message;
    while let Some(start) = rest.find('\'') {
        let Some(after) = rest.get(start + 1..) else {
            break;
        };
        let Some(end) = after.find('\'') else {
            break;
        };
        if let Some(name) = after.get(..end) {
            names.push(name.to_string());
        }
        rest = after.get(end + 1..).unwrap_or("");
    }
    names
}

fn unique_name(original: &str, exists: impl Fn(&str) -> bool) -> String {
    let base = original.trim_end_matches(|ch: char| ch.is_ascii_digit());
    let base = if base.is_empty() { original } else { base };
    for index in 2..1000 {
        let candidate = format!("{base}{index}");
        if !exists(&candidate) {
            return candidate;
        }
    }
    format!("{base}Copy")
}

fn line_span_with_newline(doc: &Doc<'_>, path: &Path, line: u32) -> Span {
    let length = doc.line(line).chars().count() + 1;
    Span::new(path, line as usize, 1, length)
}

fn block_span(path: &Path, doc: &Doc<'_>, start: u32, end: u32) -> Span {
    let length = (start..=end)
        .map(|line| doc.line(line).chars().count())
        .sum::<usize>()
        + usize::try_from(end.saturating_sub(start)).unwrap_or(0);
    Span::new(path, start as usize, 1, length)
}

fn delete_block(doc: &Doc<'_>, path: &Path, line: u32, edit: &mut WorkspaceEdit) {
    let indent = doc.indent(line);
    let mut end = line;
    while end < doc.end_line() {
        let next = end + 1;
        let content = doc.line(next);
        if content.trim().is_empty() {
            end = next;
            continue;
        }
        if doc.indent(next) <= indent {
            break;
        }
        end = next;
    }
    edit.add(TextEdit {
        span: block_span(path, doc, line, end),
        new_text: String::new(),
    });
}

fn table_header_line(doc: &Doc<'_>, path: &Path) -> Option<u32> {
    let scene = doc.line(1).trim_start();
    if !scene.starts_with("Scene:") {
        return None;
    }
    let _ = path;
    (2..=doc.end_line()).find(|line| doc.line(*line).trim_start().starts_with('|'))
}

fn table_data_end(doc: &Doc<'_>, header: u32) -> u32 {
    let mut end = header;
    while end < doc.end_line() && doc.line(end + 1).trim_start().starts_with('|') {
        end += 1;
    }
    end
}

fn line_starting_with(doc: &Doc<'_>, needle: &str) -> Option<u32> {
    (1..=doc.end_line()).find(|line| doc.line(*line).trim() == needle)
}

struct PlatformName(String);

impl PlatformName {
    fn from_message(message: &str) -> Option<Self> {
        for platform in impresario_core::platforms::Platform::ALL {
            if message.contains(platform.as_str()) {
                return Some(Self(platform.as_str().to_string()));
            }
        }
        None
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// The diagnostics overlapping a selection, useful for tests and clients.
#[must_use]
pub fn diagnostics_in_range(
    diagnostics: &[DiagnosticInfo],
    from: u32,
    to: u32,
) -> Vec<&DiagnosticInfo> {
    diagnostics
        .iter()
        .filter(|diagnostic| {
            let line = u32::try_from(diagnostic.span.line).unwrap_or(u32::MAX);
            line >= from && line <= to
        })
        .collect()
}

/// The absolute path of a workspace edit file key.
#[must_use]
pub fn edit_path(key: &str) -> PathBuf {
    PathBuf::from(key)
}
