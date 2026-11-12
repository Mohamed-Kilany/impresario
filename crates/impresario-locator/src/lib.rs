#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::todo,
        clippy::unreachable
    )
)]

//! `impresario-locator`: one pure locator engine shared by the inspector and
//! the runner.
//!
//! The canonical text form is additive to the existing per-platform
//! `prefix:value` selector grammar. Existing scene cells are untouched; a
//! cell is treated as a locator expression only when it starts with a locator
//! keyword (`role=`, `text=`, `testid=`, ...), is exactly `focused`, contains
//! a top-level `>>`, or starts with `within(`.
//!
//! Resolution is a pure function over a snapshot-like flat tree (see
//! [`matcher::Node`]); there is no second matching implementation.

pub mod ast;
pub mod legacy;
pub mod matcher;
pub mod parse;
pub mod print;
pub mod role;
pub mod synthesize;
pub mod text;

pub use ast::{Context, Kind, Locator, Rect, Region, SimpleSelector, State, StateFilter};
pub use legacy::{to_locator, LegacyPlatform};
pub use matcher::{resolve, resolve_one, Node, NodeMatch};
pub use parse::{parse, LocatorError};
pub use print::print;
pub use role::Role;
pub use synthesize::{looks_dynamic, synthesize, Suggestion, SynthesisOptions};
pub use text::{MatchMode, NameFilter};

/// True when a scene cell should be routed to the locator parser instead of
/// the legacy selector parser. This is deliberately conservative so every
/// existing scene keeps its exact meaning.
#[must_use]
pub fn is_locator_expression(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed == "focused" || trimmed.starts_with("focused[") {
        return true;
    }
    if trimmed.starts_with("within(") {
        return true;
    }
    if contains_top_level_chain(trimmed) {
        return true;
    }
    let Some((key, _)) = trimmed.split_once('=') else {
        return false;
    };
    matches!(
        key.trim(),
        "role"
            | "text"
            | "label"
            | "placeholder"
            | "alt"
            | "title"
            | "testid"
            | "id"
            | "css"
            | "xpath"
            | "region"
    )
}

fn contains_top_level_chain(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0usize;
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    while let Some(&byte) = bytes.get(index) {
        if let Some(active) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == active {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'>' if depth == 0 && bytes.get(index + 1) == Some(&b'>') => {
                return true;
            }
            _ => {}
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_locator_cells_without_capturing_legacy_forms() {
        assert!(is_locator_expression("role=button[name=\"Sign in\"]"));
        assert!(is_locator_expression("text=Welcome"));
        assert!(is_locator_expression("testid=login.submit"));
        assert!(is_locator_expression("focused"));
        assert!(is_locator_expression("within(role=dialog) >> button"));
        assert!(is_locator_expression("role=list >> role=listitem"));

        assert!(!is_locator_expression("css:#username"));
        assert!(!is_locator_expression("#username"));
        assert!(!is_locator_expression("text:Welcome"));
        assert!(!is_locator_expression("accessibilityId:login"));
        assert!(!is_locator_expression("button[type='submit']"));
        assert!(!is_locator_expression("id:login_btn"));
    }
}
