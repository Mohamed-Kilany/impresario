//! Canonical locator printer.

use std::fmt::Write as _;

use crate::ast::{Kind, Locator, SimpleSelector, StateFilter};
use crate::text::{MatchMode, NameFilter};

/// Print a locator in its canonical, re-parseable text form.
#[must_use]
pub fn print(locator: &Locator) -> String {
    let mut out = String::new();
    for scope in &locator.within {
        let _ = write!(out, "within({}) ", print(scope));
    }
    for (index, step) in locator.steps.iter().enumerate() {
        if index > 0 {
            out.push_str(" >> ");
        }
        out.push_str(&print_step(step));
    }
    if let Some(nth) = locator.nth {
        let _ = write!(out, "[nth={nth}]");
    }
    if locator.first {
        out.push_str("[first]");
    }
    if locator.last {
        out.push_str("[last]");
    }
    if !locator.visible {
        out.push_str("[visible=false]");
    }
    if let Some(context) = locator.context {
        let _ = write!(out, "[context={}]", context.as_str());
    }
    out.trim_end().to_string()
}

fn print_step(step: &SimpleSelector) -> String {
    let mut out = String::new();
    match &step.kind {
        Kind::Role(role) => {
            let _ = write!(out, "role={}", role.as_str());
        }
        Kind::Text(filter) => {
            let _ = write!(out, "text={}", print_filter(filter));
        }
        Kind::Label(filter) => {
            let _ = write!(out, "label={}", print_filter(filter));
        }
        Kind::Placeholder(filter) => {
            let _ = write!(out, "placeholder={}", print_filter(filter));
        }
        Kind::Alt(filter) => {
            let _ = write!(out, "alt={}", print_filter(filter));
        }
        Kind::Title(filter) => {
            let _ = write!(out, "title={}", print_filter(filter));
        }
        Kind::TestId(value) => {
            let _ = write!(out, "testid={}", print_plain(value));
        }
        Kind::Id(value) => {
            let _ = write!(out, "id={}", print_plain(value));
        }
        Kind::Css(value) => {
            let _ = write!(out, "css={}", print_selector(value));
        }
        Kind::XPath(value) => {
            let _ = write!(out, "xpath={}", print_selector(value));
        }
        Kind::Focused => out.push_str("focused"),
        Kind::Region(region) => {
            let _ = write!(
                out,
                "region={},{},{},{}",
                number(region.x),
                number(region.y),
                number(region.w),
                number(region.h)
            );
        }
    }
    if let Some(filter) = &step.name {
        let _ = write!(out, "[name={}]", print_filter(filter));
    }
    for state in &step.states {
        out.push_str(&print_state(state));
    }
    for has in &step.has {
        let _ = write!(out, "[has=({})]", print(has));
    }
    for has_not in &step.has_not {
        let _ = write!(out, "[has-not=({})]", print(has_not));
    }
    out
}

fn print_filter(filter: &NameFilter) -> String {
    let value = match filter.mode() {
        MatchMode::Regex => format!("/{}/", filter.value().replace('/', "\\/")),
        MatchMode::Contains | MatchMode::Exact => quote(filter.value()),
    };
    let mut out = value;
    if filter.mode() == MatchMode::Exact {
        out.push_str("[exact]");
    }
    if filter.ignore_case() {
        out.push_str("[i]");
    }
    out
}

fn print_state(state: &StateFilter) -> String {
    if state.value {
        format!("[{}]", state.state.as_str())
    } else {
        format!("[{}={}]", state.state.as_str(), state.value)
    }
}

fn print_plain(value: &str) -> String {
    if bare_ok(value) {
        value.to_string()
    } else {
        quote(value)
    }
}

fn print_selector(value: &str) -> String {
    if value.is_empty()
        || value.trim() != value
        || value.contains(">>")
        || value.contains('"')
        || value.contains('\'')
    {
        quote(value)
    } else {
        value.to_string()
    }
}

fn bare_ok(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '.' | '-' | '_' | '/' | ':' | '#' | '@' | '$' | '*' | '+' | '~'
                )
        })
}

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn number(value: f32) -> String {
    let text = format!("{value}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    #[test]
    fn canonical_forms_round_trip() {
        for text in [
            "role=button[name=\"Sign in\"][disabled=false]",
            "text=\"Welcome\"[exact][i]",
            "text=/^Hi, .+/",
            "testid=login.submit",
            "id=com.app:id/ok",
            "css=.btn.primary",
            "xpath=//div/a",
            "focused",
            "region=0.1,0.2,0.3,0.4",
            "within(role=dialog[name=\"Delete\"]) >> role=button[name=\"Delete\"]",
            "role=list[has=(role=listitem)] >> role=button[nth=0]",
            "role=tab[first]",
            "role=tab[last]",
            "role=list[has-not=(role=dialog)][context=WEBVIEW][visible=false]",
        ] {
            let parsed = parse(text).unwrap_or_else(|error| panic!("{text}: {error}"));
            let printed = print(&parsed);
            let reparsed = parse(&printed)
                .unwrap_or_else(|error| panic!("printed '{printed}' does not parse: {error}"));
            assert_eq!(parsed, reparsed, "{text} -> {printed}");
        }
    }

    #[test]
    fn quotes_values_that_need_it() {
        let parsed = parse("testid=\"a b\"").expect("testid");
        assert_eq!(print(&parsed), "testid=\"a b\"");
    }
}
