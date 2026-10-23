//! Span ⇄ LSP position conversion and the negotiated position encoding
//! (spec section 13).
//!
//! Core spans are 1-based `line:column` counted in Unicode scalar values.
//! LSP positions are 0-based `line:character` counted in the encoding that was
//! negotiated at `initialize`: UTF-8 when the client offers it, else UTF-16.
//! Nothing here assumes bytes.

use impresario_core::span::Span;
use tower_lsp_server::ls_types::{Position, PositionEncodingKind, Range};

/// The position encoding of one server session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// Character offsets count UTF-8 code units.
    Utf8,
    /// Character offsets count UTF-16 code units (the LSP default).
    #[default]
    Utf16,
}

impl Encoding {
    /// The capability value reported in `initialize`.
    #[must_use]
    pub fn kind(self) -> PositionEncodingKind {
        match self {
            Encoding::Utf8 => PositionEncodingKind::UTF8,
            Encoding::Utf16 => PositionEncodingKind::UTF16,
        }
    }

    fn units(self, ch: char) -> u32 {
        let units = match self {
            Encoding::Utf8 => ch.len_utf8(),
            Encoding::Utf16 => ch.len_utf16(),
        };
        u32::try_from(units).unwrap_or(u32::MAX)
    }
}

/// Negotiate the position encoding: UTF-8 when the client offers it, else
/// UTF-16 (spec section 13).
#[must_use]
pub fn negotiate(offered: &[PositionEncodingKind]) -> Encoding {
    let utf8 = offered
        .iter()
        .any(|kind| serde_json::to_value(kind).is_ok_and(|value| value.as_str() == Some("utf-8")));
    if utf8 {
        Encoding::Utf8
    } else {
        Encoding::Utf16
    }
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn units_up_to(enc: Encoding, chars: &[char], up_to: usize) -> u32 {
    chars
        .iter()
        .take(up_to)
        .fold(0u32, |acc, ch| acc.saturating_add(enc.units(*ch)))
}

/// One document's text with a line table.
///
/// The LSP layer keeps documents as full text (the server advertises full
/// sync), so every conversion is a cheap scan of one line.
pub struct Doc<'a> {
    lines: Vec<&'a str>,
}

impl<'a> Doc<'a> {
    /// Build the line table of `text`.
    #[must_use]
    pub fn new(text: &'a str) -> Self {
        Self {
            lines: text.lines().collect(),
        }
    }

    /// The text of a 1-based line.
    #[must_use]
    pub fn line(&self, line1: usize) -> Option<&'a str> {
        self.lines.get(line1.checked_sub(1)?).copied()
    }

    /// The LSP position of a 1-based `line` and a 1-based scalar `col`.
    ///
    /// Positions past the end of the text clamp to the end (the index may be
    /// one edit behind the buffer).
    #[must_use]
    pub fn position(&self, enc: Encoding, line: usize, col: usize) -> Position {
        let text = self.line(line).unwrap_or("");
        let chars: Vec<char> = text.chars().collect();
        let index = col.saturating_sub(1).min(chars.len());
        Position::new(
            to_u32(line.saturating_sub(1)),
            units_up_to(enc, &chars, index),
        )
    }

    /// The LSP range of a core [`Span`].
    #[must_use]
    pub fn range(&self, enc: Encoding, span: &Span) -> Range {
        let start = self.position(enc, span.line, span.col);
        let end = self.position(enc, span.line, span.col.saturating_add(span.len));
        Range::new(start, end)
    }

    /// Convert an LSP position to `(1-based line, 0-based scalar index)`
    /// within this document. Returns `None` for lines past the end.
    #[must_use]
    pub fn scalar_index(&self, enc: Encoding, pos: Position) -> Option<(usize, usize)> {
        let line1 = usize::try_from(pos.line).ok()?.checked_add(1)?;
        let text = self.line(line1)?;
        let chars: Vec<char> = text.chars().collect();
        let mut units = 0u32;
        for (index, ch) in chars.iter().enumerate() {
            if units >= pos.character {
                return Some((line1, index));
            }
            units = units.saturating_add(enc.units(*ch));
        }
        Some((line1, chars.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "P: \"😀😀\" err";

    #[test]
    fn utf16_offsets_count_surrogate_pairs() {
        let doc = Doc::new(LINE);
        // "err" starts at scalar index 8; each emoji adds 2 UTF-16 units.
        let range = doc.range(Encoding::Utf16, &Span::new("p.play", 1, 9, 3));
        assert_eq!(range.start, Position::new(0, 10));
        assert_eq!(range.end, Position::new(0, 13));
    }

    #[test]
    fn utf8_offsets_count_bytes() {
        let doc = Doc::new(LINE);
        let range = doc.range(Encoding::Utf8, &Span::new("p.play", 1, 9, 3));
        assert_eq!(range.start, Position::new(0, 14));
        assert_eq!(range.end, Position::new(0, 17));
    }

    #[test]
    fn scalar_index_round_trips_and_clamps() {
        let doc = Doc::new(LINE);
        assert_eq!(
            doc.scalar_index(Encoding::Utf16, Position::new(0, 12)),
            Some((1, 10))
        );
        assert_eq!(
            doc.scalar_index(Encoding::Utf16, Position::new(0, 999)),
            Some((1, 11))
        );
        assert_eq!(doc.scalar_index(Encoding::Utf16, Position::new(9, 0)), None);
    }

    #[test]
    fn positions_past_the_end_clamp() {
        let doc = Doc::new("a\nb");
        let range = doc.range(Encoding::Utf16, &Span::new("x.scene", 4, 2, 5));
        assert_eq!(range.start, Position::new(3, 0));
        assert_eq!(range.end, Position::new(3, 0));
    }

    #[test]
    fn negotiate_prefers_utf8_then_falls_back() {
        assert_eq!(
            negotiate(&[PositionEncodingKind::UTF16, PositionEncodingKind::UTF8]),
            Encoding::Utf8
        );
        assert_eq!(negotiate(&[PositionEncodingKind::UTF16]), Encoding::Utf16);
        assert_eq!(negotiate(&[]), Encoding::Utf16);
    }
}
