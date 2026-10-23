//! The editor-neutral language service.
//!
//! Both `impresario lsp` and the standalone VS Code extension call this module
//! through thin adapters, so completion, navigation, refactoring, diagnostics
//! and formatting cannot drift apart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use impresario_core::span::Span;
use impresario_core::validation::DeviceRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diagnostics::{analyze_sources, DiagnosticInfo};
use crate::model::{self, AppInfo, DeviceInfo, Index, SourceFile};
use crate::text::Position;

/// One text edit in a source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub span: Span,
    pub new_text: String,
}

/// A file created by a workspace edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateFile {
    pub path: String,
    pub text: String,
    pub overwrite: bool,
}

/// A file renamed by a workspace edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameFile {
    pub old_path: String,
    pub new_path: String,
}

/// An editor-neutral workspace edit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEdit {
    pub changes: BTreeMap<String, Vec<TextEdit>>,
    pub create_files: Vec<CreateFile>,
    pub delete_files: Vec<String>,
    pub rename_files: Vec<RenameFile>,
}

impl WorkspaceEdit {
    /// Add one edit, coalescing repeated edits in the same file.
    pub fn add(&mut self, edit: TextEdit) {
        self.changes
            .entry(edit.span.file.display().to_string())
            .or_default()
            .push(edit);
    }

    /// True when the edit does nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.values().all(Vec::is_empty)
            && self.create_files.is_empty()
            && self.delete_files.is_empty()
            && self.rename_files.is_empty()
    }
}

/// A completion item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    pub label: String,
    pub kind: String,
    pub detail: Option<String>,
    pub insert_text: Option<String>,
    pub sort_text: Option<String>,
    pub filter_text: Option<String>,
    pub documentation: Option<String>,
    pub replace: Option<TextEdit>,
}

/// A hover response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoverResult {
    pub markdown: String,
    pub span: Option<Span>,
}

/// A document symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: String,
    pub span: Span,
    pub selection_span: Span,
    pub children: Vec<DocumentSymbol>,
}

/// A workspace symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: String,
    pub span: Span,
}

/// A folding range. Lines are 1-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldingRange {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: Option<String>,
}

/// A semantic token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticToken {
    pub span: Span,
    pub token_type: String,
    pub modifiers: Vec<String>,
}

/// A signature help parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureParameter {
    pub label: String,
    pub documentation: Option<String>,
}

/// One signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub label: String,
    pub documentation: Option<String>,
    pub parameters: Vec<SignatureParameter>,
}

/// Signature help.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureHelp {
    pub signatures: Vec<Signature>,
    pub active_signature: u32,
    pub active_parameter: u32,
}

/// A quick fix or refactoring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAction {
    pub title: String,
    pub kind: String,
    pub diagnostics: Vec<String>,
    pub edit: Option<WorkspaceEdit>,
    pub command: Option<Command>,
}

/// A client command (code lens, quick action).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub title: String,
    pub command: String,
    pub arguments: Vec<Value>,
}

/// An inlay hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlayHint {
    pub span: Span,
    pub label: String,
    pub kind: String,
}

/// A code lens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeLens {
    pub span: Span,
    pub command: Command,
}

/// The prepared range of a rename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareRename {
    pub span: Span,
    pub placeholder: String,
}

/// The complete editor language service.
#[derive(Clone)]
pub struct Language {
    pub workspace: impresario_core::workspace::Workspace,
    pub index: Index,
    pub sources: BTreeMap<PathBuf, String>,
    pub devices: Vec<DeviceInfo>,
    pub apps: Vec<AppInfo>,
    pub diagnostics: BTreeMap<PathBuf, Vec<DiagnosticInfo>>,
    pub root: Option<PathBuf>,
}

impl Language {
    /// Parse and index a workspace from in-memory buffers.
    ///
    /// The apps list is informational here; use [`Self::analyze_with_apps`]
    /// when an `impresario.apps.config` is authoritative and unknown `App:`
    /// names must be diagnosed.
    #[must_use]
    pub fn analyze(
        sources: &[SourceFile],
        devices: &[DeviceRef],
        apps: &[AppInfo],
        root: Option<PathBuf>,
    ) -> Self {
        Self::analyze_inner(sources, devices, apps, root, false)
    }

    /// Like [`Self::analyze`], but every `App:` name must be in `apps`.
    #[must_use]
    pub fn analyze_with_apps(
        sources: &[SourceFile],
        devices: &[DeviceRef],
        apps: &[AppInfo],
        root: Option<PathBuf>,
    ) -> Self {
        Self::analyze_inner(sources, devices, apps, root, true)
    }

    fn analyze_inner(
        sources: &[SourceFile],
        devices: &[DeviceRef],
        apps: &[AppInfo],
        root: Option<PathBuf>,
        check_apps: bool,
    ) -> Self {
        let (workspace, _all, mut diagnostics) = analyze_sources(sources, devices);
        if check_apps {
            crate::diagnostics::add_app_diagnostics(&workspace, apps, &mut diagnostics);
        }
        let index = Index::build(&workspace);
        Self {
            workspace,
            index,
            sources: sources
                .iter()
                .map(|source| (source.path.clone(), source.text.clone()))
                .collect(),
            devices: devices
                .iter()
                .map(|device| DeviceInfo {
                    name: device.name.clone(),
                    platform: device.platform,
                })
                .collect(),
            apps: apps.to_vec(),
            diagnostics,
            root,
        }
    }

    /// Diagnostics for one file.
    #[must_use]
    pub fn diagnostics_for(&self, path: &Path) -> &[DiagnosticInfo] {
        self.diagnostics.get(path).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The source text of one file.
    #[must_use]
    pub fn source(&self, path: &Path) -> Option<&str> {
        self.sources.get(path).map(String::as_str)
    }

    /// The parsed scene for a file.
    #[must_use]
    pub fn scene(&self, path: &Path) -> Option<&impresario_core::ast::SceneFile> {
        self.workspace
            .scenes
            .iter()
            .find(|scene| scene.path == path)
    }

    /// The parsed theatre for a file.
    #[must_use]
    pub fn theatre(&self, path: &Path) -> Option<&impresario_core::ast::TheatreFile> {
        self.workspace
            .theatres
            .iter()
            .find(|theatre| theatre.path == path)
    }

    /// The parsed play for a file.
    #[must_use]
    pub fn play(&self, path: &Path) -> Option<&impresario_core::ast::PlayFile> {
        self.workspace.plays.iter().find(|play| play.path == path)
    }

    /// The configured platforms, de-duplicated, or every platform.
    #[must_use]
    pub fn platforms(&self) -> Vec<impresario_core::platforms::Platform> {
        let mut platforms = self
            .devices
            .iter()
            .map(|device| device.platform)
            .collect::<Vec<_>>();
        platforms.sort();
        platforms.dedup();
        if platforms.is_empty() {
            impresario_core::platforms::Platform::ALL.to_vec()
        } else {
            platforms
        }
    }

    /// The workspace root or the parent of a file.
    #[must_use]
    pub fn root_for(&self, path: &Path) -> PathBuf {
        self.root
            .clone()
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_default()
    }

    /// The key under which a file is serialized in workspace edits.
    #[must_use]
    pub fn key(path: &Path) -> String {
        path.display().to_string()
    }

    /// All symbols in the workspace, for `workspace/symbol`.
    #[must_use]
    pub fn workspace_symbols(&self, query: &str) -> Vec<WorkspaceSymbol> {
        let needle = query.to_ascii_lowercase();
        self.index
            .workspace_symbols
            .iter()
            .filter(|symbol| {
                needle.is_empty() || symbol.name.to_ascii_lowercase().contains(&needle)
            })
            .map(|symbol| WorkspaceSymbol {
                name: symbol.name.clone(),
                detail: symbol.detail.clone(),
                kind: Index::key_kind(&symbol.key).to_string(),
                span: symbol.span.clone(),
            })
            .collect()
    }

    /// Render one symbol reference in a compact form.
    #[must_use]
    pub fn location(&self, span: &Span) -> Span {
        span.clone()
    }

    /// The key name of a symbol (diagnostics, tests).
    #[must_use]
    pub fn key_name(&self, key: &model::SymbolKey) -> String {
        self.index.key_name(key)
    }

    /// The line prefix before a position, for context-sensitive features.
    #[must_use]
    pub fn prefix(&self, path: &Path, position: Position) -> String {
        let Some(text) = self.source(path) else {
            return String::new();
        };
        let doc = crate::text::Doc::new(text);
        let line = doc.line(position.line);
        line.chars()
            .take(position.col.saturating_sub(1) as usize)
            .collect()
    }
}
