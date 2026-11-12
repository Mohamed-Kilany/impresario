//! Locator AST and small constructors.

use serde::{Deserialize, Serialize};

use crate::role::Role;
use crate::text::NameFilter;

/// The normalized context a node belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Context {
    Native,
    WebView,
    Flutter,
    Web,
}

impl Context {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Context::Native => "NATIVE",
            Context::WebView => "WEBVIEW",
            Context::Flutter => "FLUTTER",
            Context::Web => "WEB",
        }
    }
}

/// A boolean element state a locator can require.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Enabled,
    Disabled,
    Focused,
    Selected,
    Checked,
    Expanded,
    Pressed,
    ReadOnly,
    Required,
}

impl State {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            State::Enabled => "enabled",
            State::Disabled => "disabled",
            State::Focused => "focused",
            State::Selected => "selected",
            State::Checked => "checked",
            State::Expanded => "expanded",
            State::Pressed => "pressed",
            State::ReadOnly => "readonly",
            State::Required => "required",
        }
    }
}

/// One `[state=true|false]` requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateFilter {
    pub state: State,
    pub value: bool,
}

/// A normalized screen-relative rectangle in logical units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    #[must_use]
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    #[must_use]
    pub fn center(self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    #[must_use]
    pub fn area(self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = (self.x + self.w).min(other.x + other.w);
        let bottom = (self.y + self.h).min(other.y + other.h);
        Self {
            x,
            y,
            w: (right - x).max(0.0),
            h: (bottom - y).max(0.0),
        }
    }

    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.intersection(other).area() > 0.0
    }
}

/// A normalized visual-fallback region (`region=x,y,w,h`, all in `0..=1`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Region {
    /// Scale the normalized region onto a screen.
    #[must_use]
    pub fn to_rect(self, screen_width: f32, screen_height: f32) -> Rect {
        Rect {
            x: self.x * screen_width,
            y: self.y * screen_height,
            w: self.w * screen_width,
            h: self.h * screen_height,
        }
    }
}

/// What a single `>>` step selects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    Role(Role),
    Text(NameFilter),
    Label(NameFilter),
    Placeholder(NameFilter),
    Alt(NameFilter),
    Title(NameFilter),
    TestId(String),
    Id(String),
    Css(String),
    XPath(String),
    Focused,
    Region(Region),
}

impl Kind {
    /// The canonical keyword.
    #[must_use]
    pub fn keyword(&self) -> &'static str {
        match self {
            Kind::Role(_) => "role",
            Kind::Text(_) => "text",
            Kind::Label(_) => "label",
            Kind::Placeholder(_) => "placeholder",
            Kind::Alt(_) => "alt",
            Kind::Title(_) => "title",
            Kind::TestId(_) => "testid",
            Kind::Id(_) => "id",
            Kind::Css(_) => "css",
            Kind::XPath(_) => "xpath",
            Kind::Focused => "focused",
            Kind::Region(_) => "region",
        }
    }

    /// The name/text filter carried by this kind, when it has one.
    #[must_use]
    pub fn filter(&self) -> Option<&NameFilter> {
        match self {
            Kind::Text(filter)
            | Kind::Label(filter)
            | Kind::Placeholder(filter)
            | Kind::Alt(filter)
            | Kind::Title(filter) => Some(filter),
            _ => None,
        }
    }

    /// A mutable name/text filter, when this kind has one.
    pub fn filter_mut(&mut self) -> Option<&mut NameFilter> {
        match self {
            Kind::Text(filter)
            | Kind::Label(filter)
            | Kind::Placeholder(filter)
            | Kind::Alt(filter)
            | Kind::Title(filter) => Some(filter),
            _ => None,
        }
    }
}

/// One step of a `>>` chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimpleSelector {
    pub kind: Kind,
    /// An additional accessible-name filter (`[name="..."]`).
    pub name: Option<NameFilter>,
    pub states: Vec<StateFilter>,
    /// Descendants that must match.
    pub has: Vec<Locator>,
    /// Descendants that must not match.
    pub has_not: Vec<Locator>,
}

impl SimpleSelector {
    #[must_use]
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            name: None,
            states: Vec::new(),
            has: Vec::new(),
            has_not: Vec::new(),
        }
    }
}

/// A complete locator expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Locator {
    /// `within(...)` scopes; every scope must contain the first step.
    pub within: Vec<Locator>,
    /// `>>` chain; never empty for a parsed locator.
    pub steps: Vec<SimpleSelector>,
    pub nth: Option<usize>,
    pub first: bool,
    pub last: bool,
    /// Defaults to `true`; `[visible=false]` includes hidden nodes.
    pub visible: bool,
    pub context: Option<Context>,
}

impl Locator {
    #[must_use]
    pub fn simple(kind: Kind) -> Self {
        Self {
            within: Vec::new(),
            steps: vec![SimpleSelector::new(kind)],
            nth: None,
            first: false,
            last: false,
            visible: true,
            context: None,
        }
    }

    #[must_use]
    pub fn with_name(mut self, filter: NameFilter) -> Self {
        if let Some(step) = self.steps.first_mut() {
            step.name = Some(filter);
        }
        self
    }

    #[must_use]
    pub fn with_state(mut self, state: State, value: bool) -> Self {
        if let Some(step) = self.steps.first_mut() {
            step.states.push(StateFilter { state, value });
        }
        self
    }

    #[must_use]
    pub fn with_nth(mut self, nth: usize) -> Self {
        self.nth = Some(nth);
        self
    }

    #[must_use]
    pub fn with_first(mut self) -> Self {
        self.first = true;
        self.nth = None;
        self.last = false;
        self
    }

    #[must_use]
    pub fn with_last(mut self) -> Self {
        self.last = true;
        self.nth = None;
        self.first = false;
        self
    }

    #[must_use]
    pub fn with_context(mut self, context: Context) -> Self {
        self.context = Some(context);
        self
    }

    /// True when the pure snapshot resolver can evaluate this locator.
    /// CSS/XPath need the live platform engine and are reported as
    /// unsupported by [`crate::resolve`].
    #[must_use]
    pub fn is_snapshot_resolvable(&self) -> bool {
        self.within.iter().all(Locator::is_snapshot_resolvable)
            && self.steps.iter().all(|step| {
                !matches!(step.kind, Kind::Css(_) | Kind::XPath(_))
                    && step.has.iter().all(Locator::is_snapshot_resolvable)
                    && step.has_not.iter().all(Locator::is_snapshot_resolvable)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_geometry_is_clamped() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        let intersection = a.intersection(b);
        assert_eq!(intersection, Rect::new(5.0, 5.0, 5.0, 5.0));
        assert!(a.intersects(b));
        assert!(!a.intersects(Rect::new(20.0, 20.0, 1.0, 1.0)));
    }

    #[test]
    fn region_scales_to_the_screen() {
        let region = Region {
            x: 0.5,
            y: 0.5,
            w: 0.5,
            h: 0.5,
        };
        assert_eq!(
            region.to_rect(100.0, 200.0),
            Rect::new(50.0, 100.0, 50.0, 100.0)
        );
    }

    #[test]
    fn constructors_keep_the_first_step() {
        let locator = Locator::simple(Kind::Role(Role::Button))
            .with_name(NameFilter::contains("Save", false))
            .with_state(State::Enabled, true)
            .with_nth(2);
        assert_eq!(locator.steps.len(), 1);
        assert_eq!(locator.nth, Some(2));
        assert_eq!(
            locator.steps.first().and_then(|step| step.name.as_ref()),
            Some(&NameFilter::contains("Save", false))
        );
    }
}
