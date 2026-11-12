//! Recursive-descent parser for the canonical locator text form.

use std::str::FromStr;

use crate::ast::{Context, Kind, Locator, Region, SimpleSelector, State, StateFilter};
use crate::role::Role;
use crate::text::{MatchMode, NameFilter};

/// A locator parse/resolve failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocatorError {
    #[error("empty locator")]
    Empty,
    #[error("locator syntax error at byte {at}: {message}")]
    Unexpected { message: String, at: usize },
    #[error("unknown role '{0}'")]
    UnknownRole(String),
    #[error("unknown state '{0}'")]
    UnknownState(String),
    #[error("unknown context '{0}'")]
    UnknownContext(String),
    #[error("invalid regular expression: {0}")]
    InvalidRegex(String),
    #[error("the snapshot resolver cannot evaluate {0} locators; use the live platform engine")]
    Unsupported(String),
    #[error("locator matched no elements")]
    NotFound,
    #[error("ambiguous locator: {0} matches")]
    Ambiguous(usize),
    #[error("invalid region: {0}")]
    InvalidRegion(String),
}

/// Parse one canonical locator expression.
pub fn parse(input: &str) -> Result<Locator, LocatorError> {
    if input.trim().is_empty() {
        return Err(LocatorError::Empty);
    }
    let mut parser = Parser::new(input);
    let locator = parser.parse_locator()?;
    parser.skip_ws();
    if !parser.is_end() {
        return Err(parser.error("unexpected trailing text"));
    }
    Ok(locator)
}

#[derive(Debug, Clone, Default)]
struct Modifiers {
    nth: Option<usize>,
    first: bool,
    last: bool,
    visible: Option<bool>,
    context: Option<Context>,
}

impl Modifiers {
    fn merge(&mut self, other: Self) {
        if other.nth.is_some() {
            self.nth = other.nth;
            self.first = false;
            self.last = false;
        }
        if other.first {
            self.first = true;
            self.last = false;
            self.nth = None;
        }
        if other.last {
            self.last = true;
            self.first = false;
            self.nth = None;
        }
        if other.visible.is_some() {
            self.visible = other.visible;
        }
        if other.context.is_some() {
            self.context = other.context;
        }
    }
}

struct Parser<'a> {
    input: &'a str,
    position: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, position: 0 }
    }

    fn rest(&self) -> &'a str {
        self.input.get(self.position..).unwrap_or("")
    }

    fn is_end(&self) -> bool {
        self.rest().is_empty()
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        let mut chars = self.rest().chars();
        chars.next()?;
        chars.next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.position += character.len_utf8();
        Some(character)
    }

    fn starts_with(&self, text: &str) -> bool {
        self.rest().starts_with(text)
    }

    fn consume(&mut self, text: &str) -> bool {
        if self.starts_with(text) {
            self.position += text.len();
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn error(&self, message: impl Into<String>) -> LocatorError {
        LocatorError::Unexpected {
            message: message.into(),
            at: self.position,
        }
    }

    fn expect(&mut self, text: &str) -> Result<(), LocatorError> {
        self.skip_ws();
        if self.consume(text) {
            Ok(())
        } else {
            Err(self.error(format!("expected '{text}'")))
        }
    }

    fn parse_locator(&mut self) -> Result<Locator, LocatorError> {
        self.skip_ws();
        let mut within = Vec::new();
        while self.starts_with("within") {
            let saved = self.position;
            self.position += "within".len();
            self.skip_ws();
            if !self.consume("(") {
                self.position = saved;
                break;
            }
            let scope = self.parse_locator()?;
            self.skip_ws();
            self.expect(")")?;
            within.push(scope);
            self.skip_ws();
        }
        if !within.is_empty() {
            // A scope is normally followed by an explicit chain operator:
            // `within(role=dialog) >> role=button`.
            self.consume(">>");
            self.skip_ws();
        }

        let mut steps = Vec::new();
        let mut modifiers = Modifiers::default();
        loop {
            let (step, step_modifiers) = self.parse_step()?;
            modifiers.merge(step_modifiers);
            steps.push(step);
            self.skip_ws();
            if self.consume(">>") {
                self.skip_ws();
                continue;
            }
            break;
        }
        if steps.is_empty() {
            return Err(self.error("expected a locator step"));
        }
        Ok(Locator {
            within,
            steps,
            nth: modifiers.nth,
            first: modifiers.first,
            last: modifiers.last,
            visible: modifiers.visible.unwrap_or(true),
            context: modifiers.context,
        })
    }

    fn parse_step(&mut self) -> Result<(SimpleSelector, Modifiers), LocatorError> {
        self.skip_ws();
        let (kind, name) = self.parse_kind()?;
        let mut step = SimpleSelector::new(kind);
        step.name = name;
        let mut modifiers = Modifiers::default();
        loop {
            self.skip_ws();
            if !self.starts_with("[") {
                break;
            }
            self.bump();
            self.skip_ws();
            let key = self.parse_key()?;
            self.skip_ws();
            match key.as_str() {
                "name" => {
                    self.expect("=")?;
                    step.name = Some(self.parse_filter_value(MatchMode::Contains)?);
                }
                "exact" => apply_match_mode(&mut step, MatchMode::Exact)?,
                "contains" => apply_match_mode(&mut step, MatchMode::Contains)?,
                "i" => apply_ignore_case(&mut step, true)?,
                "has" => {
                    let inner = self.parse_nested_locator()?;
                    step.has.push(inner);
                }
                "has-not" => {
                    let inner = self.parse_nested_locator()?;
                    step.has_not.push(inner);
                }
                "nth" => {
                    self.expect("=")?;
                    modifiers.nth = Some(self.parse_usize()?);
                    modifiers.first = false;
                    modifiers.last = false;
                }
                "first" => {
                    if modifiers.last {
                        return Err(self.error("first and last cannot be combined"));
                    }
                    modifiers.first = true;
                    modifiers.nth = None;
                }
                "last" => {
                    if modifiers.first {
                        return Err(self.error("first and last cannot be combined"));
                    }
                    modifiers.last = true;
                    modifiers.nth = None;
                }
                "visible" => {
                    modifiers.visible = Some(self.parse_optional_bool()?);
                }
                "context" => {
                    self.expect("=")?;
                    modifiers.context = Some(self.parse_context()?);
                }
                state => {
                    let state = parse_state(state)?;
                    let value = self.parse_optional_bool()?;
                    step.states.push(StateFilter { state, value });
                }
            }
            self.skip_ws();
            self.expect("]")?;
        }
        if modifiers.first && modifiers.last {
            return Err(self.error("first and last cannot be combined"));
        }
        Ok((step, modifiers))
    }

    fn parse_kind(&mut self) -> Result<(Kind, Option<NameFilter>), LocatorError> {
        self.skip_ws();
        let keyword = self.parse_keyword()?;
        self.skip_ws();
        if keyword == "focused" {
            return Ok((Kind::Focused, None));
        }
        if !self.consume("=") {
            return Err(self.error(format!("expected '=' after '{keyword}'")));
        }
        self.skip_ws();
        let kind = match keyword.as_str() {
            "role" => {
                let text = self.parse_role_name()?;
                let role =
                    Role::from_str(&text).map_err(|_| LocatorError::UnknownRole(text.clone()))?;
                Kind::Role(role)
            }
            "text" => Kind::Text(self.parse_filter_value(MatchMode::Contains)?),
            "label" => Kind::Label(self.parse_filter_value(MatchMode::Contains)?),
            "placeholder" => Kind::Placeholder(self.parse_filter_value(MatchMode::Contains)?),
            "alt" => Kind::Alt(self.parse_filter_value(MatchMode::Contains)?),
            "title" => Kind::Title(self.parse_filter_value(MatchMode::Contains)?),
            "testid" => Kind::TestId(self.parse_plain_value()?),
            "id" => Kind::Id(self.parse_plain_value()?),
            "css" => Kind::Css(self.parse_css_value()?),
            "xpath" => Kind::XPath(self.parse_css_value()?),
            "region" => Kind::Region(self.parse_region()?),
            _ => return Err(self.error(format!("unknown locator kind '{keyword}'"))),
        };
        Ok((kind, None))
    }

    fn parse_keyword(&mut self) -> Result<String, LocatorError> {
        let mut out = String::new();
        while let Some(character) = self.peek() {
            if character.is_ascii_alphabetic() {
                out.push(character);
                self.bump();
            } else {
                break;
            }
        }
        if out.is_empty() {
            Err(self.error("expected a locator kind"))
        } else {
            Ok(out)
        }
    }

    fn parse_key(&mut self) -> Result<String, LocatorError> {
        let mut out = String::new();
        while let Some(character) = self.peek() {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                out.push(character.to_ascii_lowercase());
                self.bump();
            } else {
                break;
            }
        }
        if out.is_empty() {
            Err(self.error("expected a modifier name"))
        } else {
            Ok(out)
        }
    }

    fn parse_role_name(&mut self) -> Result<String, LocatorError> {
        let mut out = String::new();
        while let Some(character) = self.peek() {
            if matches!(character, '[' | ']' | ')' | '>' | '=') || character.is_whitespace() {
                break;
            }
            out.push(character);
            self.bump();
        }
        if out.trim().is_empty() {
            Err(self.error("expected a role name"))
        } else {
            Ok(out.trim().to_string())
        }
    }

    fn parse_filter_value(&mut self, default_mode: MatchMode) -> Result<NameFilter, LocatorError> {
        self.skip_ws();
        if self.consume("=") {
            self.skip_ws();
        }
        if matches!(self.peek(), Some('"') | Some('\'')) {
            let value = self.parse_quoted()?;
            return filter_from_literal(value, default_mode, false);
        }
        if self.peek() == Some('/') {
            let pattern = self.parse_regex()?;
            return NameFilter::regex(pattern, false);
        }
        let value = self.parse_bare_value()?;
        if value.is_empty() {
            return Err(self.error("expected a text/name value"));
        }
        filter_from_literal(value, default_mode, false)
    }

    fn parse_plain_value(&mut self) -> Result<String, LocatorError> {
        self.skip_ws();
        if matches!(self.peek(), Some('"') | Some('\'')) {
            let value = self.parse_quoted()?;
            if value.is_empty() {
                return Err(self.error("expected a value"));
            }
            return Ok(value);
        }
        let value = self.parse_bare_value()?;
        if value.is_empty() {
            Err(self.error("expected a value"))
        } else {
            Ok(value)
        }
    }

    fn parse_css_value(&mut self) -> Result<String, LocatorError> {
        self.skip_ws();
        if matches!(self.peek(), Some('"') | Some('\'')) {
            let value = self.parse_quoted()?;
            if value.is_empty() {
                return Err(self.error("expected a value"));
            }
            return Ok(value);
        }
        let mut out = String::new();
        while let Some(character) = self.peek() {
            if character == '>' && self.peek_second() == Some('>') {
                break;
            }
            out.push(character);
            self.bump();
        }
        let trimmed = out.trim().to_string();
        if trimmed.is_empty() {
            Err(self.error("expected a value"))
        } else {
            Ok(trimmed)
        }
    }

    fn parse_region(&mut self) -> Result<Region, LocatorError> {
        let x = self.parse_f32_until(&[','])?;
        self.expect(",")?;
        let y = self.parse_f32_until(&[','])?;
        self.expect(",")?;
        let w = self.parse_f32_until(&[','])?;
        self.expect(",")?;
        let h = self.parse_f32_until(&[',', ']'])?;
        let valid = (0.0..=1.0).contains(&x)
            && (0.0..=1.0).contains(&y)
            && w > 0.0
            && h > 0.0
            && x + w <= 1.000_1
            && y + h <= 1.000_1;
        if !valid {
            return Err(LocatorError::InvalidRegion(format!(
                "{x},{y},{w},{h} must be normalized 0..=1 with positive width/height"
            )));
        }
        Ok(Region { x, y, w, h })
    }

    fn parse_f32_until(&mut self, stops: &[char]) -> Result<f32, LocatorError> {
        self.skip_ws();
        let mut out = String::new();
        while let Some(character) = self.peek() {
            if stops.contains(&character) || character == ']' {
                break;
            }
            out.push(character);
            self.bump();
        }
        out.trim()
            .parse::<f32>()
            .map_err(|_| self.error("expected a number"))
    }

    fn parse_quoted(&mut self) -> Result<String, LocatorError> {
        let quote = self
            .bump()
            .ok_or_else(|| self.error("expected a quoted string"))?;
        let mut out = String::new();
        while let Some(character) = self.bump() {
            if character == '\\' {
                let Some(escaped) = self.bump() else {
                    return Err(self.error("unterminated escape"));
                };
                match escaped {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    'u' => out.push(self.parse_unicode_escape()?),
                    other => out.push(other),
                }
            } else if character == quote {
                return Ok(out);
            } else {
                out.push(character);
            }
        }
        Err(self.error("unterminated quoted string"))
    }

    fn parse_unicode_escape(&mut self) -> Result<char, LocatorError> {
        if !self.consume("{") {
            return Err(self.error("expected '{' after \\u"));
        }
        let mut digits = String::new();
        while let Some(character) = self.peek() {
            if character == '}' {
                self.bump();
                break;
            }
            if !character.is_ascii_hexdigit() || digits.len() >= 6 {
                return Err(self.error("invalid unicode escape"));
            }
            digits.push(character);
            self.bump();
        }
        let value =
            u32::from_str_radix(&digits, 16).map_err(|_| self.error("invalid unicode escape"))?;
        char::from_u32(value).ok_or_else(|| self.error("invalid unicode scalar"))
    }

    fn parse_regex(&mut self) -> Result<String, LocatorError> {
        if !self.consume("/") {
            return Err(self.error("expected a regular expression"));
        }
        let mut out = String::new();
        while let Some(character) = self.bump() {
            if character == '\\' {
                let Some(escaped) = self.bump() else {
                    return Err(self.error("unterminated regular expression"));
                };
                if escaped == '/' {
                    out.push('/');
                } else {
                    out.push('\\');
                    out.push(escaped);
                }
            } else if character == '/' {
                return Ok(out);
            } else {
                out.push(character);
            }
        }
        Err(self.error("unterminated regular expression"))
    }

    fn parse_bare_value(&mut self) -> Result<String, LocatorError> {
        let mut out = String::new();
        while let Some(character) = self.peek() {
            if matches!(character, '[' | ']' | ')') {
                break;
            }
            if character == '>' && self.peek_second() == Some('>') {
                break;
            }
            if character == '\\' {
                self.bump();
                if let Some(escaped) = self.bump() {
                    out.push(escaped);
                }
                continue;
            }
            out.push(character);
            self.bump();
        }
        Ok(out.trim().to_string())
    }

    fn parse_nested_locator(&mut self) -> Result<Locator, LocatorError> {
        self.skip_ws();
        if self.consume("=") {
            self.skip_ws();
        }
        self.expect("(")?;
        let inner = self.parse_locator()?;
        self.skip_ws();
        self.expect(")")?;
        Ok(inner)
    }

    fn parse_usize(&mut self) -> Result<usize, LocatorError> {
        self.skip_ws();
        let mut digits = String::new();
        while let Some(character) = self.peek() {
            if character.is_ascii_digit() {
                digits.push(character);
                self.bump();
            } else {
                break;
            }
        }
        if digits.is_empty() {
            Err(self.error("expected a non-negative integer"))
        } else {
            digits
                .parse::<usize>()
                .map_err(|_| self.error("integer is too large"))
        }
    }

    fn parse_optional_bool(&mut self) -> Result<bool, LocatorError> {
        self.skip_ws();
        if self.consume("=") {
            self.skip_ws();
        }
        if self.consume("true") {
            return Ok(true);
        }
        if self.consume("false") {
            return Ok(false);
        }
        if self.starts_with("]") || self.is_end() {
            return Ok(true);
        }
        Err(self.error("expected true or false"))
    }

    fn parse_context(&mut self) -> Result<Context, LocatorError> {
        let mut out = String::new();
        while let Some(character) = self.peek() {
            if character.is_ascii_alphabetic() {
                out.push(character.to_ascii_uppercase());
                self.bump();
            } else {
                break;
            }
        }
        match out.as_str() {
            "NATIVE" => Ok(Context::Native),
            "WEBVIEW" => Ok(Context::WebView),
            "FLUTTER" => Ok(Context::Flutter),
            "WEB" => Ok(Context::Web),
            other => Err(LocatorError::UnknownContext(other.to_string())),
        }
    }
}

fn filter_from_literal(
    value: String,
    mode: MatchMode,
    ignore_case: bool,
) -> Result<NameFilter, LocatorError> {
    match mode {
        MatchMode::Contains => Ok(NameFilter::contains(value, ignore_case)),
        MatchMode::Exact => Ok(NameFilter::exact(value, ignore_case)),
        MatchMode::Regex => NameFilter::regex(value, ignore_case),
    }
}

fn apply_match_mode(step: &mut SimpleSelector, mode: MatchMode) -> Result<(), LocatorError> {
    if let Some(filter) = step.kind.filter_mut() {
        *filter = filter.with_mode(mode)?;
        return Ok(());
    }
    if let Some(filter) = step.name.as_mut() {
        *filter = filter.with_mode(mode)?;
        return Ok(());
    }
    Err(LocatorError::Unexpected {
        message: "[exact]/[contains] needs a text/name filter".to_string(),
        at: 0,
    })
}

fn apply_ignore_case(step: &mut SimpleSelector, ignore_case: bool) -> Result<(), LocatorError> {
    if let Some(filter) = step.kind.filter_mut() {
        *filter = filter.with_ignore_case(ignore_case)?;
        return Ok(());
    }
    if let Some(filter) = step.name.as_mut() {
        *filter = filter.with_ignore_case(ignore_case)?;
        return Ok(());
    }
    Err(LocatorError::Unexpected {
        message: "[i] needs a text/name filter".to_string(),
        at: 0,
    })
}

fn parse_state(value: &str) -> Result<State, LocatorError> {
    match value {
        "enabled" => Ok(State::Enabled),
        "disabled" => Ok(State::Disabled),
        "focused" => Ok(State::Focused),
        "selected" => Ok(State::Selected),
        "checked" => Ok(State::Checked),
        "expanded" => Ok(State::Expanded),
        "pressed" => Ok(State::Pressed),
        "readonly" => Ok(State::ReadOnly),
        "required" => Ok(State::Required),
        other => Err(LocatorError::UnknownState(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_spec_examples() {
        let role = parse("role=button[name=\"Sign in\"][disabled=false]").expect("role");
        assert_eq!(role.steps.len(), 1);
        assert_eq!(
            role.steps.first().map(|step| step.kind.clone()),
            Some(Kind::Role(Role::Button))
        );
        assert_eq!(
            role.steps.first().and_then(|step| step.states.first()),
            Some(&StateFilter {
                state: State::Disabled,
                value: false
            })
        );

        let text = parse("text=\"Welcome\"").expect("text");
        assert!(matches!(
            text.steps.first().map(|step| &step.kind),
            Some(Kind::Text(_))
        ));
        let regex = parse("text=/^Hi, .+/").expect("regex");
        assert!(matches!(
            regex.steps.first().map(|step| &step.kind),
            Some(Kind::Text(_))
        ));
        let focused = parse("focused").expect("focused");
        assert!(matches!(
            focused.steps.first().map(|step| &step.kind),
            Some(Kind::Focused)
        ));
    }

    #[test]
    fn parses_chains_scopes_and_has() {
        let chained = parse("within(role=dialog[name=\"Delete\"]) >> role=button[name=\"Delete\"]")
            .expect("chain");
        assert_eq!(chained.within.len(), 1);
        assert_eq!(chained.steps.len(), 1);

        let has = parse("role=list[has=(role=listitem)] >> role=button[nth=0]").expect("has");
        assert_eq!(has.steps.len(), 2);
        assert_eq!(has.nth, Some(0));
        assert_eq!(has.steps.first().map(|step| step.has.len()), Some(1));
    }

    #[test]
    fn parses_all_kinds_and_modifiers() {
        for text in [
            "role=textbox[name=\"Email\"][i]",
            "text=Search[exact]",
            "label=\"Email\"",
            "placeholder=Search",
            "alt=\"Logo\"",
            "title=\"Close\"",
            "testid=login.submit",
            "id=com.app:id/ok",
            "css=.btn.primary",
            "xpath=//div/a",
            "region=0.1,0.2,0.3,0.4",
            "role=list[has-not=(role=dialog)][context=WEBVIEW][visible=false]",
            "role=tab[first]",
            "role=tab[last]",
        ] {
            let locator = parse(text).unwrap_or_else(|error| panic!("{text}: {error}"));
            assert!(!locator.steps.is_empty(), "{text}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        for text in [
            "",
            "role=",
            "role=nonsense",
            "text=",
            "role=button[",
            "role=button[nth=x]",
            "role=button[first][last]",
            "region=0.1,0.2,0.3",
            "region=2,0,1,1",
            "text=/unterminated",
            "wat=button",
        ] {
            assert!(parse(text).is_err(), "{text} should fail");
        }
    }

    #[test]
    fn quoted_values_support_escapes() {
        let locator = parse("text=\"Say \\\"hi\\\"\"[exact]").expect("escaped");
        match locator.steps.first().map(|step| &step.kind) {
            Some(Kind::Text(filter)) => {
                assert_eq!(filter.value(), "Say \"hi\"");
                assert_eq!(filter.mode(), MatchMode::Exact);
            }
            other => panic!("unexpected kind {other:?}"),
        }
    }
}
