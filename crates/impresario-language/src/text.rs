//! Scalar (Unicode code point) positions used by the editor service.
//!
//! Core spans are 1-based lines and columns counted in Unicode scalar values.
//! LSP clients use UTF-16 or UTF-8 code units, and JavaScript strings use
//! UTF-16, so every front end converts at its boundary with the helpers here.

use impresario_core::span::Span;

/// A 1-based line and scalar column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Position {
    pub line: u32,
    pub col: u32,
}

impl Position {
    #[must_use]
    pub const fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }
}

impl std::ops::Add for Position {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            line: self.line.saturating_add(other.line),
            col: self.col.saturating_add(other.col),
        }
    }
}

/// Position conversion helpers for one immutable source text.
pub struct Doc<'a> {
    pub text: &'a str,
    /// Lines without their trailing `\n`.
    pub lines: Vec<&'a str>,
}

impl<'a> Doc<'a> {
    #[must_use]
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            lines: text.split('\n').collect(),
        }
    }

    /// The text of a 1-based line, without `\r`.
    #[must_use]
    pub fn line(&self, line: u32) -> &'a str {
        let index = line.saturating_sub(1) as usize;
        self.lines
            .get(index)
            .copied()
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .unwrap_or("")
    }

    /// How many scalar columns precede `utf16_col` on the given 0-based line.
    #[must_use]
    pub fn scalar_col(&self, line: u32, utf16_col: u32) -> u32 {
        let text = self.line(line);
        let mut units = 0u32;
        for (index, ch) in text.chars().enumerate() {
            if units >= utf16_col {
                return index as u32 + 1;
            }
            units = units.saturating_add(u32::try_from(ch.len_utf16()).unwrap_or(1));
        }
        u32::try_from(text.chars().count()).unwrap_or(u32::MAX) + 1
    }

    /// How many UTF-16 code units precede a 1-based scalar column.
    #[must_use]
    pub fn utf16_col(&self, line: u32, scalar_col: u32) -> u32 {
        self.line(line)
            .chars()
            .take(scalar_col.saturating_sub(1) as usize)
            .map(|ch| u32::try_from(ch.len_utf16()).unwrap_or(1))
            .sum()
    }

    /// The identifier or interpolated reference around a position.
    #[must_use]
    pub fn word_at(&self, position: Position) -> &'a str {
        let line = self.line(position.line);
        let mut start = None;
        let mut end = 0;
        let target = position.col.saturating_sub(1);
        let mut scalar = 0u32;
        for (byte, ch) in line.char_indices() {
            let next = scalar + 1;
            if scalar <= target && target < next && is_word(ch) {
                start = Some(byte);
            }
            if start.is_some() {
                if is_word(ch) {
                    end = byte + ch.len_utf8();
                } else {
                    break;
                }
            }
            scalar = next;
        }
        start.and_then(|start| line.get(start..end)).unwrap_or("")
    }

    /// The indentation depth in four-space units of a 1-based line.
    #[must_use]
    pub fn indent(&self, line: u32) -> usize {
        self.line(line).chars().take_while(|ch| *ch == ' ').count() / 4
    }

    /// The 1-based line and scalar column of a byte offset.
    #[must_use]
    pub fn position_of_offset(&self, offset: usize) -> Position {
        let mut line = 1u32;
        let mut col = 1u32;
        for (index, ch) in self.text.char_indices() {
            if index >= offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        Position::new(line, col)
    }

    /// The span of a 1-based physical line, including its indentation.
    #[must_use]
    pub fn line_span(&self, path: &std::path::Path, line: u32) -> Span {
        let text = self.line(line);
        Span::new(path, line as usize, 1, text.chars().count())
    }

    /// The last 1-based line with content at or below `start`.
    #[must_use]
    pub fn end_line(&self) -> u32 {
        u32::try_from(self.lines.len()).unwrap_or(u32::MAX)
    }

    /// The span from the start of `start_line` through the end of `end_line`.
    #[must_use]
    pub fn block_span(&self, path: &std::path::Path, start_line: u32, end_line: u32) -> Span {
        let start = start_line.max(1);
        let end = end_line.max(start);
        let length = (start..=end)
            .map(|line| self.line(line).chars().count())
            .sum::<usize>()
            + usize::try_from(end - start).unwrap_or(0);
        Span::new(path, start as usize, 1, length)
    }
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '.' | '-' | '/' | ':')
}
