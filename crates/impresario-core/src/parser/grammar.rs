//! Winnow grammars for line-level constructs (D-05).
//!
//! The file structure (indentation, headers) is parsed by the hand-written
//! parser in `super`; these primitives cover the small line grammars:
//! identifiers, integers, tags, platform lists, directions, key names.

use winnow::ascii::dec_uint;
use winnow::combinator::alt;
use winnow::token::{one_of, take_while};
use winnow::Parser;

use crate::ast::{AppOp, Direction};
use crate::keys::RemoteKey;
use crate::platforms::Platform;

/// `[A-Za-z][A-Za-z0-9_]*`
pub fn ident(input: &mut &str) -> winnow::Result<String> {
    let first = one_of(|c: char| c.is_ascii_alphabetic()).parse_next(input)?;
    let rest: &str =
        take_while(0.., |c: char| c.is_ascii_alphanumeric() || c == '_').parse_next(input)?;
    let mut out = String::with_capacity(1 + rest.len());
    out.push(first);
    out.push_str(rest);
    Ok(out)
}

/// A bare token: letters, digits, `_`, `-`, `.`.
pub fn token<'i>(input: &mut &'i str) -> winnow::Result<&'i str> {
    take_while(1.., |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
    })
    .parse_next(input)
}

/// Non-negative integer.
pub fn integer(input: &mut &str) -> winnow::Result<u32> {
    dec_uint(input)
}

/// Device platform name (exact).
pub fn platform(input: &mut &str) -> winnow::Result<Platform> {
    let save = *input;
    let word = token.parse_next(input)?;
    match word.parse::<Platform>() {
        Ok(platform) => Ok(platform),
        Err(()) => {
            *input = save;
            Err(winnow::error::ContextError::new())
        }
    }
}

/// Tag: `[a-z0-9_-]+`
pub fn tag<'i>(input: &mut &'i str) -> winnow::Result<&'i str> {
    take_while(1.., |c: char| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-')
    })
    .parse_next(input)
}

/// Scroll/swipe direction.
pub fn direction(input: &mut &str) -> winnow::Result<Direction> {
    alt((
        "down".value(Direction::Down),
        "left".value(Direction::Left),
        "right".value(Direction::Right),
        "up".value(Direction::Up),
    ))
    .parse_next(input)
}

/// `key` action key name (exact, case-sensitive).
pub fn keyname(input: &mut &str) -> winnow::Result<RemoteKey> {
    let save = *input;
    let word = token.parse_next(input)?;
    match RemoteKey::from_dsl(word) {
        Some(key) => Ok(key),
        None => {
            *input = save;
            Err(winnow::error::ContextError::new())
        }
    }
}

/// `app` operation.
pub fn app_op(input: &mut &str) -> winnow::Result<AppOp> {
    alt((
        "terminate".value(AppOp::Terminate),
        "restart".value(AppOp::Restart),
        "launch".value(AppOp::Launch),
        "background".value(AppOp::Background),
    ))
    .parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use winnow::Parser;

    #[test]
    fn parses_ident() {
        let mut input = "LoginScreen rest";
        assert_eq!(ident.parse_next(&mut input), Ok("LoginScreen".into()));
        assert_eq!(input, " rest");

        let mut bad = "9nope";
        assert!(ident.parse_next(&mut bad).is_err());
        let mut empty = "";
        assert!(ident.parse_next(&mut empty).is_err());
    }

    #[test]
    fn parses_integer() {
        let mut input = "42abc";
        assert_eq!(integer.parse_next(&mut input), Ok(42));
        assert_eq!(input, "abc");
        let mut bad = "abc";
        assert!(integer.parse_next(&mut bad).is_err());
    }

    #[test]
    fn parses_platforms_exactly() {
        for p in Platform::ALL {
            let mut input = p.as_str();
            assert_eq!(platform.parse_next(&mut input), Ok(p));
        }
        let mut bad = "webostv";
        assert!(platform.parse_next(&mut bad).is_err());
        // failure must not consume input
        assert_eq!(bad, "webostv");
    }

    #[test]
    fn parses_tags() {
        let mut input = "smoke-1x_ok rest";
        assert_eq!(tag.parse_next(&mut input), Ok("smoke-1x_ok"));
        let mut bad = "UPPER";
        assert!(tag.parse_next(&mut bad).is_err());
    }

    #[test]
    fn parses_directions_and_keys() {
        let mut input = "down";
        assert_eq!(direction.parse_next(&mut input), Ok(Direction::Down));
        let mut input = "playpause";
        assert_eq!(keyname.parse_next(&mut input), Ok(RemoteKey::PlayPause));
        let mut bad = "select";
        assert!(keyname.parse_next(&mut bad).is_err());
    }

    #[test]
    fn parses_app_ops() {
        let mut input = "terminate";
        assert_eq!(app_op.parse_next(&mut input), Ok(AppOp::Terminate));
        let mut bad = "stop";
        assert!(app_op.parse_next(&mut bad).is_err());
    }
}
