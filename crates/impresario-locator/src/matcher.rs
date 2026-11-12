//! Pure snapshot matcher.
//!
//! The matcher is the single resolver used by the inspector and by
//! snapshot-backed runner verification. It works on a flat, viewport-only
//! tree: every node knows its parent index, so descendant scoping, `has`,
//! `has-not` and `within` are all evaluated without a second engine.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ast::{Context, Kind, Locator, Rect, SimpleSelector, State};
use crate::parse::LocatorError;
use crate::role::Role;

/// One node of a snapshot tree, as seen by the matcher.
///
/// Implementations only need stable ids, roles and geometry; every other
/// accessor has a conservative default so tests and platform adapters stay
/// small.
pub trait Node {
    /// Stable snapshot id (not the platform element id).
    fn id(&self) -> &str;

    /// Normalized role.
    fn role(&self) -> Role;

    /// Accessible name.
    fn name(&self) -> &str {
        ""
    }

    /// Visible text content.
    fn text(&self) -> &str {
        ""
    }

    /// Accessible label / content description.
    fn label(&self) -> &str {
        ""
    }

    /// Placeholder text.
    fn placeholder(&self) -> &str {
        ""
    }

    /// Alternative text.
    fn alt(&self) -> &str {
        ""
    }

    /// Title / tooltip.
    fn title(&self) -> &str {
        ""
    }

    /// Test id / accessibility identifier.
    fn test_id(&self) -> Option<&str> {
        None
    }

    /// Platform element id (Android resource id, DOM id, ...).
    fn native_id(&self) -> Option<&str> {
        None
    }

    /// A boolean state, when the platform exposes it.
    fn state(&self, _state: State) -> Option<bool> {
        None
    }

    /// Whether the node is visible in the viewport.
    fn visible(&self) -> bool {
        true
    }

    /// Screen-relative logical rectangle.
    fn rect(&self) -> Rect {
        Rect::new(0.0, 0.0, 0.0, 0.0)
    }

    /// The normalized context this node belongs to.
    fn context(&self) -> Context {
        Context::Native
    }

    /// Parent index in the flat snapshot array.
    fn parent(&self) -> Option<usize> {
        None
    }

    /// A CSS selector for this node, when the platform can provide one.
    fn css_selector(&self) -> Option<&str> {
        None
    }

    /// An XPath for this node, when the platform can provide one.
    fn xpath(&self) -> Option<&str> {
        None
    }
}

/// One matched node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeMatch {
    /// Index in the flat snapshot array.
    pub index: usize,
    /// Stable snapshot id.
    pub id: String,
    pub role: Role,
    pub name: String,
    pub rect: Rect,
    pub visible: bool,
}

/// Resolve every match of `locator` in `nodes`.
///
/// `screen` is `(width, height)` in the same logical units as [`Node::rect`];
/// it is only used by `region=` locators.
pub fn resolve<N: Node>(
    locator: &Locator,
    nodes: &[N],
    screen: (f32, f32),
) -> Result<Vec<NodeMatch>, LocatorError> {
    let resolver = Resolver::new(nodes, screen);
    let indices = resolver.resolve_locator(locator, None)?;
    Ok(indices
        .into_iter()
        .filter_map(|index| resolver.node_match(index))
        .collect())
}

/// Resolve exactly one match.
///
/// Returns [`LocatorError::NotFound`] for zero matches and
/// [`LocatorError::Ambiguous`] for more than one.
pub fn resolve_one<N: Node>(
    locator: &Locator,
    nodes: &[N],
    screen: (f32, f32),
) -> Result<NodeMatch, LocatorError> {
    let matches = resolve(locator, nodes, screen)?;
    match matches.len() {
        0 => Err(LocatorError::NotFound),
        1 => matches.into_iter().next().ok_or(LocatorError::NotFound),
        count => Err(LocatorError::Ambiguous(count)),
    }
}

struct Resolver<'a, N: Node> {
    nodes: &'a [N],
    children: Vec<Vec<usize>>,
    screen: (f32, f32),
}

impl<'a, N: Node> Resolver<'a, N> {
    fn new(nodes: &'a [N], screen: (f32, f32)) -> Self {
        let mut children = vec![Vec::new(); nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            let Some(parent) = node.parent() else {
                continue;
            };
            if parent == index {
                continue;
            }
            if let Some(list) = children.get_mut(parent) {
                list.push(index);
            }
        }
        Self {
            nodes,
            children,
            screen,
        }
    }

    fn node_match(&self, index: usize) -> Option<NodeMatch> {
        let node = self.nodes.get(index)?;
        Some(NodeMatch {
            index,
            id: node.id().to_string(),
            role: node.role(),
            name: node.name().to_string(),
            rect: node.rect(),
            visible: node.visible(),
        })
    }

    /// Strict descendants of `root`, in document order.
    fn descendants(&self, root: usize) -> Vec<usize> {
        let mut seen = vec![false; self.nodes.len()];
        if let Some(flag) = seen.get_mut(root) {
            *flag = true;
        }
        let mut stack: Vec<usize> = self
            .children
            .get(root)
            .map(|list| list.iter().rev().copied().collect())
            .unwrap_or_default();
        let mut out = Vec::new();
        while let Some(index) = stack.pop() {
            if let Some(flag) = seen.get_mut(index) {
                if *flag {
                    continue;
                }
                *flag = true;
            } else {
                continue;
            }
            out.push(index);
            if let Some(list) = self.children.get(index) {
                stack.extend(list.iter().rev().copied());
            }
        }
        out.sort_unstable();
        out
    }

    fn resolve_locator(
        &self,
        locator: &Locator,
        scope: Option<usize>,
    ) -> Result<Vec<usize>, LocatorError> {
        if let Some(keyword) = unsupported_keyword(locator) {
            return Err(LocatorError::Unsupported(keyword.to_string()));
        }
        let Some(first_step) = locator.steps.first() else {
            return Err(LocatorError::Empty);
        };

        let mut base: Vec<usize> = match scope {
            Some(root) => self.descendants(root),
            None => (0..self.nodes.len()).collect(),
        };

        if !locator.within.is_empty() {
            let mut allowed: Option<BTreeSet<usize>> = None;
            for within in &locator.within {
                let scope_matches = self.resolve_locator(within, scope)?;
                let mut descendants = BTreeSet::new();
                for root in scope_matches {
                    descendants.extend(self.descendants(root));
                }
                allowed = Some(match allowed {
                    Some(previous) => previous.intersection(&descendants).copied().collect(),
                    None => descendants,
                });
            }
            if let Some(allowed) = allowed {
                base.retain(|index| allowed.contains(index));
            }
        }

        let mut current = self.filter_step(&base, first_step, locator)?;
        for step in locator.steps.iter().skip(1) {
            let mut next = BTreeSet::new();
            for index in &current {
                next.extend(self.descendants(*index));
            }
            let next: Vec<usize> = next.into_iter().collect();
            current = self.filter_step(&next, step, locator)?;
        }

        if let Some(nth) = locator.nth {
            current = current.get(nth).copied().into_iter().collect();
        } else if locator.first {
            current = current.first().copied().into_iter().collect();
        } else if locator.last {
            current = current.last().copied().into_iter().collect();
        }
        Ok(current)
    }

    fn filter_step(
        &self,
        candidates: &[usize],
        step: &SimpleSelector,
        locator: &Locator,
    ) -> Result<Vec<usize>, LocatorError> {
        let mut out = Vec::new();
        for &index in candidates {
            let Some(node) = self.nodes.get(index) else {
                continue;
            };
            if locator.visible && !node.visible() {
                continue;
            }
            if let Some(context) = locator.context {
                if node.context() != context {
                    continue;
                }
            }
            if self.matches_step(index, node, step)? {
                out.push(index);
            }
        }
        Ok(out)
    }

    fn matches_step(
        &self,
        index: usize,
        node: &N,
        step: &SimpleSelector,
    ) -> Result<bool, LocatorError> {
        if !self.matches_kind(node, &step.kind)? {
            return Ok(false);
        }
        if let Some(filter) = &step.name {
            if !filter.matches(node.name()) {
                return Ok(false);
            }
        }
        for filter in &step.states {
            let actual = node.state(filter.state).unwrap_or(false);
            if actual != filter.value {
                return Ok(false);
            }
        }
        for nested in &step.has {
            if self.resolve_locator(nested, Some(index))?.is_empty() {
                return Ok(false);
            }
        }
        for nested in &step.has_not {
            if !self.resolve_locator(nested, Some(index))?.is_empty() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn matches_kind(&self, node: &N, kind: &Kind) -> Result<bool, LocatorError> {
        match kind {
            Kind::Role(role) => Ok(node.role() == *role),
            Kind::Text(filter) => Ok(filter.matches(node.text())),
            Kind::Label(filter) => Ok(filter.matches(node.label())),
            Kind::Placeholder(filter) => Ok(filter.matches(node.placeholder())),
            Kind::Alt(filter) => Ok(filter.matches(node.alt())),
            Kind::Title(filter) => Ok(filter.matches(node.title())),
            Kind::TestId(value) => Ok(node
                .test_id()
                .is_some_and(|candidate| candidate == value.as_str())),
            Kind::Id(value) => Ok(node
                .native_id()
                .is_some_and(|candidate| candidate == value.as_str())),
            Kind::Css(_) | Kind::XPath(_) => {
                Err(LocatorError::Unsupported(kind.keyword().to_string()))
            }
            Kind::Focused => Ok(node.state(State::Focused).unwrap_or(false)),
            Kind::Region(region) => {
                let target = region.to_rect(self.screen.0, self.screen.1);
                Ok(target.area() > 0.0 && node.rect().intersects(target))
            }
        }
    }
}

fn unsupported_keyword(locator: &Locator) -> Option<&'static str> {
    for scope in &locator.within {
        if let Some(keyword) = unsupported_keyword(scope) {
            return Some(keyword);
        }
    }
    for step in &locator.steps {
        match &step.kind {
            Kind::Css(_) => return Some("css"),
            Kind::XPath(_) => return Some("xpath"),
            _ => {}
        }
        for nested in step.has.iter().chain(step.has_not.iter()) {
            if let Some(keyword) = unsupported_keyword(nested) {
                return Some(keyword);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use crate::text::NameFilter;

    #[derive(Debug)]
    struct MockNode {
        id: &'static str,
        role: Role,
        name: &'static str,
        text: &'static str,
        test_id: Option<&'static str>,
        native_id: Option<&'static str>,
        parent: Option<usize>,
        visible: bool,
        rect: Rect,
        context: Context,
        states: Vec<(State, bool)>,
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

        fn state(&self, state: State) -> Option<bool> {
            self.states
                .iter()
                .find(|(candidate, _)| *candidate == state)
                .map(|(_, value)| *value)
        }

        fn visible(&self) -> bool {
            self.visible
        }

        fn rect(&self) -> Rect {
            self.rect
        }

        fn context(&self) -> Context {
            self.context
        }

        fn parent(&self) -> Option<usize> {
            self.parent
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
            parent,
            visible: true,
            rect: Rect::new(500.0, 500.0, 10.0, 10.0),
            context: Context::Web,
            states: Vec::new(),
        }
    }

    fn tree() -> Vec<MockNode> {
        let mut root = node("root", Role::Generic, "", None);
        root.context = Context::Native;
        let mut dialog = node("dialog", Role::Dialog, "Delete", Some(0));
        dialog.context = Context::Native;
        let mut confirm = node("confirm", Role::Button, "Delete", Some(1));
        confirm.test_id = Some("delete.confirm");
        confirm.native_id = Some("btn_delete");
        confirm.states = vec![(State::Focused, true), (State::Disabled, false)];
        confirm.rect = Rect::new(10.0, 10.0, 80.0, 30.0);
        let cancel = node("cancel", Role::Button, "Cancel", Some(1));
        let outside = node("outside", Role::Button, "Delete", Some(0));
        let list = node("list", Role::List, "Items", Some(0));
        let item = node("item", Role::ListItem, "One", Some(5));
        let mut hidden = node("hidden", Role::Button, "Delete", Some(5));
        hidden.visible = false;
        let mut other = node("other", Role::Button, "Other", Some(5));
        other.text = "Other text";
        vec![
            root, dialog, confirm, cancel, outside, list, item, hidden, other,
        ]
    }

    fn resolve_text(text: &str) -> Result<Vec<usize>, LocatorError> {
        let locator = parse(text)?;
        let nodes = tree();
        Ok(resolve(&locator, &nodes, (100.0, 100.0))?
            .into_iter()
            .map(|matched| matched.index)
            .collect())
    }

    #[test]
    fn resolves_role_name_states_and_visibility() {
        assert_eq!(
            resolve_text("role=button[name=\"Delete\"]").expect("resolve"),
            vec![2, 4]
        );
        assert_eq!(
            resolve_text("role=button[name=\"Delete\"][disabled=false]").expect("resolve"),
            vec![2, 4]
        );
        assert_eq!(
            resolve_text("role=button[name=\"Delete\"][disabled=true]").expect("resolve"),
            Vec::<usize>::new()
        );
        assert_eq!(resolve_text("focused").expect("resolve"), vec![2]);
        assert_eq!(
            resolve_text("role=button[name=\"Delete\"][visible=false]").expect("resolve"),
            vec![2, 4, 7]
        );
    }

    #[test]
    fn resolves_test_ids_native_ids_and_text() {
        assert_eq!(
            resolve_text("testid=delete.confirm").expect("resolve"),
            vec![2]
        );
        assert_eq!(resolve_text("id=btn_delete").expect("resolve"), vec![2]);
        assert_eq!(
            resolve_text("text=\"Other text\"[exact]").expect("resolve"),
            vec![8]
        );
        assert_eq!(resolve_text("text=Delete").expect("resolve"), vec![2, 4]);
    }

    #[test]
    fn resolves_within_and_chains() {
        assert_eq!(
            resolve_text("within(role=dialog[name=\"Delete\"]) >> role=button[name=\"Delete\"]")
                .expect("resolve"),
            vec![2]
        );
        assert_eq!(
            resolve_text("role=list >> role=button").expect("resolve"),
            vec![8]
        );
    }

    #[test]
    fn resolves_has_and_has_not() {
        assert_eq!(
            resolve_text("role=list[has=(role=listitem)]").expect("resolve"),
            vec![5]
        );
        assert_eq!(
            resolve_text("role=list[has-not=(role=listitem)]").expect("resolve"),
            Vec::<usize>::new()
        );
        assert_eq!(
            resolve_text("role=button[has-not=(role=image)]").expect("resolve"),
            vec![2, 3, 4, 8]
        );
    }

    #[test]
    fn applies_nth_first_and_last() {
        assert_eq!(
            resolve_text("role=button[name=\"Delete\"][nth=1]").expect("resolve"),
            vec![4]
        );
        assert_eq!(
            resolve_text("role=button[name=\"Delete\"][first]").expect("resolve"),
            vec![2]
        );
        assert_eq!(
            resolve_text("role=button[name=\"Delete\"][last]").expect("resolve"),
            vec![4]
        );
    }

    #[test]
    fn filters_context_and_regions() {
        assert_eq!(
            resolve_text("role=button[context=WEB]").expect("resolve"),
            vec![2, 3, 4, 8]
        );
        assert_eq!(
            resolve_text("role=button[context=NATIVE]").expect("resolve"),
            Vec::<usize>::new()
        );
        assert_eq!(
            resolve_text("region=0.05,0.05,0.2,0.2").expect("resolve"),
            vec![2]
        );
        assert_eq!(
            resolve_text("region=0.9,0.9,0.05,0.05").expect("resolve"),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn resolve_one_reports_not_found_and_ambiguity() {
        let nodes = tree();
        let unique = parse("testid=delete.confirm").expect("parse");
        assert_eq!(
            resolve_one(&unique, &nodes, (100.0, 100.0))
                .expect("unique")
                .index,
            2
        );
        let ambiguous = parse("role=button[name=\"Delete\"]").expect("parse");
        assert_eq!(
            resolve_one(&ambiguous, &nodes, (100.0, 100.0)),
            Err(LocatorError::Ambiguous(2))
        );
        let missing = parse("role=button[name=\"Nope\"]").expect("parse");
        assert_eq!(
            resolve_one(&missing, &nodes, (100.0, 100.0)),
            Err(LocatorError::NotFound)
        );
    }

    #[test]
    fn css_and_xpath_are_reported_as_unsupported() {
        let nodes = tree();
        for text in ["css=.button", "xpath=//button"] {
            let locator = parse(text).expect("parse");
            let error = resolve(&locator, &nodes, (100.0, 100.0)).expect_err("unsupported");
            assert!(matches!(error, LocatorError::Unsupported(_)));
        }
    }

    #[test]
    fn name_filters_apply_to_roles() {
        let locator =
            Locator::simple(Kind::Role(Role::Button)).with_name(NameFilter::contains("can", true));
        let nodes = tree();
        let matches = resolve(&locator, &nodes, (100.0, 100.0)).expect("resolve");
        assert_eq!(
            matches
                .into_iter()
                .map(|matched| matched.index)
                .collect::<Vec<_>>(),
            vec![3]
        );
    }
}
