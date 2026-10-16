//! Source spans and positions shared by every impresario diagnostic.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A half-open source region `[start, start+len)` within one file.
///
/// `line` and `col` are 1-based; `col` counts Unicode scalar values (not
/// bytes) so it can be converted to UTF-16 by clients (LSP) if needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Path of the file as the user wrote it (relative paths are preserved).
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column, in Unicode scalar values.
    pub col: usize,
    /// Length of the span in Unicode scalar values.
    pub len: usize,
}

impl Span {
    #[must_use]
    pub fn new(file: impl Into<PathBuf>, line: usize, col: usize, len: usize) -> Self {
        Self {
            file: file.into(),
            line,
            col,
            len,
        }
    }

    /// A zero-width span marking the start position of `other`.
    #[must_use]
    pub fn point_of(other: &Span) -> Self {
        Self {
            file: other.file.clone(),
            line: other.line,
            col: other.col,
            len: 0,
        }
    }

    #[must_use]
    pub fn file(&self) -> &Path {
        &self.file
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_display_is_file_line_col() {
        let span = Span::new("plays/login.play", 12, 9, 4);
        assert_eq!(span.to_string(), "plays/login.play:12:9");
    }

    #[test]
    fn point_of_copies_position_without_length() {
        let span = Span::new("a.scene", 3, 5, 11);
        let point = Span::point_of(&span);
        assert_eq!(point.line, 3);
        assert_eq!(point.col, 5);
        assert_eq!(point.len, 0);
    }
}
