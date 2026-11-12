//! Lossless scene authoring for the inspector (spec section 9).
//!
//! Every operation edits the original source text instead of re-rendering the
//! AST: untouched rows, comments, ordering and alignment keep their exact
//! bytes. The only rows that change are the ones an operation targets (plus
//! every row when a column is added, because a column is a table-wide change).
//!
//! Edits are previews. [`SceneDocument::fill_cell`] and friends return a
//! [`SceneEdit`] with the new source, the parsed scene and a unified diff;
//! nothing is written until [`SceneDocument::commit`] is called. Commit uses
//! optimistic concurrency: the edit records the disk revision it was based on
//! and refuses to overwrite a file that changed in the meantime. The conflict
//! error carries the fresh disk content so a client can rebase.
//!
//! Writes are atomic (temporary file in the same directory, then rename).

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::ast::{PieceDecl, SceneFile};
use crate::diag::{Diagnostics, Severity};
use crate::parser;
use crate::platforms::Platform;
use crate::scanner::LogicalLine;
use crate::span::Span;
use crate::validation;

/// The result of committing an edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneCommit {
    /// The file that was written.
    pub path: PathBuf,
    /// SHA-256 of the committed content.
    pub revision: String,
}

/// A structured scene-authoring failure.
#[derive(Debug, thiserror::Error)]
pub enum SceneEditError {
    /// The file changed on disk since the edit was based on it.
    #[error("SCENE_CONFLICT: the scene changed on disk (expected {expected:?}, found {actual:?})")]
    Conflict {
        /// Revision the edit was based on (`None` for a new file).
        expected: Option<String>,
        /// Revision found on disk (`None` when the file is missing).
        actual: Option<String>,
        /// Fresh disk content, when it could be read.
        fresh_source: Option<String>,
    },
    /// The edited source no longer parses into a scene AST.
    #[error("SCENE_INVALID: the edited scene is not parseable")]
    Invalid {
        /// Parser and validation diagnostics.
        diagnostics: Diagnostics,
    },
    /// A filesystem operation failed.
    #[error("SCENE_IO: {path:?}: {source}")]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The target path is outside the workspace root.
    #[error("SCENE_PATH_OUTSIDE_WORKSPACE: {path:?} is outside {root:?}")]
    PathOutsideWorkspace {
        /// The rejected path.
        path: PathBuf,
        /// The workspace root.
        root: PathBuf,
    },
    /// `CreateScene` was asked to overwrite an existing file.
    #[error("SCENE_ALREADY_EXISTS: {path:?} already exists")]
    AlreadyExists {
        /// The existing path.
        path: PathBuf,
    },
    /// An edit belongs to a different document.
    #[error("SCENE_PATH_MISMATCH: edit targets {expected:?}, document is {actual:?}")]
    PathMismatch {
        /// The edit's path.
        expected: PathBuf,
        /// The document's path.
        actual: PathBuf,
    },
    /// The scene has no `| Piece | ... |` table.
    #[error("SCENE_INVALID: the scene has no piece table")]
    MissingTable,
    /// A table row does not match the header.
    #[error("SCENE_INVALID: the piece table is malformed")]
    InvalidTable,
    /// The requested column does not exist.
    #[error("SCENE_UNKNOWN_COLUMN: the scene has no column '{column}' (available: {available:?})")]
    UnknownColumn {
        /// The requested column.
        column: String,
        /// Platform columns that do exist.
        available: Vec<String>,
    },
    /// The requested row does not exist.
    #[error("SCENE_UNKNOWN_ROW: the scene has no piece '{row}' (available: {available:?})")]
    UnknownRow {
        /// The requested piece.
        row: String,
        /// Pieces that do exist.
        available: Vec<String>,
    },
    /// A row with that name already exists.
    #[error("SCENE_DUPLICATE_ROW: the scene already has a piece '{row}'")]
    DuplicateRow {
        /// The duplicate name.
        row: String,
    },
    /// A column for that platform already exists.
    #[error("SCENE_DUPLICATE_COLUMN: the scene already has a column '{column}'")]
    DuplicateColumn {
        /// The duplicate column key.
        column: String,
    },
    /// `FillCell` was asked to overwrite an occupied cell.
    #[error("SCENE_CELL_OCCUPIED: '{row}' already has a locator in '{column}'")]
    CellOccupied {
        /// The row name.
        row: String,
        /// The column key.
        column: String,
    },
    /// A row or scene name is not a DSL identifier.
    #[error("SCENE_INVALID_NAME: '{name}' is not a valid name: {reason}")]
    InvalidName {
        /// The rejected name.
        name: String,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// A locator cell would not round-trip through the table grammar.
    #[error("SCENE_INVALID_LOCATOR: {reason}")]
    InvalidLocator {
        /// Why the locator was rejected.
        reason: String,
    },
    /// `ReplaceCell` needs explicit confirmation.
    #[error("SCENE_CONFIRM_REQUIRED: replacing an occupied cell needs confirmation")]
    ConfirmationRequired {
        /// The diff the caller must show before confirming.
        diff: String,
    },
}

impl SceneEditError {
    /// A stable machine-readable code for MCP/HTTP clients.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            SceneEditError::Conflict { .. } => "SCENE_CONFLICT",
            SceneEditError::Invalid { .. } => "SCENE_INVALID",
            SceneEditError::Io { .. } => "SCENE_IO",
            SceneEditError::PathOutsideWorkspace { .. } => "SCENE_PATH_OUTSIDE_WORKSPACE",
            SceneEditError::AlreadyExists { .. } => "SCENE_ALREADY_EXISTS",
            SceneEditError::PathMismatch { .. } => "SCENE_PATH_MISMATCH",
            SceneEditError::MissingTable | SceneEditError::InvalidTable => "SCENE_INVALID",
            SceneEditError::UnknownColumn { .. } => "SCENE_UNKNOWN_COLUMN",
            SceneEditError::UnknownRow { .. } => "SCENE_UNKNOWN_ROW",
            SceneEditError::DuplicateRow { .. } => "SCENE_DUPLICATE_ROW",
            SceneEditError::DuplicateColumn { .. } => "SCENE_DUPLICATE_COLUMN",
            SceneEditError::CellOccupied { .. } => "SCENE_CELL_OCCUPIED",
            SceneEditError::InvalidName { .. } => "SCENE_INVALID_NAME",
            SceneEditError::InvalidLocator { .. } => "SCENE_INVALID_LOCATOR",
            SceneEditError::ConfirmationRequired { .. } => "SCENE_CONFIRM_REQUIRED",
        }
    }
}

/// An open scene file and its current in-memory state.
#[derive(Debug, Clone)]
pub struct SceneDocument {
    path: PathBuf,
    source: String,
    revision: String,
    base_revision: Option<String>,
    scene: SceneFile,
    diagnostics: Diagnostics,
}

impl SceneDocument {
    /// Open an existing `.scene` file.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, SceneEditError> {
        let path = path.into();
        let source = fs::read_to_string(&path).map_err(|source| SceneEditError::Io {
            path: path.clone(),
            source,
        })?;
        let revision = revision_of(&source);
        Self::from_source(path, source, Some(revision))
    }

    /// Open a scene that must stay inside `root`.
    pub fn open_in_workspace(
        root: impl AsRef<Path>,
        path: impl AsRef<Path>,
    ) -> Result<Self, SceneEditError> {
        let resolved = resolve_in_workspace(root.as_ref(), path.as_ref())?;
        Self::open(resolved)
    }

    /// Create a new scene skeleton in memory. Nothing is written until
    /// [`SceneDocument::commit`] is called; an existing file is never
    /// overwritten.
    pub fn create(
        path: impl Into<PathBuf>,
        name: &str,
        columns: &[Platform],
    ) -> Result<Self, SceneEditError> {
        let path = path.into();
        if path.exists() {
            return Err(SceneEditError::AlreadyExists { path });
        }
        let source = skeleton(name, columns)?;
        Self::from_source(path, source, None)
    }

    /// Create a new scene that must stay inside `root`.
    pub fn create_in_workspace(
        root: impl AsRef<Path>,
        path: impl AsRef<Path>,
        name: &str,
        columns: &[Platform],
    ) -> Result<Self, SceneEditError> {
        let resolved = resolve_in_workspace(root.as_ref(), path.as_ref())?;
        Self::create(resolved, name, columns)
    }

    fn from_source(
        path: PathBuf,
        source: String,
        base_revision: Option<String>,
    ) -> Result<Self, SceneEditError> {
        let (scene, diagnostics) = parse_source(&source, &path)?;
        let revision = revision_of(&source);
        Ok(Self {
            path,
            source,
            revision,
            base_revision,
            scene,
            diagnostics,
        })
    }

    /// The scene path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// SHA-256 of the current in-memory source.
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// The disk revision this document was opened from (`None` for a new file).
    #[must_use]
    pub fn base_revision(&self) -> Option<&str> {
        self.base_revision.as_deref()
    }

    /// The current source text.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The parsed scene.
    #[must_use]
    pub fn scene(&self) -> &SceneFile {
        &self.scene
    }

    /// Parser and validation diagnostics for the current source.
    #[must_use]
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    /// True when the current source has no error diagnostics.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.diagnostics.has_errors()
    }

    /// The revision currently on disk (`None` when the file is missing).
    pub fn disk_revision(&self) -> Result<Option<String>, SceneEditError> {
        match fs::read_to_string(&self.path) {
            Ok(source) => Ok(Some(revision_of(&source))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(SceneEditError::Io {
                path: self.path.clone(),
                source,
            }),
        }
    }

    /// Reload the document when the file changed on disk.
    ///
    /// Returns `true` when new content was loaded. A missing file is left
    /// untouched (the document can still be committed as a new file).
    pub fn refresh(&mut self) -> Result<bool, SceneEditError> {
        let source = match fs::read_to_string(&self.path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(SceneEditError::Io {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        let revision = revision_of(&source);
        if self.base_revision.as_deref() == Some(revision.as_str()) {
            return Ok(false);
        }
        let (scene, diagnostics) = parse_source(&source, &self.path)?;
        self.source = source;
        self.revision = revision.clone();
        self.base_revision = Some(revision);
        self.scene = scene;
        self.diagnostics = diagnostics;
        Ok(true)
    }

    /// Add a new piece row and place `locator` in `column`.
    pub fn add_element(
        &self,
        name: &str,
        column: &str,
        locator: &str,
    ) -> Result<SceneEdit, SceneEditError> {
        let name = name.trim();
        validate_ident(name)?;
        if self.scene.piece(name).is_some() {
            return Err(SceneEditError::DuplicateRow {
                row: name.to_string(),
            });
        }
        let layout = table_layout(&self.source, &self.path)?;
        let header = header_cells(&self.source, &self.path, layout.header_line)?;
        let target = column_index(&self.scene, &header, column)?;
        let value = cell_value(locator)?;

        let mut values: Vec<String> = Vec::new();
        for index in 1..header.len() {
            if index == target {
                values.push(value.clone());
            } else {
                values.push("-".to_string());
            }
        }
        let row = if values.is_empty() {
            format!("| {name} |")
        } else {
            format!("| {name} | {} |", values.join(" | "))
        };
        let after = layout
            .table_lines
            .last()
            .copied()
            .ok_or(SceneEditError::MissingTable)?;
        let source = append_row(&self.source, after, &row)?;
        self.preview(source)
    }

    /// Fill an existing row's empty cell.
    pub fn fill_cell(
        &self,
        row: &str,
        column: &str,
        locator: &str,
    ) -> Result<SceneEdit, SceneEditError> {
        let piece = self.piece(row)?;
        let layout = table_layout(&self.source, &self.path)?;
        let header = header_cells(&self.source, &self.path, layout.header_line)?;
        let target = column_index(&self.scene, &header, column)?;
        let cells = row_cells(&self.source, &self.path, piece)?;
        let cell = cells.get(target).ok_or(SceneEditError::InvalidTable)?;
        if !cell.absent {
            return Err(SceneEditError::CellOccupied {
                row: piece.name.clone(),
                column: column.to_string(),
            });
        }
        let value = cell_value(locator)?;
        let source = replace_span(&self.source, &cell.span, &value)?;
        self.preview(source)
    }

    /// Replace an occupied cell. Without `confirm`, returns the diff that the
    /// caller must show first.
    pub fn replace_cell(
        &self,
        row: &str,
        column: &str,
        locator: &str,
        confirm: bool,
    ) -> Result<SceneEdit, SceneEditError> {
        let piece = self.piece(row)?;
        let layout = table_layout(&self.source, &self.path)?;
        let header = header_cells(&self.source, &self.path, layout.header_line)?;
        let target = column_index(&self.scene, &header, column)?;
        let cells = row_cells(&self.source, &self.path, piece)?;
        let cell = cells.get(target).ok_or(SceneEditError::InvalidTable)?;
        let value = cell_value(locator)?;
        let source = replace_span(&self.source, &cell.span, &value)?;
        if !confirm {
            return Err(SceneEditError::ConfirmationRequired {
                diff: unified_diff(&self.path, &self.source, &source),
            });
        }
        self.preview(source)
    }

    /// Rename a piece row.
    pub fn rename_row(&self, row: &str, new_name: &str) -> Result<SceneEdit, SceneEditError> {
        let piece = self.piece(row)?;
        let new_name = new_name.trim();
        validate_ident(new_name)?;
        if new_name != piece.name && self.scene.piece(new_name).is_some() {
            return Err(SceneEditError::DuplicateRow {
                row: new_name.to_string(),
            });
        }
        let source = replace_span(&self.source, &piece.name_span, new_name)?;
        self.preview(source)
    }

    /// Remove a piece row.
    pub fn remove_row(&self, row: &str) -> Result<SceneEdit, SceneEditError> {
        let piece = self.piece(row)?;
        let source = remove_line(&self.source, piece.span.line)?;
        self.preview(source)
    }

    /// Add a platform column at the end of the table.
    pub fn add_column(&self, platform: Platform) -> Result<SceneEdit, SceneEditError> {
        if self
            .scene
            .columns
            .iter()
            .any(|column| column.platform == Some(platform))
        {
            return Err(SceneEditError::DuplicateColumn {
                column: platform.as_str().to_string(),
            });
        }
        let layout = table_layout(&self.source, &self.path)?;
        let header = header_cells(&self.source, &self.path, layout.header_line)?;
        if header.len() != self.scene.columns.len() + 1 {
            return Err(SceneEditError::InvalidTable);
        }

        let name = platform.as_str();
        let mut source = append_cell(&self.source, layout.header_line, &format!("| {name} "))?;
        for line in layout.table_lines.iter().copied().skip(1) {
            let is_row = layout.row_lines.binary_search(&line).is_ok();
            let cell = if is_row { "| - " } else { "|-------" };
            source = append_cell(&source, line, cell)?;
        }
        self.preview(source)
    }

    /// The unique UpperCamelCase row name derived from `seed`.
    #[must_use]
    pub fn suggest_row_name(&self, seed: &str) -> String {
        suggest_row_name(&self.scene, seed)
    }

    /// Diagnostics for the current in-memory source.
    #[must_use]
    pub fn validate(&self) -> &Diagnostics {
        &self.diagnostics
    }

    fn piece(&self, row: &str) -> Result<&PieceDecl, SceneEditError> {
        let row = row.trim();
        self.scene
            .pieces
            .iter()
            .find(|piece| piece.name == row)
            .ok_or_else(|| SceneEditError::UnknownRow {
                row: row.to_string(),
                available: self
                    .scene
                    .pieces
                    .iter()
                    .map(|piece| piece.name.clone())
                    .collect(),
            })
    }

    fn preview(&self, source: String) -> Result<SceneEdit, SceneEditError> {
        let (scene, diagnostics) = parse_source(&source, &self.path)?;
        let revision = revision_of(&source);
        let diff = unified_diff(&self.path, &self.source, &source);
        Ok(SceneEdit {
            path: self.path.clone(),
            base_revision: self.base_revision.clone(),
            revision,
            source,
            diff,
            scene,
            diagnostics,
        })
    }

    /// Write a previewed edit atomically, checking the disk revision first.
    pub fn commit(&mut self, edit: &SceneEdit) -> Result<SceneCommit, SceneEditError> {
        if edit.path != self.path {
            return Err(SceneEditError::PathMismatch {
                expected: edit.path.clone(),
                actual: self.path.clone(),
            });
        }
        match &edit.base_revision {
            Some(expected) => {
                let disk = match fs::read_to_string(&self.path) {
                    Ok(disk) => disk,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        return Err(SceneEditError::Conflict {
                            expected: Some(expected.clone()),
                            actual: None,
                            fresh_source: None,
                        })
                    }
                    Err(source) => {
                        return Err(SceneEditError::Io {
                            path: self.path.clone(),
                            source,
                        })
                    }
                };
                let actual = revision_of(&disk);
                if &actual != expected {
                    return Err(SceneEditError::Conflict {
                        expected: Some(expected.clone()),
                        actual: Some(actual),
                        fresh_source: Some(disk),
                    });
                }
            }
            None => {
                if self.path.exists() {
                    let disk = fs::read_to_string(&self.path).ok();
                    return Err(SceneEditError::Conflict {
                        expected: None,
                        actual: disk.as_deref().map(revision_of),
                        fresh_source: disk,
                    });
                }
            }
        }

        atomic_write(&self.path, &edit.source).map_err(|source| SceneEditError::Io {
            path: self.path.clone(),
            source,
        })?;
        self.source = edit.source.clone();
        self.revision = edit.revision.clone();
        self.base_revision = Some(edit.revision.clone());
        self.scene = edit.scene.clone();
        self.diagnostics = edit.diagnostics.clone();
        Ok(SceneCommit {
            path: self.path.clone(),
            revision: edit.revision.clone(),
        })
    }

    /// Write `source` after checking that the disk still has
    /// `expected_revision` (the inspector's undo path).
    ///
    /// This is the same optimistic-concurrency and atomic-write path as
    /// [`Self::commit`], for restoring a previously captured source rather
    /// than applying a [`SceneEdit`].
    pub fn restore(
        &mut self,
        source: &str,
        expected_revision: Option<&str>,
    ) -> Result<SceneCommit, SceneEditError> {
        let (scene, diagnostics) = parse_source(source, &self.path)?;
        let revision = revision_of(source);
        match expected_revision {
            Some(expected) => {
                let disk = match fs::read_to_string(&self.path) {
                    Ok(disk) => disk,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        return Err(SceneEditError::Conflict {
                            expected: Some(expected.to_string()),
                            actual: None,
                            fresh_source: None,
                        })
                    }
                    Err(source) => {
                        return Err(SceneEditError::Io {
                            path: self.path.clone(),
                            source,
                        })
                    }
                };
                let actual = revision_of(&disk);
                if actual != expected {
                    return Err(SceneEditError::Conflict {
                        expected: Some(expected.to_string()),
                        actual: Some(actual),
                        fresh_source: Some(disk),
                    });
                }
            }
            None => {
                if self.path.exists() {
                    let disk = fs::read_to_string(&self.path).ok();
                    return Err(SceneEditError::Conflict {
                        expected: None,
                        actual: disk.as_deref().map(revision_of),
                        fresh_source: disk,
                    });
                }
            }
        }
        atomic_write(&self.path, source).map_err(|source| SceneEditError::Io {
            path: self.path.clone(),
            source,
        })?;
        self.source = source.to_string();
        self.revision = revision.clone();
        self.base_revision = Some(revision.clone());
        self.scene = scene;
        self.diagnostics = diagnostics;
        Ok(SceneCommit {
            path: self.path.clone(),
            revision,
        })
    }
}

/// A previewed scene edit. The file is unchanged until
/// [`SceneDocument::commit`] is called.
#[derive(Debug, Clone)]
pub struct SceneEdit {
    path: PathBuf,
    base_revision: Option<String>,
    revision: String,
    source: String,
    diff: String,
    scene: SceneFile,
    diagnostics: Diagnostics,
}

impl SceneEdit {
    /// The scene path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The disk revision this edit was based on.
    #[must_use]
    pub fn base_revision(&self) -> Option<&str> {
        self.base_revision.as_deref()
    }

    /// SHA-256 of the edited source.
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// The edited source text.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The unified diff from the base source to the edited source.
    #[must_use]
    pub fn diff(&self) -> &str {
        &self.diff
    }

    /// The parsed edited scene.
    #[must_use]
    pub fn scene(&self) -> &SceneFile {
        &self.scene
    }

    /// Parser and validation diagnostics for the edited source.
    #[must_use]
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    /// True when the edited source has no error diagnostics.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.diagnostics.has_errors()
    }

    /// True when the edit did not change the source.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.diff.is_empty()
    }
}

/// Validate scene source and return parser plus identity diagnostics.
#[must_use]
pub fn validate_source(source: &str, path: &Path) -> Diagnostics {
    let mut outcome = parser::parse_scene(source, path);
    if let Some(scene) = outcome.ast.as_ref() {
        validation::check_scene_identity(scene, &mut outcome.diagnostics);
    }
    outcome.diagnostics
}

/// Suggest a unique UpperCamelCase piece name derived from `seed`.
#[must_use]
pub fn suggest_row_name(scene: &SceneFile, seed: &str) -> String {
    let camel = validation::to_upper_camel(seed);
    let mut cleaned = String::with_capacity(camel.len());
    for character in camel.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            cleaned.push(character);
        }
    }
    if cleaned.is_empty() {
        cleaned.push_str("Element");
    }
    if !cleaned
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic())
    {
        cleaned.insert_str(0, "Element");
    }

    let mut candidate = cleaned.clone();
    let mut suffix = 2u32;
    while scene.pieces.iter().any(|piece| piece.name == candidate) {
        candidate = format!("{cleaned}{suffix}");
        suffix = suffix.saturating_add(1);
    }
    candidate
}

fn parse_source(source: &str, path: &Path) -> Result<(SceneFile, Diagnostics), SceneEditError> {
    let mut outcome = parser::parse_scene(source, path);
    if let Some(scene) = outcome.ast.as_ref() {
        validation::check_scene_identity(scene, &mut outcome.diagnostics);
    }
    match outcome.ast {
        Some(scene) => Ok((scene, outcome.diagnostics)),
        None => Err(SceneEditError::Invalid {
            diagnostics: outcome.diagnostics,
        }),
    }
}

fn skeleton(name: &str, columns: &[Platform]) -> Result<String, SceneEditError> {
    validate_scene_name(name)?;
    let mut header = String::from("| Piece |");
    let mut separator = String::from("|-------|");
    let mut seen: Vec<Platform> = Vec::new();
    for platform in columns {
        if seen.contains(platform) {
            continue;
        }
        seen.push(*platform);
        let column = platform.as_str();
        let _ = write!(header, " {column} |");
        let _ = write!(separator, "{}|", "-".repeat(column.len() + 2));
    }
    Ok(format!(
        "Scene: {name}\nPlatforms: cross-platform\n\n{header}\n{separator}\n"
    ))
}

fn validate_scene_name(name: &str) -> Result<(), SceneEditError> {
    validate_ident(name)
}

fn validate_ident(name: &str) -> Result<(), SceneEditError> {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return Err(SceneEditError::InvalidName {
            name: name.to_string(),
            reason: "a name cannot be empty",
        });
    };
    if !first.is_ascii_alphabetic() {
        return Err(SceneEditError::InvalidName {
            name: name.to_string(),
            reason: "a name must start with an ASCII letter",
        });
    }
    if !characters.all(|character| character.is_ascii_alphanumeric() || character == '_') {
        return Err(SceneEditError::InvalidName {
            name: name.to_string(),
            reason: "a name may only contain ASCII letters, digits and '_'",
        });
    }
    Ok(())
}

fn cell_value(locator: &str) -> Result<String, SceneEditError> {
    let trimmed = locator.trim();
    if trimmed.is_empty() {
        return Err(SceneEditError::InvalidLocator {
            reason: "a locator cannot be empty".to_string(),
        });
    }
    if trimmed
        .chars()
        .any(|character| character == '\n' || character == '\r')
    {
        return Err(SceneEditError::InvalidLocator {
            reason: "a locator must be a single line".to_string(),
        });
    }
    if trimmed.eq_ignore_ascii_case("none") || trimmed.eq_ignore_ascii_case("null") {
        return Err(SceneEditError::InvalidLocator {
            reason: "this text is reserved for an empty table cell".to_string(),
        });
    }
    if matches!(trimmed, "-" | "~") {
        return Err(SceneEditError::InvalidLocator {
            reason: "this text is reserved for an empty table cell".to_string(),
        });
    }
    let first = trimmed.chars().next().unwrap_or('\0');
    let last = trimmed.chars().next_back().unwrap_or('\0');
    if (first == '"' || first == '\'') && first == last {
        return Err(SceneEditError::InvalidLocator {
            reason: "a locator cannot start and end with the same quote in a table cell"
                .to_string(),
        });
    }

    let mut escaped = String::with_capacity(trimmed.len());
    for character in trimmed.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '|' => escaped.push_str("\\|"),
            other => escaped.push(other),
        }
    }
    Ok(escaped)
}

fn column_index(
    scene: &SceneFile,
    header: &[crate::parser::table::Cell],
    column: &str,
) -> Result<usize, SceneEditError> {
    let available: Vec<String> = scene
        .columns
        .iter()
        .filter(|declared| declared.platform.is_some())
        .map(|declared| declared.name.clone())
        .collect();
    let Some(declared) = scene.columns.iter().find(|declared| {
        declared.platform.is_some() && declared.name.eq_ignore_ascii_case(column.trim())
    }) else {
        return Err(SceneEditError::UnknownColumn {
            column: column.to_string(),
            available,
        });
    };
    header
        .iter()
        .position(|cell| {
            cell.text
                .trim()
                .eq_ignore_ascii_case(declared.name.as_str())
        })
        .ok_or(SceneEditError::InvalidTable)
}

fn header_cells(
    source: &str,
    path: &Path,
    header_line: usize,
) -> Result<Vec<crate::parser::table::Cell>, SceneEditError> {
    let index = LineIndex::new(source);
    let text = index
        .text(source, header_line)
        .ok_or(SceneEditError::InvalidTable)?;
    split_cells(text, header_line, path).ok_or(SceneEditError::InvalidTable)
}

fn row_cells(
    source: &str,
    path: &Path,
    piece: &PieceDecl,
) -> Result<Vec<crate::parser::table::Cell>, SceneEditError> {
    let index = LineIndex::new(source);
    let text = index
        .text(source, piece.span.line)
        .ok_or(SceneEditError::InvalidTable)?;
    split_cells(text, piece.span.line, path).ok_or(SceneEditError::InvalidTable)
}

struct TableLayout {
    header_line: usize,
    table_lines: Vec<usize>,
    row_lines: Vec<usize>,
}

fn table_layout(source: &str, path: &Path) -> Result<TableLayout, SceneEditError> {
    let index = LineIndex::new(source);
    let mut header_line = None;
    for line in 1..=index.line_count() {
        let Some(text) = index.text(source, line) else {
            continue;
        };
        let Some(cells) = split_cells(text, line, path) else {
            continue;
        };
        let is_header = cells
            .first()
            .is_some_and(|cell| cell.text.trim().eq_ignore_ascii_case("Piece"));
        if is_header {
            header_line = Some(line);
            break;
        }
    }
    let header_line = header_line.ok_or(SceneEditError::MissingTable)?;

    let mut table_lines = vec![header_line];
    let mut row_lines = Vec::new();
    let mut line = header_line.saturating_add(1);
    while line <= index.line_count() {
        let Some(text) = index.text(source, line) else {
            break;
        };
        let trimmed = text.trim_start();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            line = line.saturating_add(1);
            continue;
        }
        if !trimmed.starts_with('|') {
            break;
        }
        let cells = split_cells(text, line, path).ok_or(SceneEditError::InvalidTable)?;
        table_lines.push(line);
        if !is_separator(&cells) {
            row_lines.push(line);
        }
        line = line.saturating_add(1);
    }

    Ok(TableLayout {
        header_line,
        table_lines,
        row_lines,
    })
}

fn split_cells(text: &str, line: usize, path: &Path) -> Option<Vec<crate::parser::table::Cell>> {
    let logical = logical_line(text, line)?;
    crate::parser::table::split_row(&logical, path).ok()
}

fn is_separator(cells: &[crate::parser::table::Cell]) -> bool {
    !cells.is_empty() && cells.iter().all(crate::parser::table::Cell::is_separator)
}

fn logical_line(text: &str, line: usize) -> Option<LogicalLine> {
    let raw = text.strip_suffix('\r').unwrap_or(text);
    if raw.trim().is_empty() {
        return None;
    }
    let content_start = raw
        .char_indices()
        .find(|(_, character)| *character != ' ' && *character != '\t')
        .map(|(index, _)| index)?;
    let content = raw.get(content_start..)?.trim_end().to_string();
    if content.starts_with("//") {
        return None;
    }
    let indent = raw
        .get(..content_start)
        .map_or(0, |prefix| prefix.chars().count());
    Some(LogicalLine {
        line,
        depth: indent / 2,
        content,
        col: indent + 1,
    })
}

struct LineIndex {
    starts: Vec<usize>,
    ends: Vec<usize>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let bytes = source.as_bytes();
        let mut starts = Vec::new();
        let mut ends = Vec::new();
        let mut start = 0usize;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            let mut end = index;
            if end > start && bytes.get(end.saturating_sub(1)) == Some(&b'\r') {
                end = end.saturating_sub(1);
            }
            starts.push(start);
            ends.push(end);
            start = index.saturating_add(1);
        }
        if start < source.len() || starts.is_empty() {
            starts.push(start);
            ends.push(source.len());
        }
        Self { starts, ends }
    }

    fn line_count(&self) -> usize {
        self.starts.len()
    }

    fn start(&self, line: usize) -> Option<usize> {
        line.checked_sub(1)
            .and_then(|index| self.starts.get(index))
            .copied()
    }

    fn end(&self, line: usize) -> Option<usize> {
        line.checked_sub(1)
            .and_then(|index| self.ends.get(index))
            .copied()
    }

    fn text<'a>(&self, source: &'a str, line: usize) -> Option<&'a str> {
        source.get(self.start(line)?..self.end(line)?)
    }

    fn next_start(&self, line: usize) -> Option<usize> {
        self.starts.get(line).copied()
    }
}

fn has_terminator(source: &str, line_end: usize) -> bool {
    let bytes = source.as_bytes();
    match bytes.get(line_end) {
        Some(b'\n') => true,
        Some(b'\r') => bytes.get(line_end.saturating_add(1)) == Some(&b'\n'),
        _ => false,
    }
}

fn char_to_byte(text: &str, char_index: usize) -> Option<usize> {
    if char_index == text.chars().count() {
        return Some(text.len());
    }
    text.char_indices().nth(char_index).map(|(index, _)| index)
}

fn replace_span(source: &str, span: &Span, replacement: &str) -> Result<String, SceneEditError> {
    let index = LineIndex::new(source);
    let text = index
        .text(source, span.line)
        .ok_or(SceneEditError::InvalidTable)?;
    let start_char = span
        .col
        .checked_sub(1)
        .ok_or(SceneEditError::InvalidTable)?;
    let end_char = start_char
        .checked_add(span.len)
        .ok_or(SceneEditError::InvalidTable)?;
    let start_byte = char_to_byte(text, start_char).ok_or(SceneEditError::InvalidTable)?;
    let end_byte = char_to_byte(text, end_char).ok_or(SceneEditError::InvalidTable)?;
    let line_start = index.start(span.line).ok_or(SceneEditError::InvalidTable)?;
    let prefix_end = line_start
        .checked_add(start_byte)
        .ok_or(SceneEditError::InvalidTable)?;
    let suffix_start = line_start
        .checked_add(end_byte)
        .ok_or(SceneEditError::InvalidTable)?;
    let mut out = String::with_capacity(
        source
            .len()
            .saturating_sub(end_byte.saturating_sub(start_byte))
            .saturating_add(replacement.len()),
    );
    out.push_str(
        source
            .get(..prefix_end)
            .ok_or(SceneEditError::InvalidTable)?,
    );
    out.push_str(replacement);
    out.push_str(
        source
            .get(suffix_start..)
            .ok_or(SceneEditError::InvalidTable)?,
    );
    Ok(out)
}

fn insert_at(source: &str, at: usize, text: &str) -> Result<String, SceneEditError> {
    let prefix = source.get(..at).ok_or(SceneEditError::InvalidTable)?;
    let suffix = source.get(at..).ok_or(SceneEditError::InvalidTable)?;
    let mut out = String::with_capacity(source.len().saturating_add(text.len()));
    out.push_str(prefix);
    out.push_str(text);
    out.push_str(suffix);
    Ok(out)
}

fn remove_range(source: &str, from: usize, to: usize) -> Result<String, SceneEditError> {
    let prefix = source.get(..from).ok_or(SceneEditError::InvalidTable)?;
    let suffix = source.get(to..).ok_or(SceneEditError::InvalidTable)?;
    let mut out = String::with_capacity(source.len().saturating_sub(to.saturating_sub(from)));
    out.push_str(prefix);
    out.push_str(suffix);
    Ok(out)
}

fn append_cell(source: &str, line: usize, cell: &str) -> Result<String, SceneEditError> {
    let index = LineIndex::new(source);
    let text = index
        .text(source, line)
        .ok_or(SceneEditError::InvalidTable)?;
    let trimmed_len = text.trim_end().len();
    let pipe = trimmed_len
        .checked_sub(1)
        .ok_or(SceneEditError::InvalidTable)?;
    if text.as_bytes().get(pipe) != Some(&b'|') {
        return Err(SceneEditError::InvalidTable);
    }
    let at = index
        .start(line)
        .and_then(|start| start.checked_add(pipe))
        .ok_or(SceneEditError::InvalidTable)?;
    insert_at(source, at, cell)
}

fn newline(source: &str) -> &'static str {
    if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn append_row(source: &str, after_line: usize, row: &str) -> Result<String, SceneEditError> {
    let index = LineIndex::new(source);
    let line_end = index.end(after_line).ok_or(SceneEditError::InvalidTable)?;
    let nl = newline(source);
    if let Some(next) = index.next_start(after_line) {
        return insert_at(source, next, &format!("{row}{nl}"));
    }
    if has_terminator(source, line_end) {
        return insert_at(source, source.len(), &format!("{row}{nl}"));
    }
    insert_at(source, source.len(), &format!("{nl}{row}"))
}

fn remove_line(source: &str, line: usize) -> Result<String, SceneEditError> {
    let index = LineIndex::new(source);
    let start = index.start(line).ok_or(SceneEditError::InvalidTable)?;
    let end = index.end(line).ok_or(SceneEditError::InvalidTable)?;
    if let Some(next) = index.next_start(line) {
        return remove_range(source, start, next);
    }
    if has_terminator(source, end) || start == 0 {
        return remove_range(source, start, source.len());
    }
    let mut from = start.saturating_sub(1);
    if source.as_bytes().get(from) != Some(&b'\n') {
        from = start;
    }
    if from > 0 && source.as_bytes().get(from.saturating_sub(1)) == Some(&b'\r') {
        from = from.saturating_sub(1);
    }
    remove_range(source, from, source.len())
}

fn revision_of(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn unified_diff(path: &Path, old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines.get(prefix) == new_lines.get(prefix)
    {
        prefix = prefix.saturating_add(1);
    }
    let mut suffix = 0usize;
    while suffix < old_lines.len().saturating_sub(prefix)
        && suffix < new_lines.len().saturating_sub(prefix)
        && old_lines.get(old_lines.len().saturating_sub(suffix).saturating_sub(1))
            == new_lines.get(new_lines.len().saturating_sub(suffix).saturating_sub(1))
    {
        suffix = suffix.saturating_add(1);
    }

    let old_end = old_lines.len().saturating_sub(suffix);
    let new_end = new_lines.len().saturating_sub(suffix);
    if prefix == old_end && prefix == new_end {
        return String::new();
    }

    const CONTEXT: usize = 3;
    let start = prefix.saturating_sub(CONTEXT);
    let old_stop = old_end.saturating_add(CONTEXT).min(old_lines.len());
    let new_stop = new_end.saturating_add(CONTEXT).min(new_lines.len());

    let mut out = String::new();
    let _ = writeln!(out, "--- a/{}", path.display());
    let _ = writeln!(out, "+++ b/{}", path.display());
    let _ = writeln!(
        out,
        "@@ -{},{} +{},{} @@",
        start.saturating_add(1),
        old_stop.saturating_sub(start),
        start.saturating_add(1),
        new_stop.saturating_sub(start)
    );
    if let Some(lines) = old_lines.get(start..prefix) {
        for line in lines {
            let _ = writeln!(out, " {line}");
        }
    }
    if let Some(lines) = old_lines.get(prefix..old_end) {
        for line in lines {
            let _ = writeln!(out, "-{line}");
        }
    }
    if let Some(lines) = new_lines.get(prefix..new_end) {
        for line in lines {
            let _ = writeln!(out, "+{line}");
        }
    }
    if let Some(lines) = new_lines.get(new_end..new_stop) {
        for line in lines {
            let _ = writeln!(out, " {line}");
        }
    }
    out
}

fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("scene");
    let mut attempt = 0u32;
    loop {
        let temporary = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), attempt));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => {
                let result = write_and_rename(file, &temporary, path, contents);
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                return result;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt = attempt.saturating_add(1);
            }
            Err(error) => return Err(error),
        }
    }
}

fn write_and_rename(
    mut file: fs::File,
    temporary: &Path,
    path: &Path,
    contents: &str,
) -> io::Result<()> {
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)
}

fn resolve_in_workspace(root: &Path, path: &Path) -> Result<PathBuf, SceneEditError> {
    let root = fs::canonicalize(root).map_err(|source| SceneEditError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let candidate = lexical_normalize(&candidate);
    let resolved = canonical_target(&candidate).map_err(|source| SceneEditError::Io {
        path: candidate.clone(),
        source,
    })?;
    if !resolved.starts_with(&root) {
        return Err(SceneEditError::PathOutsideWorkspace {
            path: candidate,
            root,
        });
    }
    Ok(resolved)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn canonical_target(path: &Path) -> io::Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path);
    }
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    loop {
        if current.exists() {
            let mut out = fs::canonicalize(current)?;
            for name in missing.iter().rev() {
                out.push(name);
            }
            return Ok(out);
        }
        let name = current.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "path has no existing ancestor")
        })?;
        missing.push(name.to_os_string());
        current = current.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "path has no existing ancestor")
        })?;
    }
}

/// Render a list of diagnostics as one line each (MCP/HTTP friendly).
#[must_use]
pub fn diagnostics_summary(diagnostics: &Diagnostics) -> Vec<String> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let severity = match diagnostic.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            match &diagnostic.span {
                Some(span) => format!(
                    "{severity} {} at {}:{}:{}: {}",
                    diagnostic.code,
                    span.file.display(),
                    span.line,
                    span.col,
                    diagnostic.message
                ),
                None => format!("{severity} {}: {}", diagnostic.code, diagnostic.message),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_source() -> &'static str {
        "Scene: LoginScreen\n\
         // keep this comment\n\
         Platforms: cross-platform\n\
         \n\
         | Piece | web | android |\n\
         |-------|-----|---------|\n\
         | Title | h1.title | - |\n\
         | Submit | button[type='submit'] | - |\n\
         // trailing comment\n"
    }

    fn write_scene(dir: &tempfile::TempDir) -> PathBuf {
        let path = dir.path().join("LoginScreen.scene");
        fs::write(&path, scene_source()).expect("write scene");
        path
    }

    #[test]
    fn fill_cell_preserves_comments_and_untouched_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let mut document = SceneDocument::open(&path).expect("open");
        let before = document.revision().to_string();

        let edit = document
            .fill_cell("Title", "android", "role=heading[name=\"Welcome\"]")
            .expect("fill");
        assert_ne!(edit.revision(), before.as_str());
        assert!(edit.diff().contains("-| Title | h1.title | - |"));
        assert!(edit
            .diff()
            .contains("+| Title | h1.title | role=heading[name=\"Welcome\"] |"));
        assert!(edit.source().contains("// keep this comment"));
        assert!(edit.source().contains("// trailing comment"));
        assert_eq!(
            edit.scene()
                .piece("Title")
                .and_then(|piece| piece.selectors.iter().find(|s| s.key == Platform::Android))
                .map(|selector| selector.raw.as_str()),
            Some("role=heading[name=\"Welcome\"]")
        );

        let commit = document.commit(&edit).expect("commit");
        assert_eq!(commit.revision, edit.revision());
        assert_eq!(fs::read_to_string(&path).expect("read"), edit.source());
        assert_eq!(document.base_revision(), Some(edit.revision()));
    }

    #[test]
    fn fill_cell_refuses_to_overwrite_an_occupied_cell() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let document = SceneDocument::open(&path).expect("open");
        let error = document
            .fill_cell("Title", "web", "role=heading")
            .expect_err("occupied");
        assert_eq!(error.code(), "SCENE_CELL_OCCUPIED");
    }

    #[test]
    fn replace_cell_requires_confirmation_and_returns_the_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let document = SceneDocument::open(&path).expect("open");

        let error = document
            .replace_cell("Title", "web", "role=heading", false)
            .expect_err("confirmation");
        let diff = match error {
            SceneEditError::ConfirmationRequired { diff } => diff,
            other => panic!("unexpected error: {other:?}"),
        };
        assert!(diff.contains("-| Title | h1.title | - |"));

        let edit = document
            .replace_cell("Title", "web", "role=heading", true)
            .expect("replace");
        assert!(edit.source().contains("| Title | role=heading | - |"));
    }

    #[test]
    fn add_element_appends_a_row_in_the_requested_column() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let document = SceneDocument::open(&path).expect("open");

        let edit = document
            .add_element("LoginButton", "android", "role=button[name=\"Log in\"]")
            .expect("add element");
        assert!(edit
            .source()
            .contains("| LoginButton | - | role=button[name=\"Log in\"] |\n"));
        assert_eq!(edit.scene().pieces.len(), 3);
        assert!(edit.is_valid());
    }

    #[test]
    fn add_element_escapes_pipes_and_backslashes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let document = SceneDocument::open(&path).expect("open");
        let locator = "role=button[name=\"A|B\\\\C\"]";
        let edit = document
            .add_element("Odd", "web", locator)
            .expect("add element");
        assert!(edit.source().contains("A\\|B\\\\\\\\C"));
        assert_eq!(
            edit.scene()
                .piece("Odd")
                .and_then(|piece| piece.selectors.first())
                .map(|selector| selector.raw.as_str()),
            Some(locator)
        );
    }

    #[test]
    fn add_column_updates_header_separator_and_every_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let document = SceneDocument::open(&path).expect("open");

        let edit = document.add_column(Platform::WebOs).expect("add column");
        assert!(edit
            .source()
            .contains("| Piece | web | android | webos |\n"));
        assert!(edit
            .source()
            .contains("|-------|-----|---------|-------|\n"));
        assert!(edit.source().contains("| Title | h1.title | - | - |\n"));
        assert_eq!(
            edit.scene()
                .columns
                .iter()
                .filter_map(|column| column.platform)
                .collect::<Vec<_>>(),
            vec![Platform::Web, Platform::Android, Platform::WebOs]
        );

        let document = SceneDocument::open(&path).expect("reopen");
        let edit = document.add_column(Platform::WebOs).expect("add column");
        let document = SceneDocument {
            source: edit.source().to_string(),
            revision: edit.revision().to_string(),
            scene: edit.scene().clone(),
            diagnostics: edit.diagnostics().clone(),
            ..document
        };
        let filled = document
            .fill_cell("Title", "webos", "role=heading")
            .expect("fill new column");
        assert!(filled
            .source()
            .contains("| Title | h1.title | - | role=heading |"));
    }

    #[test]
    fn rename_and_remove_rows_keep_the_rest_of_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let document = SceneDocument::open(&path).expect("open");

        let renamed = document
            .rename_row("Title", "WelcomeTitle")
            .expect("rename");
        assert!(renamed.source().contains("| WelcomeTitle | h1.title | - |"));
        assert_eq!(
            renamed
                .scene()
                .piece("WelcomeTitle")
                .map(|piece| piece.name.as_str()),
            Some("WelcomeTitle")
        );

        let removed = document.remove_row("Title").expect("remove");
        assert!(!removed.source().contains("| Title |"));
        assert!(removed.source().contains("// keep this comment"));
        assert_eq!(removed.scene().pieces.len(), 1);
    }

    #[test]
    fn create_refuses_overwrite_and_commits_a_new_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("NewScene.scene");
        let mut document =
            SceneDocument::create(&path, "NewScene", &[Platform::Web]).expect("create");
        assert!(!path.exists());
        assert!(!document.is_valid());

        let edit = document
            .add_element("Title", "web", "role=heading")
            .expect("add element");
        assert!(edit.is_valid());
        let commit = document.commit(&edit).expect("commit");
        assert!(path.exists());
        assert_eq!(commit.revision, edit.revision());

        let error =
            SceneDocument::create(&path, "NewScene", &[Platform::Web]).expect_err("overwrite");
        assert_eq!(error.code(), "SCENE_ALREADY_EXISTS");
    }

    #[test]
    fn commit_conflict_returns_the_fresh_source_and_never_overwrites() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let mut document = SceneDocument::open(&path).expect("open");
        let edit = document
            .fill_cell("Title", "android", "role=heading")
            .expect("fill");

        let external = scene_source().replace("h1.title", "h2.external");
        fs::write(&path, &external).expect("external write");

        let error = document.commit(&edit).expect_err("conflict");
        let (expected, actual, fresh) = match error {
            SceneEditError::Conflict {
                expected,
                actual,
                fresh_source,
            } => (expected, actual, fresh_source),
            other => panic!("unexpected error: {other:?}"),
        };
        assert_eq!(
            expected.as_deref(),
            Some(edit.base_revision().unwrap_or_default())
        );
        assert_eq!(actual, Some(revision_of(&external)));
        assert_eq!(fresh.as_deref(), Some(external.as_str()));
        assert_eq!(fs::read_to_string(&path).expect("read"), external);

        assert!(document.refresh().expect("refresh"));
        let edit = document
            .fill_cell("Title", "android", "role=heading")
            .expect("fill after refresh");
        document.commit(&edit).expect("commit after refresh");
    }

    #[test]
    fn workspace_paths_outside_the_root_are_rejected() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let path = outside.path().join("Escape.scene");
        fs::write(&path, scene_source().replace("LoginScreen", "Escape")).expect("write");
        let error = SceneDocument::open_in_workspace(root.path(), &path).expect_err("outside");
        assert_eq!(error.code(), "SCENE_PATH_OUTSIDE_WORKSPACE");

        let inside = root.path().join("Inside.scene");
        fs::write(&inside, scene_source().replace("LoginScreen", "Inside")).expect("write");
        let document =
            SceneDocument::open_in_workspace(root.path(), "Inside.scene").expect("inside relative");
        assert_eq!(document.scene().name, "Inside");
    }

    #[test]
    fn crlf_line_endings_survive_fill_and_append() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("Crlf.scene");
        let source = scene_source().replace('\n', "\r\n");
        fs::write(&path, &source).expect("write");
        let document = SceneDocument::open(&path).expect("open");

        let edit = document
            .fill_cell("Title", "android", "role=heading")
            .expect("fill");
        assert_eq!(
            edit.source().matches("\r\n").count(),
            edit.source().matches('\n').count()
        );

        let edit = document
            .add_element("Next", "web", "role=link")
            .expect("append");
        assert_eq!(
            edit.source().matches("\r\n").count(),
            edit.source().matches('\n').count()
        );
        assert!(edit.source().contains("| Next | role=link | - |\r\n"));
    }

    #[test]
    fn remove_row_handles_a_final_line_without_a_newline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("NoNewline.scene");
        let source = "Scene: NoNewline\n| Piece | web |\n|-------|-----|\n| Only | h1 |";
        fs::write(&path, source).expect("write");
        let document = SceneDocument::open(&path).expect("open");
        let edit = document.remove_row("Only").expect("remove");
        assert_eq!(
            edit.source(),
            "Scene: NoNewline\n| Piece | web |\n|-------|-----|"
        );
    }

    #[test]
    fn unknown_column_and_row_report_available_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let document = SceneDocument::open(&path).expect("open");

        let error = document
            .fill_cell("Title", "ios", "role=heading")
            .expect_err("unknown column");
        match error {
            SceneEditError::UnknownColumn { column, available } => {
                assert_eq!(column, "ios");
                assert_eq!(available, vec!["web".to_string(), "android".to_string()]);
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let error = document
            .fill_cell("Missing", "web", "role=heading")
            .expect_err("unknown row");
        assert_eq!(error.code(), "SCENE_UNKNOWN_ROW");
    }

    #[test]
    fn suggested_names_are_unique_and_dsl_safe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_scene(&dir);
        let document = SceneDocument::open(&path).expect("open");
        assert_eq!(document.suggest_row_name("login-button"), "LoginButton");
        assert_eq!(document.suggest_row_name("title"), "Title2");
        assert_eq!(document.suggest_row_name("123 go"), "Element123Go");
        assert_eq!(document.suggest_row_name("***"), "Element");
    }

    #[test]
    fn validation_reports_parse_and_identity_diagnostics() {
        let diagnostics = validate_source("Scene: LoginScreen\n", Path::new("Other.scene"));
        assert!(diagnostics.has_errors());
        let summary = diagnostics_summary(&diagnostics);
        assert!(!summary.is_empty());
    }
}
