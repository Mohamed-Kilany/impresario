//! String literals: quoting, escapes and interpolation.
//!
//! Grammar (spec 5.1): strings are single or double quoted; escapes are
//! `\\ \' \" \n \t \u{XXXX}`; a string never spans lines. Interpolation
//! happens inside strings only: `${env.NAME}` (process environment),
//! `${var.NAME}` (play variable), `${param.NAME}` (stage/act parameter) and
//! `${example.NAME}` (Examples column).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::diag::{Code, Diagnostic};
use crate::span::Span;

/// One piece of an interpolated string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Part {
    /// Literal text (already unescaped).
    Literal(String),
    /// `${env.NAME}` reference.
    Env { name: String, span: Span },
    /// `${var.NAME}` reference.
    Var { name: String, span: Span },
    /// `${param.NAME}` reference (stage or act parameter).
    Param { name: String, span: Span },
    /// `${example.NAME}` reference (Examples column).
    Example { name: String, span: Span },
}

/// A parsed string literal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedString {
    /// The unescaped literal text with `${...}` regions kept verbatim.
    pub raw: String,
    /// The interpolation breakdown of `raw`.
    pub parts: Vec<Part>,
    /// Span of the literal including its quotes.
    pub span: Span,
}

impl ParsedString {
    /// True when the string contains no interpolation.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        self.parts.iter().all(|p| matches!(p, Part::Literal(_)))
    }

    /// All variable names referenced by `${var.*}`.
    #[must_use]
    pub fn var_names(&self) -> Vec<&str> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Var { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    /// All environment names referenced by `${env.*}`.
    #[must_use]
    pub fn env_names(&self) -> Vec<&str> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Env { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    /// All stage/act parameter names referenced by `${param.*}`.
    #[must_use]
    pub fn param_names(&self) -> Vec<&str> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Param { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    /// All example-column names referenced by `${example.*}`.
    #[must_use]
    pub fn example_names(&self) -> Vec<&str> {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Example { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic()
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Parse a quoted string starting at `input` (which must begin with `'` or `"`).
///
/// Returns the parsed string and the number of *chars* consumed (including
/// both quotes). The `base_col` is the 1-based column of the opening quote.
pub fn parse_quoted(
    input: &str,
    file: &Path,
    line: usize,
    base_col: usize,
) -> Result<(ParsedString, usize), Diagnostic> {
    let chars: Vec<char> = input.chars().collect();
    let quote = match chars.first() {
        Some(c @ ('\'' | '"')) => *c,
        _ => {
            return Err(
                Diagnostic::error(Code::DslUnterminatedString, "expected a quoted string")
                    .with_span(Span::new(file, line, base_col, 1)),
            )
        }
    };

    let mut value = String::new();
    // Source column (1-based) of every character pushed to `value`.
    let mut source_cols: Vec<usize> = Vec::new();
    let mut i = 1usize; // skip the opening quote
    let mut closed = false;

    while let Some(&c) = chars.get(i) {
        let col_of_c = base_col + i;
        i += 1;
        if c == quote {
            closed = true;
            break;
        }
        if c == '\\' {
            let Some(&esc) = chars.get(i) else { break };
            let esc_col = base_col + i;
            i += 1;
            match esc {
                '\\' => {
                    value.push('\\');
                    source_cols.push(esc_col);
                }
                '\'' => {
                    value.push('\'');
                    source_cols.push(esc_col);
                }
                '"' => {
                    value.push('"');
                    source_cols.push(esc_col);
                }
                'n' => {
                    value.push('\n');
                    source_cols.push(esc_col);
                }
                't' => {
                    value.push('\t');
                    source_cols.push(esc_col);
                }
                'u' => {
                    let mut j = i;
                    let mut encoded = String::new();
                    let mut closed_brace = false;
                    if chars.get(j) == Some(&'{') {
                        j += 1;
                        scan_hex(&chars, &mut j, &mut encoded, &mut closed_brace);
                    }
                    if !closed_brace {
                        return Err(Diagnostic::error(
                            Code::DslUnterminatedString,
                            "unterminated \\u{...} escape",
                        )
                        .with_span(Span::new(file, line, base_col, j))
                        .with_help("write unicode escapes as \\u{XXXX}"));
                    }
                    i = j;
                    let codepoint = u32::from_str_radix(&encoded, 16)
                        .ok()
                        .and_then(char::from_u32);
                    match codepoint {
                        Some(ch) => {
                            value.push(ch);
                            source_cols.push(esc_col);
                        }
                        None => {
                            return Err(Diagnostic::error(
                                Code::DslUnterminatedString,
                                format!("invalid unicode escape \\u{{{encoded}}}"),
                            )
                            .with_span(Span::new(file, line, esc_col, encoded.len() + 4))
                            .with_help("\\u{...} takes 1 to 6 hex digits (e.g. \\u{1F600})"));
                        }
                    }
                }
                other => {
                    return Err(Diagnostic::error(
                        Code::DslUnterminatedString,
                        format!("invalid escape sequence '\\{other}'"),
                    )
                    .with_span(Span::new(file, line, esc_col, 2))
                    .with_help("valid escapes are \\\\ \\' \\\" \\n \\t \\u{XXXX}"));
                }
            }
            continue;
        }
        value.push(c);
        source_cols.push(col_of_c);
    }

    if !closed {
        return Err(
            Diagnostic::error(Code::DslUnterminatedString, "unterminated string literal")
                .with_span(Span::new(file, line, base_col, input.chars().count()))
                .with_help("strings are closed on the same line; check the quotes"),
        );
    }

    let span = Span::new(file, line, base_col, i);
    let parts = interpolation_parts(&value, &source_cols, file, line);
    Ok((
        ParsedString {
            raw: value,
            parts,
            span,
        },
        i,
    ))
}

fn scan_hex(chars: &[char], j: &mut usize, out: &mut String, closed: &mut bool) {
    loop {
        let Some(&ch) = chars.get(*j) else { return };
        *j += 1;
        if ch == '}' {
            *closed = true;
            return;
        }
        out.push(ch);
    }
}

/// Split unescaped text into literal and interpolation parts.
///
/// A `${` sequence that is not exactly `${env.IDENT}` or `${var.IDENT}` is
/// treated as literal text (documented in docs/DECISIONS.md).
fn interpolation_parts(text: &str, source_cols: &[usize], file: &Path, line: usize) -> Vec<Part> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts: Vec<Part> = Vec::new();
    let mut literal = String::new();
    let mut i = 0usize;
    let col_at = |index: usize| source_cols.get(index).copied().unwrap_or(1);
    while let Some(&c) = chars.get(i) {
        if c == '$' && chars.get(i + 1) == Some(&'{') {
            let mut j = i + 2;
            let mut inner = String::new();
            let mut terminated = false;
            while let Some(&ch) = chars.get(j) {
                if ch == '}' {
                    terminated = true;
                    break;
                }
                inner.push(ch);
                j += 1;
            }
            let parsed = if terminated {
                inner.split_once('.').and_then(|(kind, name)| {
                    let name_ok = name.chars().next().is_some_and(is_ident_start)
                        && name.chars().all(is_ident_char);
                    if !name_ok {
                        return None;
                    }
                    let span = Span::new(file, line, col_at(i), j - i + 1);
                    match kind {
                        "env" => Some(Part::Env {
                            name: name.to_string(),
                            span,
                        }),
                        "var" => Some(Part::Var {
                            name: name.to_string(),
                            span,
                        }),
                        "param" => Some(Part::Param {
                            name: name.to_string(),
                            span,
                        }),
                        "example" => Some(Part::Example {
                            name: name.to_string(),
                            span,
                        }),
                        _ => None,
                    }
                })
            } else {
                None
            };
            if let Some(part) = parsed {
                if !literal.is_empty() {
                    parts.push(Part::Literal(std::mem::take(&mut literal)));
                }
                parts.push(part);
                i = j + 1;
                continue;
            }
        }
        literal.push(c);
        i += 1;
    }
    if !literal.is_empty() || parts.is_empty() {
        parts.push(Part::Literal(literal));
    }
    parts
}

/// Validate free text (Play/Act/Stage names, descriptions):
/// 1..=200 chars with no leading/trailing whitespace.
#[must_use]
pub fn free_text_ok(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= 200 && text.trim() == text
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn file() -> &'static Path {
        Path::new("test.play")
    }

    #[test]
    fn parses_simple_double_quoted() {
        let (parsed, consumed) = parse_quoted("\"hello\" rest", file(), 1, 1).expect("ok");
        assert_eq!(parsed.raw, "hello");
        assert_eq!(consumed, 7);
        assert_eq!(parsed.span.col, 1);
        assert_eq!(parsed.span.len, 7);
    }

    #[test]
    fn parses_single_quoted_with_escapes() {
        let (parsed, _) = parse_quoted(r#"'a\nb\tc\\d\'e\"'"#, file(), 1, 1).expect("ok");
        assert_eq!(parsed.raw, "a\nb\tc\\d'e\"");
    }

    #[test]
    fn parses_unicode_escape() {
        let (parsed, _) = parse_quoted(r"'\u{1F600}'", file(), 1, 1).expect("ok");
        assert_eq!(parsed.raw, "\u{1F600}");
        let (parsed2, _) = parse_quoted(r"'\u{41}'", file(), 1, 1).expect("ok");
        assert_eq!(parsed2.raw, "A");
    }

    #[test]
    fn rejects_unterminated() {
        let err = parse_quoted("'abc", file(), 3, 5).expect_err("must fail");
        assert_eq!(err.code, Code::DslUnterminatedString);
        assert_eq!(err.span.expect("span").line, 3);
    }

    #[test]
    fn rejects_bad_escapes_and_unicode() {
        assert!(parse_quoted("'ab\\q'", file(), 1, 1).is_err());
        assert!(parse_quoted("", file(), 1, 1).is_err());
        assert!(parse_quoted("'ab\\u{nope}'", file(), 1, 1).is_err());
        assert!(parse_quoted("'ab\\u{110000}'", file(), 1, 1).is_err());
        assert!(parse_quoted("'ab\\u'", file(), 1, 1).is_err());
    }

    #[test]
    fn interpolation_env_and_var() {
        let (parsed, _) =
            parse_quoted("'${env.TEST_PASSWORD}-${var.USER}'", file(), 2, 9).expect("ok");
        assert_eq!(parsed.raw, "${env.TEST_PASSWORD}-${var.USER}");
        assert_eq!(parsed.env_names(), vec!["TEST_PASSWORD"]);
        assert_eq!(parsed.var_names(), vec!["USER"]);
        assert!(!parsed.is_plain());
        let env_span = parsed
            .parts
            .iter()
            .find_map(|p| match p {
                Part::Env { span, .. } => Some(span.clone()),
                _ => None,
            })
            .expect("env part");
        assert_eq!(env_span.line, 2);
        assert_eq!(env_span.col, 10); // column of '$'
        assert_eq!(env_span.len, 20); // `${env.TEST_PASSWORD}` (2 + 4 + 1 + 13)
    }

    #[test]
    fn interpolation_span_survives_escapes() {
        // The \t escape shrinks one output char from two source chars; the
        // column of ${var.X} must still point at the '$' in the source.
        let (parsed, _) = parse_quoted("'a\\t${var.X}'", file(), 1, 1).expect("ok");
        let var_span = parsed
            .parts
            .iter()
            .find_map(|p| match p {
                Part::Var { span, .. } => Some(span.clone()),
                _ => None,
            })
            .expect("var part");
        assert_eq!(var_span.col, 5);
    }

    #[test]
    fn non_matching_dollar_brace_is_literal() {
        let (parsed, _) = parse_quoted("'${oops} ${env.}'", file(), 1, 1).expect("ok");
        assert!(parsed.is_plain());
        assert_eq!(parsed.raw, "${oops} ${env.}");
    }

    #[test]
    fn multiple_interpolations_keep_literal_segments() {
        let (parsed, _) = parse_quoted("'a${var.X}b${var.Y}c'", file(), 1, 1).expect("ok");
        assert_eq!(parsed.parts.len(), 5);
        assert_eq!(parsed.var_names(), vec!["X", "Y"]);
    }

    #[test]
    fn empty_string_is_allowed() {
        let (parsed, consumed) = parse_quoted("''", file(), 1, 1).expect("ok");
        assert_eq!(parsed.raw, "");
        assert_eq!(consumed, 2);
        assert!(parsed.is_plain());
    }

    #[test]
    fn free_text_rules() {
        assert!(free_text_ok("Login flow"));
        assert!(!free_text_ok(""));
        assert!(!free_text_ok(" padded "));
        assert!(!free_text_ok(&"x".repeat(201)));
        assert!(free_text_ok(&"x".repeat(200)));
    }
}
