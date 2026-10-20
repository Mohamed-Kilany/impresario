//! Durations in the DSL: integer + unit `ms`, `s`, `m`, range `1ms..=10m`.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use winnow::ascii::dec_uint;
use winnow::combinator::alt;
use winnow::Parser;

use crate::diag::{Code, Diagnostic};
use crate::span::Span;

/// The maximum duration the DSL accepts (10 minutes).
pub const MAX: Duration = Duration::from_secs(600);

/// A validated DSL duration: `1ms..=10m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DslDuration {
    millis: u64,
}

impl DslDuration {
    /// Build from milliseconds; `None` when out of range.
    #[must_use]
    pub fn from_millis(millis: u64) -> Option<Self> {
        if millis == 0 || millis > MAX.as_millis() as u64 {
            None
        } else {
            Some(Self { millis })
        }
    }

    #[must_use]
    pub fn millis(self) -> u64 {
        self.millis
    }

    /// One second (always valid; used as the `longpress` default).
    pub const ONE_SECOND: DslDuration = DslDuration { millis: 1000 };

    #[must_use]
    pub fn as_std(self) -> Duration {
        Duration::from_millis(self.millis)
    }
}

impl fmt::Display for DslDuration {
    /// Canonical rendering: `500ms`, `2s`, `1m`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.millis.is_multiple_of(60_000) {
            write!(f, "{}m", self.millis / 60_000)
        } else if self.millis.is_multiple_of(1000) {
            write!(f, "{}s", self.millis / 1000)
        } else {
            write!(f, "{}ms", self.millis)
        }
    }
}

impl From<DslDuration> for Duration {
    fn from(value: DslDuration) -> Self {
        value.as_std()
    }
}

fn unit_millis(input: &mut &str) -> winnow::Result<u64> {
    alt(("ms".value(1u64), "s".value(1000u64), "m".value(60_000u64))).parse_next(input)
}

/// Non-negative integer (error type pinned to `ContextError`).
fn uint(input: &mut &str) -> winnow::Result<u64> {
    dec_uint(input)
}

/// Parse `"<integer><ms|s|m>"` strictly (no whitespace, no sign).
fn parse_duration(input: &str) -> Result<u64, String> {
    let mut rest = input;
    let value: u64 =
        uint(&mut rest).map_err(|_| "expected an integer followed by ms, s or m".to_string())?;
    let unit: u64 = unit_millis
        .parse_next(&mut rest)
        .map_err(|_| "expected unit ms, s or m".to_string())?;
    if !rest.is_empty() {
        return Err("unexpected trailing characters after duration".to_string());
    }
    let millis = value
        .checked_mul(unit)
        .ok_or_else(|| "duration overflows".to_string())?;
    Ok(millis)
}

/// Parse a duration literal, returning a diagnostic on failure.
///
/// `span` (when present) is attached to the produced diagnostic.
pub fn parse(input: &str, span: Option<Span>) -> Result<DslDuration, Diagnostic> {
    let attach = |mut diag: Diagnostic| -> Diagnostic {
        if let Some(span) = span.clone() {
            diag = diag.with_span(span);
        }
        diag
    };
    let millis = parse_duration(input).map_err(|reason| {
        attach(
            Diagnostic::error(
                Code::DslInvalidDuration,
                format!("invalid duration '{input}': {reason}"),
            )
            .with_help("durations look like 500ms, 2s or 1m and must be within 1ms..=10m"),
        )
    })?;
    DslDuration::from_millis(millis).ok_or_else(|| {
        attach(
            Diagnostic::error(
                Code::DslInvalidDuration,
                format!("duration '{input}' is out of range"),
            )
            .with_help("durations are between 1ms and 10m"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_unit() {
        assert_eq!(parse("500ms", None).map(|d| d.millis()), Ok(500));
        assert_eq!(parse("2s", None).map(|d| d.millis()), Ok(2000));
        assert_eq!(parse("1m", None).map(|d| d.millis()), Ok(60_000));
        assert_eq!(parse("10m", None).map(|d| d.millis()), Ok(600_000));
        assert_eq!(parse("1ms", None).map(|d| d.millis()), Ok(1));
    }

    #[test]
    fn rejects_bad_durations() {
        for bad in [
            "0ms", "0s", "0m", "11m", "600001ms", "500", "ms", "5x", "-5ms", "1.5s", "1 s",
            "500ms ", "1h",
        ] {
            assert!(parse(bad, None).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn rejects_unknown_unit_before_overflow() {
        assert!(parse("999999999999999999999ms", None).is_err());
    }

    #[test]
    fn canonical_rendering_round_trips() {
        for text in ["500ms", "2s", "1m", "10m", "1500ms", "90000ms"] {
            let parsed = parse(text, None).expect("valid");
            let rendered = parsed.to_string();
            let again = parse(&rendered, None).expect("valid");
            assert_eq!(parsed, again);
        }
        assert_eq!(parse("90s", None).map(|d| d.to_string()), Ok("90s".into()));
        assert_eq!(
            parse("1500ms", None).map(|d| d.to_string()),
            Ok("1500ms".into())
        );
        assert_eq!(parse("120s", None).map(|d| d.to_string()), Ok("2m".into()));
    }
}
