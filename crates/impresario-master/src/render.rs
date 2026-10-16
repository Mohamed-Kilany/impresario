//! Rendering of core diagnostics with miette (spec section 5.7 and 14).
//!
//! Core diagnostics carry `Span { file, line, col, len }` in *characters*.
//! The CLI boundary is the only place that holds file contents, so it
//! resolves the span to a byte range, attaches the source and renders the
//! caret/hint output.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

use impresario_core::diag::{Diagnostic, Severity};
use impresario_core::span::Span;
use miette::{LabeledSpan, NamedSource, Report};

/// A diagnostic prepared for miette: source text + byte range attached.
pub struct Rendered {
    code: String,
    severity: Severity,
    message: String,
    help: Option<String>,
    source: Option<NamedSource<String>>,
    label: Option<(Range<usize>, String)>,
}

impl fmt::Display for Rendered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for Rendered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Rendered {}

impl miette::Diagnostic for Rendered {
    fn code(&self) -> Option<Box<dyn fmt::Display + '_>> {
        Some(Box::new(self.code.clone()))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(match self.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
        })
    }

    fn help(&self) -> Option<Box<dyn fmt::Display + '_>> {
        self.help
            .as_ref()
            .map(|help| Box::new(help.clone()) as Box<dyn fmt::Display>)
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.source
            .as_ref()
            .map(|source| source as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let (range, text) = self.label.clone()?;
        Some(Box::new(std::iter::once(LabeledSpan::new_with_span(
            Some(text),
            range,
        ))))
    }
}

/// Caches file contents for span resolution (several diagnostics share a file).
#[derive(Default)]
pub struct SourceCache {
    files: HashMap<PathBuf, Option<String>>,
}

impl SourceCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn source(&mut self, path: &Path) -> Option<&str> {
        let entry = self
            .files
            .entry(path.to_path_buf())
            .or_insert_with(|| std::fs::read_to_string(path).ok());
        entry.as_deref()
    }
}

/// Prepare one core diagnostic for miette, resolving its span and source.
pub fn prepare(diagnostic: &Diagnostic, cache: &mut SourceCache) -> Rendered {
    let mut rendered = Rendered {
        code: diagnostic.code.as_str().to_string(),
        severity: diagnostic.severity,
        message: format!("{}: {}", diagnostic.code.as_str(), diagnostic.message),
        help: diagnostic.help.clone(),
        source: None,
        label: None,
    };
    if let Some(span) = &diagnostic.span {
        if let Some(source) = cache.source(span.file()) {
            if let Some(range) = byte_range(source, span) {
                rendered.label = Some((range, diagnostic.message.clone()));
            }
            rendered.source = Some(NamedSource::new(
                span.file().display().to_string(),
                source.to_string(),
            ));
        }
    }
    rendered
}

/// Render one diagnostic to stderr with miette.
pub fn eprint(diagnostic: &Diagnostic, cache: &mut SourceCache) {
    let rendered = prepare(diagnostic, cache);
    eprintln!("{:?}", Report::new(rendered));
}

/// Print all diagnostics to stderr with miette.
pub fn eprint_all(diagnostics: &impresario_core::diag::Diagnostics) {
    let mut cache = SourceCache::new();
    for diagnostic in diagnostics.iter() {
        eprint(diagnostic, &mut cache);
    }
}

/// Byte range of a span within `source` (line/col/len count characters).
#[must_use]
pub fn byte_range(source: &str, span: &Span) -> Option<Range<usize>> {
    if span.line == 0 {
        return None;
    }
    let mut line_start = 0usize;
    let mut line = 1usize;
    while line < span.line {
        let newline = source.get(line_start..)?.find('\n')?;
        line_start = line_start.checked_add(newline)?.checked_add(1)?;
        line += 1;
    }
    let rest = source.get(line_start..)?;
    let line_end = rest
        .find('\n')
        .map_or(source.len(), |offset| line_start + offset);
    let line_text = source.get(line_start..line_end)?;
    let start_col = span.col.saturating_sub(1);
    let start_rel = char_offset(line_text, start_col)?;
    let end_rel =
        char_offset(line_text, start_col.checked_add(span.len)?).unwrap_or(line_text.len());
    let start = line_start.checked_add(start_rel)?;
    let end = line_start.checked_add(end_rel)?;
    Some(start..end)
}

/// Byte offset of the `n`-th character of `text` (`None` when past the end).
fn char_offset(text: &str, n: usize) -> Option<usize> {
    let mut count = 0usize;
    for (offset, _) in text.char_indices() {
        if count == n {
            return Some(offset);
        }
        count += 1;
    }
    if count == n {
        Some(text.len())
    } else {
        None
    }
}

/// Install a miette hook honoring `--no-color`.
pub fn install_hook(no_color: bool) {
    let hook = move |_e: &(dyn miette::Diagnostic + 'static)| -> Box<dyn miette::ReportHandler> {
        Box::new(miette::MietteHandlerOpts::new().color(!no_color).build())
    };
    // A second install (tests running in one process) is fine to ignore.
    let _ = miette::set_hook(Box::new(hook));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_range_ascii() {
        let source = "line one\nline two\n";
        let span = Span::new("x.scene", 2, 1, 4);
        assert_eq!(byte_range(source, &span), Some(9..13));
    }

    #[test]
    fn byte_range_unicode_columns() {
        let source = "héllo wörld\n";
        // col 7 is the 'w' (h,é,l,l,o,space,w) → byte 7 (é is two bytes)
        let span = Span::new("x.scene", 1, 7, 5);
        assert_eq!(byte_range(source, &span), Some(7..13));
    }

    #[test]
    fn byte_range_clamps_to_line_end() {
        let source = "abc\n";
        let span = Span::new("x.scene", 1, 2, 100);
        assert_eq!(byte_range(source, &span), Some(1..3));
    }

    #[test]
    fn byte_range_beyond_eof_is_none() {
        let source = "only one line";
        let span = Span::new("x.scene", 9, 1, 1);
        assert_eq!(byte_range(source, &span), None);
    }

    #[test]
    fn rendered_carries_code_severity_and_help() {
        use impresario_core::diag::Code;
        let diagnostic = Diagnostic::error(Code::DslUnknownScene, "unknown scene 'X'")
            .with_help("did you mean 'Y'?");
        let mut cache = SourceCache::new();
        let rendered = prepare(&diagnostic, &mut cache);
        assert_eq!(rendered.code, "IMP-DSL-008");
        assert_eq!(rendered.severity, Severity::Error);
        assert_eq!(rendered.help.as_deref(), Some("did you mean 'Y'?"));
        assert!(rendered.source.is_none());
    }
}
