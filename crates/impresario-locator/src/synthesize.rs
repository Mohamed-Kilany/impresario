//! Ranked locator synthesis with stability scoring.
//!
//! Suggestions are generated from the normalized node model, printed in the
//! canonical text form and verified against the same snapshot the inspector
//! displays. A suggestion that matches several nodes is either kept with an
//! explicit ambiguity note or disambiguated with a positional modifier or a
//! stable ancestor scope.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ast::{Kind, Locator, Region};
use crate::matcher::{resolve, Node};
use crate::parse::LocatorError;
use crate::print::print;
use crate::role::Role;
use crate::text::NameFilter;

/// One ranked locator candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// Canonical locator text, ready to paste into a scene cell.
    pub locator: String,
    /// Stability score, 0 (fragile) to 100 (stable).
    pub score: u8,
    /// True when the score and the verification notes indicate a stable
    /// locator (no dynamic value, positional fallback or visual region).
    pub stable: bool,
    /// Human-readable reasons for the score.
    pub notes: Vec<String>,
}

/// Synthesis knobs.
#[derive(Debug, Clone, PartialEq)]
pub struct SynthesisOptions {
    /// Maximum number of ranked suggestions returned.
    pub max_candidates: usize,
    /// Screen size in logical units, used only for `region=` fallbacks.
    pub screen: (f32, f32),
    /// Whether CSS candidates are included when the node provides one.
    pub allow_css: bool,
    /// Whether XPath candidates are included when the node provides one.
    pub allow_xpath: bool,
    /// Whether a visual `region=` fallback is included.
    pub allow_region: bool,
}

impl Default for SynthesisOptions {
    fn default() -> Self {
        Self {
            max_candidates: 12,
            screen: (0.0, 0.0),
            allow_css: true,
            allow_xpath: false,
            allow_region: true,
        }
    }
}

/// Generate ranked locators for `index`.
pub fn synthesize<N: Node>(
    index: usize,
    nodes: &[N],
    options: &SynthesisOptions,
) -> Vec<Suggestion> {
    let Some(node) = nodes.get(index) else {
        return Vec::new();
    };
    let mut suggestions = BTreeMap::new();
    for candidate in build_candidates(node, options) {
        score_candidate(index, nodes, options, candidate, &mut suggestions);
    }
    let mut ranked: Vec<Suggestion> = suggestions.into_values().collect();
    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.locator.len().cmp(&right.locator.len()))
            .then_with(|| left.locator.cmp(&right.locator))
    });
    ranked.truncate(options.max_candidates.max(1));
    ranked
}

/// Heuristic for values that change between snapshots (ids with counters,
/// timestamps, UUIDs and long generated text).
#[must_use]
pub fn looks_dynamic(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    if is_uuid(trimmed) {
        return true;
    }
    if longest_digit_run(trimmed) >= 2 {
        return true;
    }
    if has_long_hex_run(trimmed) {
        return true;
    }
    if has_time_like_pair(trimmed) {
        return true;
    }
    trimmed.chars().count() > 120
}

fn is_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.as_bytes().iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

fn longest_digit_run(value: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for character in value.chars() {
        if character.is_ascii_digit() {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

fn has_long_hex_run(value: &str) -> bool {
    let mut run = 0;
    let mut has_digit = false;
    for character in value.chars() {
        if character.is_ascii_hexdigit() {
            run += 1;
            has_digit |= character.is_ascii_digit();
            if run >= 8 && has_digit {
                return true;
            }
        } else {
            run = 0;
            has_digit = false;
        }
    }
    false
}

fn has_time_like_pair(value: &str) -> bool {
    value.as_bytes().windows(2).any(|pair| match pair {
        [left, right] => {
            (left.is_ascii_digit() && *right == b':') || (*left == b':' && right.is_ascii_digit())
        }
        _ => false,
    })
}

struct Candidate {
    locator: Locator,
    score: i32,
    notes: Vec<String>,
}

fn build_candidates<N: Node>(node: &N, options: &SynthesisOptions) -> Vec<Candidate> {
    let mut candidates = Vec::new();

    if let Some(test_id) = non_empty(node.test_id()) {
        add(
            &mut candidates,
            Locator::simple(Kind::TestId(test_id.to_string())),
            100,
            "test id",
        );
    }
    if let Some(native_id) = non_empty(node.native_id()) {
        add(
            &mut candidates,
            Locator::simple(Kind::Id(native_id.to_string())),
            92,
            "platform element id",
        );
    }

    let name = node.name().trim();
    if node.role() != Role::Generic {
        let (locator, score, note) = if name.is_empty() {
            (
                Locator::simple(Kind::Role(node.role())),
                35,
                "role only; likely ambiguous",
            )
        } else {
            (
                Locator::simple(Kind::Role(node.role())).with_name(NameFilter::exact(name, false)),
                90,
                "role and exact accessible name",
            )
        };
        add(&mut candidates, locator, score, note);
    }

    let label = node.label().trim();
    if !label.is_empty() {
        if node.role() != Role::Generic {
            add(
                &mut candidates,
                Locator::simple(Kind::Role(node.role())).with_name(NameFilter::exact(label, false)),
                86,
                "role and exact label",
            );
        }
        add(
            &mut candidates,
            Locator::simple(Kind::Label(NameFilter::exact(label, false))),
            72,
            "exact label",
        );
    }

    let text = node.text().trim();
    if !text.is_empty() {
        add(
            &mut candidates,
            Locator::simple(Kind::Text(NameFilter::exact(text, false))),
            76,
            "exact visible text",
        );
    }

    let placeholder = node.placeholder().trim();
    if !placeholder.is_empty() {
        add(
            &mut candidates,
            Locator::simple(Kind::Placeholder(NameFilter::exact(placeholder, false))),
            74,
            "exact placeholder",
        );
    }

    let alt = node.alt().trim();
    if !alt.is_empty() {
        add(
            &mut candidates,
            Locator::simple(Kind::Alt(NameFilter::exact(alt, false))),
            70,
            "exact alternative text",
        );
    }

    let title = node.title().trim();
    if !title.is_empty() {
        add(
            &mut candidates,
            Locator::simple(Kind::Title(NameFilter::exact(title, false))),
            68,
            "exact title",
        );
    }

    if let Some(css) = non_empty(node.css_selector()).filter(|_| options.allow_css) {
        add(
            &mut candidates,
            Locator::simple(Kind::Css(css.to_string())),
            45,
            "CSS selector; fragile and not snapshot-verifiable",
        );
    }
    if let Some(xpath) = non_empty(node.xpath()).filter(|_| options.allow_xpath) {
        add(
            &mut candidates,
            Locator::simple(Kind::XPath(xpath.to_string())),
            35,
            "XPath; fragile and not snapshot-verifiable",
        );
    }

    candidates
}

fn add(candidates: &mut Vec<Candidate>, locator: Locator, score: i32, note: &str) {
    candidates.push(Candidate {
        locator,
        score,
        notes: vec![note.to_string()],
    });
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
}

fn score_candidate<N: Node>(
    index: usize,
    nodes: &[N],
    options: &SynthesisOptions,
    candidate: Candidate,
    suggestions: &mut BTreeMap<String, Suggestion>,
) {
    let mut score = candidate.score;
    let mut notes = candidate.notes;
    if let Some(value) = candidate_value(&candidate.locator) {
        if looks_dynamic(value) {
            score -= 30;
            notes.push("value looks dynamic".to_string());
        }
        if value.chars().count() > 80 {
            score -= 10;
            notes.push("long value".to_string());
        }
    }

    match resolve(&candidate.locator, nodes, options.screen) {
        Ok(matches) => {
            let Some(position) = matches.iter().position(|matched| matched.index == index) else {
                return;
            };
            if matches.len() == 1 {
                score += 5;
                notes.push("unique in the snapshot".to_string());
                insert(suggestions, candidate.locator, score, notes);
            } else {
                score -= 15;
                notes.push(format!("matches {} elements", matches.len()));
                insert(suggestions, candidate.locator.clone(), score, notes.clone());

                let mut positional = candidate.locator.clone();
                positional.nth = Some(position);
                positional.first = false;
                positional.last = false;
                if verified(&positional, nodes, options.screen, index) {
                    let mut positional_notes = notes.clone();
                    positional_notes.push(format!(
                        "positional match {} of {}",
                        position + 1,
                        matches.len()
                    ));
                    insert(suggestions, positional, score - 10, positional_notes);
                }

                if let Some(scope) = ancestor_scope(nodes, index, options.screen) {
                    let mut scoped = candidate.locator;
                    scoped.within.push(scope);
                    if verified(&scoped, nodes, options.screen, index) {
                        let mut scoped_notes = notes;
                        scoped_notes.push("scoped by a stable ancestor".to_string());
                        insert(suggestions, scoped, score + 20, scoped_notes);
                    }
                }
            }
        }
        Err(LocatorError::Unsupported(_)) => {
            score -= 10;
            notes.push("not verifiable from the snapshot".to_string());
            insert(suggestions, candidate.locator, score, notes);
        }
        Err(_) => {}
    }

    if let Some(region) =
        region_candidate(nodes.get(index), options.screen).filter(|_| options.allow_region)
    {
        insert(
            suggestions,
            Locator::simple(Kind::Region(region)),
            10,
            vec!["visual region; fragile".to_string()],
        );
    }
}

fn candidate_value(locator: &Locator) -> Option<&str> {
    let step = locator.steps.first()?;
    if let Some(filter) = &step.name {
        return Some(filter.value());
    }
    match &step.kind {
        Kind::TestId(value) | Kind::Id(value) | Kind::Css(value) | Kind::XPath(value) => {
            Some(value)
        }
        Kind::Text(filter)
        | Kind::Label(filter)
        | Kind::Placeholder(filter)
        | Kind::Alt(filter)
        | Kind::Title(filter) => Some(filter.value()),
        Kind::Role(_) | Kind::Focused | Kind::Region(_) => None,
    }
}

fn region_candidate<N: Node>(node: Option<&N>, screen: (f32, f32)) -> Option<Region> {
    let node = node?;
    if screen.0 <= 0.0 || screen.1 <= 0.0 {
        return None;
    }
    let rect = node.rect();
    if rect.area() <= 0.0 {
        return None;
    }
    let x = (rect.x / screen.0).clamp(0.0, 1.0);
    let y = (rect.y / screen.1).clamp(0.0, 1.0);
    let w = (rect.w / screen.0).clamp(0.0, 1.0 - x);
    let h = (rect.h / screen.1).clamp(0.0, 1.0 - y);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(Region { x, y, w, h })
}

fn ancestor_scope<N: Node>(nodes: &[N], index: usize, screen: (f32, f32)) -> Option<Locator> {
    let mut current = nodes.get(index)?.parent();
    let mut depth = 0;
    while let Some(parent_index) = current {
        if depth >= 8 {
            break;
        }
        let parent = nodes.get(parent_index)?;
        let name = parent.name().trim();
        if !name.is_empty() && parent.role() != Role::Generic {
            let locator = Locator::simple(Kind::Role(parent.role()))
                .with_name(NameFilter::exact(name, false));
            if verified(&locator, nodes, screen, parent_index) {
                return Some(locator);
            }
        }
        current = parent.parent();
        depth += 1;
    }
    None
}

fn verified<N: Node>(locator: &Locator, nodes: &[N], screen: (f32, f32), index: usize) -> bool {
    resolve(locator, nodes, screen)
        .map(|matches| {
            matches.len() == 1
                && matches
                    .first()
                    .is_some_and(|matched| matched.index == index)
        })
        .unwrap_or(false)
}

fn insert(
    suggestions: &mut BTreeMap<String, Suggestion>,
    locator: Locator,
    score: i32,
    notes: Vec<String>,
) {
    let printed = print(&locator);
    if printed.is_empty() {
        return;
    }
    let clamped = score.clamp(0, 100) as u8;
    let stable = clamped >= 70
        && !notes.iter().any(|note| {
            note.contains("dynamic")
                || note.contains("positional")
                || note.contains("fragile")
                || note.contains("not verifiable")
        });
    let suggestion = Suggestion {
        locator: printed.clone(),
        score: clamped,
        stable,
        notes,
    };
    match suggestions.get(&printed) {
        Some(existing) if existing.score >= clamped => {}
        _ => {
            suggestions.insert(printed, suggestion);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Context, Rect, State};
    use crate::matcher::Node;

    struct MockNode {
        id: &'static str,
        role: Role,
        name: &'static str,
        text: &'static str,
        test_id: Option<&'static str>,
        native_id: Option<&'static str>,
        css: Option<&'static str>,
        parent: Option<usize>,
        rect: Rect,
    }

    impl Node for MockNode {
        fn id(&self) -> &str {
            self.id
        }

        fn role(&self) -> Role {
            self.role
        }

        fn name(&self) -> &str {
            self.name
        }

        fn text(&self) -> &str {
            self.text
        }

        fn label(&self) -> &str {
            self.name
        }

        fn test_id(&self) -> Option<&str> {
            self.test_id
        }

        fn native_id(&self) -> Option<&str> {
            self.native_id
        }

        fn css_selector(&self) -> Option<&str> {
            self.css
        }

        fn parent(&self) -> Option<usize> {
            self.parent
        }

        fn rect(&self) -> Rect {
            self.rect
        }

        fn context(&self) -> Context {
            Context::Web
        }

        fn state(&self, _state: State) -> Option<bool> {
            None
        }
    }

    fn node(id: &'static str, role: Role, name: &'static str, parent: Option<usize>) -> MockNode {
        MockNode {
            id,
            role,
            name,
            text: name,
            test_id: None,
            native_id: None,
            css: None,
            parent,
            rect: Rect::new(10.0, 10.0, 20.0, 20.0),
        }
    }

    fn options() -> SynthesisOptions {
        SynthesisOptions {
            screen: (100.0, 100.0),
            ..SynthesisOptions::default()
        }
    }

    #[test]
    fn test_ids_rank_first_and_are_stable() {
        let mut confirm = node("confirm", Role::Button, "Delete", None);
        confirm.test_id = Some("delete.confirm");
        let nodes = vec![confirm];
        let suggestions = synthesize(0, &nodes, &options());
        let first = suggestions.first().expect("suggestion");
        assert_eq!(first.locator, "testid=delete.confirm");
        assert_eq!(first.score, 100);
        assert!(first.stable);
    }

    #[test]
    fn unique_role_and_name_beats_css() {
        let mut cancel = node("cancel", Role::Button, "Cancel", None);
        cancel.css = Some(".cancel");
        let nodes = vec![cancel];
        let suggestions = synthesize(0, &nodes, &options());
        assert_eq!(
            suggestions
                .first()
                .map(|suggestion| suggestion.locator.as_str()),
            Some("role=button[name=\"Cancel\"][exact]")
        );
        assert!(suggestions
            .iter()
            .any(|suggestion| suggestion.locator == "css=.cancel"));
    }

    #[test]
    fn ambiguous_names_get_positional_and_scoped_variants() {
        let dialog = node("dialog", Role::Dialog, "Delete", None);
        let confirm = node("confirm", Role::Button, "Delete", Some(0));
        let outside = node("outside", Role::Button, "Delete", None);
        let nodes = vec![dialog, confirm, outside];
        let suggestions = synthesize(1, &nodes, &options());
        assert!(suggestions
            .iter()
            .any(|suggestion| suggestion.locator.contains("[nth=0]")));
        assert!(suggestions.iter().any(|suggestion| {
            suggestion
                .locator
                .starts_with("within(role=dialog[name=\"Delete\"][exact])")
        }));
    }

    #[test]
    fn dynamic_text_is_penalized_and_not_stable() {
        let item = node("item", Role::Text, "Order 12345", None);
        let nodes = vec![item];
        let suggestions = synthesize(0, &nodes, &options());
        let text = suggestions
            .iter()
            .find(|suggestion| suggestion.locator.starts_with("text="))
            .expect("text suggestion");
        assert!(!text.stable);
        assert!(text.notes.iter().any(|note| note.contains("dynamic")));
    }

    #[test]
    fn region_is_last_resort_and_normalized() {
        let button = node("button", Role::Button, "Save", None);
        let nodes = vec![button];
        let suggestions = synthesize(0, &nodes, &options());
        let region = suggestions
            .iter()
            .find(|suggestion| suggestion.locator.starts_with("region="))
            .expect("region suggestion");
        assert_eq!(region.score, 10);
        assert!(!region.stable);
        assert!(region.locator.contains("0.1,0.1"));
    }

    #[test]
    fn dynamic_heuristics_cover_common_shapes() {
        assert!(looks_dynamic("Order 12345"));
        assert!(looks_dynamic("550e8400-e29b-41d4-a716-446655440000"));
        assert!(looks_dynamic("12:30"));
        assert!(looks_dynamic("deadbeef42"));
        assert!(!looks_dynamic("Sign in"));
        assert!(!looks_dynamic("item-3"));
    }
}
