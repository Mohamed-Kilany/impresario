//! Whole-document, range and on-type formatting.
//!
//! The formatter itself lives in `impresario-core::fmt`, so the LSP, the CLI
//! `impresario fmt` and the standalone extension all produce identical text.

use std::path::Path;

use impresario_core::fmt;
use impresario_core::span::Span;

use crate::service::{Language, TextEdit};
use crate::text::{Doc, Position};

impl Language {
    /// Format a whole buffer. The returned edit is `None` when the buffer is
    /// already canonical.
    pub fn format_document(&self, path: &Path) -> Result<Option<TextEdit>, String> {
        let source = self
            .source(path)
            .ok_or_else(|| format!("unknown document {}", path.display()))?;
        let formatted = fmt::format_source(source, path).map_err(|diagnostics| {
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.one_line())
                .collect::<Vec<_>>()
                .join("; ")
        })?;
        if formatted == source {
            return Ok(None);
        }
        Ok(Some(TextEdit {
            span: Span::new(path, 1, 1, source.chars().count()),
            new_text: formatted,
        }))
    }

    /// Format the lines touched by a range.
    ///
    /// The canonical formatter is line-oriented, so formatting the whole file
    /// and replacing exactly the selected lines keeps tables and indentation
    /// consistent with a full format while leaving the rest of the buffer
    /// untouched.
    pub fn format_range(
        &self,
        path: &Path,
        start: Position,
        end: Position,
    ) -> Result<Vec<TextEdit>, String> {
        let source = self
            .source(path)
            .ok_or_else(|| format!("unknown document {}", path.display()))?;
        let formatted = fmt::format_source(source, path).map_err(|diagnostics| {
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.one_line())
                .collect::<Vec<_>>()
                .join("; ")
        })?;
        if formatted == source {
            return Ok(Vec::new());
        }
        let original = Doc::new(source);
        let rendered = Doc::new(&formatted);
        let start_line = start.line.max(1);
        let end_line = end.line.max(start_line);
        if original.lines.len() != rendered.lines.len() {
            return Ok(vec![TextEdit {
                span: Span::new(path, 1, 1, source.chars().count()),
                new_text: formatted,
            }]);
        }
        let text = (start_line..=end_line)
            .map(|line| rendered.line(line))
            .collect::<Vec<_>>()
            .join("\n");
        let old = (start_line..=end_line)
            .map(|line| original.line(line))
            .collect::<Vec<_>>()
            .join("\n");
        if text == old {
            return Ok(Vec::new());
        }
        Ok(vec![TextEdit {
            span: block_span(path, &original, start_line, end_line),
            new_text: text,
        }])
    }

    /// On-type formatting, primarily table re-alignment after typing `|`.
    pub fn format_on_type(
        &self,
        path: &Path,
        position: Position,
        ch: char,
    ) -> Result<Vec<TextEdit>, String> {
        let source = self
            .source(path)
            .ok_or_else(|| format!("unknown document {}", path.display()))?;
        let formatted = match fmt::format_source(source, path) {
            Ok(formatted) => formatted,
            Err(_) => return Ok(Vec::new()),
        };
        if formatted == source {
            return Ok(Vec::new());
        }
        let original = Doc::new(source);
        let rendered = Doc::new(&formatted);
        if original.lines.len() != rendered.lines.len() {
            return Ok(Vec::new());
        }
        match ch {
            '|' | '\n' => {
                let (start, end) = if ch == '|' {
                    table_block(&original, position.line)
                } else {
                    (position.line.max(1), position.line.max(1))
                };
                let new_text = (start..=end)
                    .map(|line| rendered.line(line))
                    .collect::<Vec<_>>()
                    .join("\n");
                let old_text = (start..=end)
                    .map(|line| original.line(line))
                    .collect::<Vec<_>>()
                    .join("\n");
                if new_text == old_text {
                    return Ok(Vec::new());
                }
                Ok(vec![TextEdit {
                    span: block_span(path, &original, start, end),
                    new_text,
                }])
            }
            _ => Ok(Vec::new()),
        }
    }

    /// The canonical text of one document (used by the standalone test
    /// corpus and the preview command).
    #[must_use]
    pub fn formatted_text(&self, path: &Path) -> Option<String> {
        let source = self.source(path)?;
        fmt::format_source(source, path).ok()
    }
}

fn table_block(doc: &Doc<'_>, line: u32) -> (u32, u32) {
    let mut start = line.max(1);
    let mut end = line.max(1);
    while start > 1 && doc.line(start - 1).trim_start().starts_with('|') {
        start -= 1;
    }
    while end < doc.end_line() && doc.line(end + 1).trim_start().starts_with('|') {
        end += 1;
    }
    (start, end)
}

fn block_span(path: &Path, doc: &Doc<'_>, start: u32, end: u32) -> Span {
    let length = (start..=end)
        .map(|line| doc.line(line).chars().count())
        .sum::<usize>()
        + usize::try_from(end.saturating_sub(start)).unwrap_or(0);
    Span::new(path, start as usize, 1, length)
}
