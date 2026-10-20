//! Pipe-delimited tables used by scenes (`| Piece | web | ... |`) and by
//! `Examples:` blocks (TASK-2 section 8.2/8.4).
//!
//! A row is split on every unescaped `|`; a literal pipe is written `\|`.
//! Cells are trimmed. A cell that starts and ends with the same quote is
//! parsed with the normal string literal rules, otherwise the text is used
//! verbatim (CSS selectors such as `button[type='submit']` are therefore not
//! mistaken for quoted strings).

use std::path::Path;

use crate::diag::{Code, Diagnostic};
use crate::scanner::LogicalLine;
use crate::span::Span;
use crate::strings;

/// One parsed table cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// Trimmed, unescaped text. Empty/absent cells keep an empty string.
    pub text: String,
    pub span: Span,
    /// True when the cell is empty, `-`, `~`, `none` or `null`.
    pub absent: bool,
}

impl Cell {
    #[must_use]
    pub fn is_separator(&self) -> bool {
        let text = self.text.trim();
        !text.is_empty() && text.chars().all(|c| c == '-' || c == ':') && text.contains('-')
    }
}

/// Split one table row into cells. The line must start and end with `|`.
pub fn split_row(line: &LogicalLine, path: &Path) -> Result<Vec<Cell>, Diagnostic> {
    let content = line.content.as_str();
    if !content.starts_with('|') {
        return Err(
            Diagnostic::error(Code::DslUnknownKeyword, "a table row must start with '|'")
                .with_span(line.span(path)),
        );
    }
    if !ends_with_unescaped_pipe(content) {
        return Err(Diagnostic::error(
            Code::DslUnknownKeyword,
            "a table row must end with '|' (write a literal pipe as '\\|')",
        )
        .with_span(line.span(path)));
    }

    let chars: Vec<char> = content.chars().collect();
    let mut cells: Vec<Cell> = Vec::new();
    let mut start = 1usize; // skip the leading pipe
    let mut i = 1usize;
    while i < chars.len() {
        match chars.get(i) {
            Some('\\') => i += 2,
            Some('|') => {
                cells.push(cell_from(&chars, start, i, line, path));
                start = i + 1;
                i += 1;
            }
            Some(_) => i += 1,
            None => break,
        }
    }

    Ok(cells)
}

fn ends_with_unescaped_pipe(content: &str) -> bool {
    if !content.ends_with('|') {
        return false;
    }
    let before = content.get(..content.len().saturating_sub(1)).unwrap_or("");
    let backslashes = before.chars().rev().take_while(|c| *c == '\\').count();
    backslashes % 2 == 0
}

fn cell_from(chars: &[char], start: usize, end: usize, line: &LogicalLine, path: &Path) -> Cell {
    let raw: String = chars.get(start..end).unwrap_or(&[]).iter().collect();
    let leading = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    let base_col = line.col + start + raw.get(..leading).map_or(0, |s| s.chars().count());
    let len = trimmed.chars().count();
    let span = Span::new(path, line.line, base_col, len);

    if trimmed.is_empty() || is_absent(trimmed) {
        return Cell {
            text: String::new(),
            span,
            absent: true,
        };
    }

    let first = trimmed.chars().next().unwrap_or('\0');
    let last = trimmed.chars().next_back().unwrap_or('\0');
    if (first == '\'' || first == '"') && first == last {
        if let Ok((parsed, consumed)) = strings::parse_quoted(trimmed, path, line.line, base_col) {
            if consumed == trimmed.chars().count() {
                return Cell {
                    text: parsed.raw,
                    span: parsed.span,
                    absent: false,
                };
            }
        }
    }

    Cell {
        text: unescape_bare(trimmed),
        span,
        absent: false,
    }
}

fn is_absent(text: &str) -> bool {
    matches!(text, "-" | "~")
        || text.eq_ignore_ascii_case("none")
        || text.eq_ignore_ascii_case("null")
}

/// Bare cells only need `\|` (and `\\` for a backslash); all other text is
/// literal, so CSS attribute selectors survive untouched.
fn unescape_bare(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('|') => out.push('|'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The span from the first to the last cell, for a whole table.
#[must_use]
pub fn table_span(first: &[Cell], last: &[Cell], path: &Path) -> Span {
    let Some(first) = first.first() else {
        return Span::new(path, 1, 1, 1);
    };
    let Some(last) = last.last() else {
        return first.span.clone();
    };
    let end_col = if last.span.line == first.span.line {
        last.span.col.saturating_sub(first.span.col) + last.span.len
    } else {
        first.span.len
    };
    Span::new(path, first.span.line, first.span.col, end_col.max(1))
}
