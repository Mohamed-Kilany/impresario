//! Hand-written indentation scanner for `.scene` and `.play` files (D-05).
//!
//! Produces logical lines: line number, indentation depth, and the trimmed
//! content with the column of its first character. Comments (`//`) and blank
//! lines are dropped. Indentation rules (spec 5.1):
//! * spaces only; a tab in indentation is `IMP-DSL-001`;
//! * the first indented line fixes the indent unit (`IMP-DSL-002`).

use std::path::Path;

use crate::diag::{Code, Diagnostic};
use crate::span::Span;

/// One logical line of a DSL file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalLine {
    /// 1-based physical line number in the file.
    pub line: usize,
    /// Indentation depth in units (0 = top level).
    pub depth: usize,
    /// The content, trailing whitespace removed, leading indentation removed.
    pub content: String,
    /// 1-based column of the first content character.
    pub col: usize,
}

impl LogicalLine {
    /// Span covering the content of this line.
    #[must_use]
    pub fn span(&self, file: &Path) -> Span {
        Span::new(file, self.line, self.col, self.content.chars().count())
    }

    /// Span of `needle` within the content, when present.
    #[must_use]
    pub fn span_of(&self, file: &Path, needle: &str) -> Option<Span> {
        let byte_index = self.content.find(needle)?;
        let char_index = self
            .content
            .char_indices()
            .take_while(|(idx, _)| *idx < byte_index)
            .count();
        Some(Span::new(
            file,
            self.line,
            self.col + char_index,
            needle.chars().count(),
        ))
    }
}

/// The scanned file: logical lines plus the discovered indent unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scanned {
    pub lines: Vec<LogicalLine>,
    pub indent_unit: usize,
    pub diagnostics: Vec<Diagnostic>,
}

fn indentation_of(raw: &str) -> (usize, bool) {
    let mut spaces = 0usize;
    let mut has_tab = false;
    for c in raw.chars() {
        match c {
            ' ' => spaces += 1,
            '\t' => {
                // count as moving to the next multiple of 4 for recovery
                spaces = spaces + (4 - spaces % 4);
                has_tab = true;
            }
            _ => break,
        }
    }
    (spaces, has_tab)
}

/// Scan source text into logical lines.
pub fn scan(source: &str, file: &Path) -> Scanned {
    // UTF-8 BOM is tolerated and stripped.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);

    let mut diagnostics = Vec::new();
    let mut lines = Vec::new();
    let mut indent_unit: Option<usize> = None;

    for (index, raw) in source.split('\n').enumerate() {
        let line_no = index + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if raw.trim().is_empty() {
            continue;
        }
        let (indent, has_tab) = indentation_of(raw);
        let content_start = raw
            .char_indices()
            .find(|(_, c)| *c != ' ' && *c != '\t')
            .map(|(idx, _)| idx)
            .unwrap_or(raw.len());
        let content = raw
            .get(content_start..)
            .unwrap_or("")
            .trim_end()
            .to_string();
        let col = indent + 1;

        if has_tab {
            diagnostics.push(
                Diagnostic::error(
                    Code::DslTabIndentation,
                    "tabs are not allowed in indentation; use spaces",
                )
                .with_span(Span::new(file, line_no, 1, indent + 1)),
            );
            continue;
        }

        if content.starts_with("//") {
            continue;
        }

        if indent > 0 {
            match indent_unit {
                None => indent_unit = Some(indent),
                Some(unit) if indent % unit != 0 => {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::DslInconsistentIndent,
                            format!(
                                "indentation of {indent} spaces is not a multiple of the {unit}-space indent unit"
                            ),
                        )
                        .with_span(Span::new(file, line_no, 1, indent)),
                    );
                    continue;
                }
                Some(_) => {}
            }
        }

        let unit = indent_unit.unwrap_or(1).max(1);
        lines.push(LogicalLine {
            line: line_no,
            depth: indent / unit,
            content,
            col,
        });
    }

    Scanned {
        lines,
        indent_unit: indent_unit.unwrap_or(0),
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn file() -> &'static Path {
        Path::new("x.scene")
    }

    #[test]
    fn strips_bom_and_crlf() {
        let src = "\u{feff}Scene: A\r\nPlatform: web\r\n";
        let scanned = scan(src, file());
        assert_eq!(scanned.lines.len(), 2);
        assert_eq!(
            scanned.lines.first().map(|l| l.content.as_str()),
            Some("Scene: A")
        );
        assert!(scanned.diagnostics.is_empty());
    }

    #[test]
    fn computes_depth_from_first_indent() {
        let src = "Scene: A\nPlatform: web\nPieces:\n    One: web=\"#a\"\n";
        let scanned = scan(src, file());
        assert_eq!(scanned.indent_unit, 4);
        assert_eq!(
            scanned.lines.iter().map(|l| l.depth).collect::<Vec<_>>(),
            vec![0, 0, 0, 1]
        );
        let piece = scanned.lines.get(3).expect("piece line");
        assert_eq!(piece.col, 5);
        assert_eq!(piece.content, "One: web=\"#a\"");
    }

    #[test]
    fn tab_indentation_is_an_error() {
        let src = "Scene: A\n\tPieces:\n";
        let scanned = scan(src, file());
        assert_eq!(scanned.diagnostics.len(), 1);
        assert_eq!(
            scanned.diagnostics.first().map(|d| d.code),
            Some(Code::DslTabIndentation)
        );
    }

    #[test]
    fn inconsistent_indent_is_an_error() {
        let src = "Scene: A\n    Pieces:\n  Bad: web=\"#a\"\n";
        let scanned = scan(src, file());
        assert_eq!(
            scanned.diagnostics.first().map(|d| d.code),
            Some(Code::DslInconsistentIndent)
        );
    }

    #[test]
    fn comments_and_blanks_are_ignored() {
        let src = "// header comment\n\nScene: A\n   \n   // indented comment\nPlatform: web\n";
        let scanned = scan(src, file());
        assert_eq!(scanned.lines.len(), 2);
        assert!(scanned.diagnostics.is_empty());
    }

    #[test]
    fn span_of_finds_needle() {
        let src = "Scene: LoginScreen\n";
        let scanned = scan(src, file());
        let line = scanned.lines.first().expect("line");
        let span = line.span_of(file(), "LoginScreen").expect("span");
        assert_eq!(span.col, 8);
        assert_eq!(span.len, 11);
        assert_eq!(line.span_of(file(), "nope"), None);
    }
}
