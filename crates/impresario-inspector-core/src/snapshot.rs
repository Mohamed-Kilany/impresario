//! Normalized, viewport-only snapshot model.

use std::collections::{BTreeMap, BTreeSet};

use impresario_core::platforms::Platform;
use impresario_locator::{
    Context, Locator, LocatorError, Node as LocatorNode, NodeMatch, Rect, Role, State, Suggestion,
    SynthesisOptions,
};
use serde::{Deserialize, Serialize};

/// A screen inset in logical units.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

/// How the platform moves focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FocusModel {
    #[default]
    Pointer,
    Dpad,
    Both,
}

/// The coordinate spaces a snapshot explicitly keeps separate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenInfo {
    /// Logical width (points / CSS px / dp).
    pub logical_w: f32,
    /// Logical height.
    pub logical_h: f32,
    /// Device pixel width.
    pub pixel_w: u32,
    /// Device pixel height.
    pub pixel_h: u32,
    /// Logical-to-pixel scale.
    pub scale: f32,
    /// Rotation in degrees (0, 90, 180, 270).
    pub rotation: u16,
    pub safe_insets: Insets,
    pub focus_model: FocusModel,
}

impl Default for ScreenInfo {
    fn default() -> Self {
        Self {
            logical_w: 0.0,
            logical_h: 0.0,
            pixel_w: 0,
            pixel_h: 0,
            scale: 1.0,
            rotation: 0,
            safe_insets: Insets::default(),
            focus_model: FocusModel::Pointer,
        }
    }
}

impl ScreenInfo {
    #[must_use]
    pub fn logical(size: (f32, f32)) -> Self {
        Self {
            logical_w: size.0,
            logical_h: size.1,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn viewport(&self) -> Rect {
        Rect::new(0.0, 0.0, self.logical_w, self.logical_h)
    }
}

/// Scroll axis of a scroll container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollAxis {
    Vertical,
    Horizontal,
    Both,
}

/// Scroll hints for pruned content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollInfo {
    pub can_scroll_forward: bool,
    pub can_scroll_backward: bool,
    pub axis: ScrollAxis,
}

impl ScrollInfo {
    #[must_use]
    pub fn vertical(can_scroll_forward: bool, can_scroll_backward: bool) -> Self {
        Self {
            can_scroll_forward,
            can_scroll_backward,
            axis: ScrollAxis::Vertical,
        }
    }

    #[must_use]
    pub fn horizontal(can_scroll_forward: bool, can_scroll_backward: bool) -> Self {
        Self {
            can_scroll_forward,
            can_scroll_backward,
            axis: ScrollAxis::Horizontal,
        }
    }
}

/// Boolean/tri-state element state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeState {
    pub enabled: bool,
    pub disabled: bool,
    pub focusable: bool,
    pub focused: bool,
    pub selected: bool,
    pub checked: Option<bool>,
    pub expanded: Option<bool>,
    pub pressed: Option<bool>,
    pub readonly: bool,
    pub required: bool,
    pub hidden_by_a11y: bool,
}

impl Default for NodeState {
    fn default() -> Self {
        Self {
            enabled: true,
            disabled: false,
            focusable: false,
            focused: false,
            selected: false,
            checked: None,
            expanded: None,
            pressed: None,
            readonly: false,
            required: false,
            hidden_by_a11y: false,
        }
    }
}

impl NodeState {
    #[must_use]
    pub fn get(&self, state: State) -> Option<bool> {
        match state {
            State::Enabled => Some(self.enabled && !self.disabled),
            State::Disabled => Some(self.disabled),
            State::Focused => Some(self.focused),
            State::Selected => Some(self.selected),
            State::Checked => self.checked,
            State::Expanded => self.expanded,
            State::Pressed => self.pressed,
            State::ReadOnly => Some(self.readonly),
            State::Required => Some(self.required),
        }
    }
}

/// One normalized element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotNode {
    /// Stable id across snapshots.
    pub id: u64,
    /// Parent id; `0` for a root.
    pub parent_id: u64,
    pub role: Role,
    pub name: String,
    pub text: String,
    #[serde(default)]
    pub text_truncated: bool,
    pub value: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub alt: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub test_id: Option<String>,
    #[serde(default)]
    pub native_id: Option<String>,
    pub bounds: Rect,
    /// Fraction of the node's area inside the viewport after clipping.
    pub visible_fraction: f32,
    #[serde(default)]
    pub state: NodeState,
    #[serde(default)]
    pub scroll: Option<ScrollInfo>,
    pub context: Context,
    #[serde(default)]
    pub source_path: String,
    #[serde(default)]
    pub raw: BTreeMap<String, String>,
    #[serde(default)]
    pub children_hidden_count: u32,
    /// True when the platform lets this node escape ancestor clips
    /// (`position:fixed`, overlays, `clipChildren=false`).
    #[serde(default)]
    pub escapes_clip: bool,
    /// True for status bars, navigation bars and keyboards.
    #[serde(default)]
    pub system_ui: bool,
    /// Secure text fields are masked unless a local reveal is requested.
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub css_selector: Option<String>,
    #[serde(default)]
    pub xpath: Option<String>,
    /// `e<id>` reference used by the compact view and MCP.
    #[serde(skip)]
    pub ref_id: String,
    /// Parent index derived from `parent_id`.
    #[serde(skip)]
    pub parent_index: Option<usize>,
}

impl SnapshotNode {
    #[must_use]
    pub fn new(id: u64, parent_id: u64, role: Role, name: impl Into<String>, bounds: Rect) -> Self {
        Self {
            id,
            parent_id,
            role,
            name: name.into(),
            text: String::new(),
            text_truncated: false,
            value: String::new(),
            label: String::new(),
            description: String::new(),
            placeholder: String::new(),
            alt: String::new(),
            title: String::new(),
            test_id: None,
            native_id: None,
            bounds,
            visible_fraction: 1.0,
            state: NodeState::default(),
            scroll: None,
            context: Context::Native,
            source_path: String::new(),
            raw: BTreeMap::new(),
            children_hidden_count: 0,
            escapes_clip: false,
            system_ui: false,
            secure: false,
            css_selector: None,
            xpath: None,
            ref_id: String::new(),
            parent_index: None,
        }
    }

    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    #[must_use]
    pub fn with_test_id(mut self, test_id: impl Into<String>) -> Self {
        self.test_id = Some(test_id.into());
        self
    }

    #[must_use]
    pub fn with_native_id(mut self, native_id: impl Into<String>) -> Self {
        self.native_id = Some(native_id.into());
        self
    }

    #[must_use]
    pub fn with_context(mut self, context: Context) -> Self {
        self.context = context;
        self
    }

    #[must_use]
    pub fn with_state(mut self, state: NodeState) -> Self {
        self.state = state;
        self
    }

    #[must_use]
    pub fn with_visible_fraction(mut self, visible_fraction: f32) -> Self {
        self.visible_fraction = visible_fraction.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn with_scroll(mut self, scroll: ScrollInfo) -> Self {
        self.scroll = Some(scroll);
        self
    }

    #[must_use]
    pub fn with_css_selector(mut self, selector: impl Into<String>) -> Self {
        self.css_selector = Some(selector.into());
        self
    }

    #[must_use]
    pub fn with_xpath(mut self, xpath: impl Into<String>) -> Self {
        self.xpath = Some(xpath.into());
        self
    }

    #[must_use]
    pub fn with_escapes_clip(mut self, escapes_clip: bool) -> Self {
        self.escapes_clip = escapes_clip;
        self
    }

    #[must_use]
    pub fn with_system_ui(mut self, system_ui: bool) -> Self {
        self.system_ui = system_ui;
        self
    }

    #[must_use]
    pub fn with_secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    /// The value safe to show in a snapshot; secure fields are masked.
    #[must_use]
    pub fn display_value(&self) -> &str {
        if self.secure {
            "***"
        } else {
            &self.value
        }
    }

    #[must_use]
    pub fn ref_id(&self) -> &str {
        &self.ref_id
    }
}

impl LocatorNode for SnapshotNode {
    fn id(&self) -> &str {
        &self.ref_id
    }

    fn role(&self) -> Role {
        self.role
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn placeholder(&self) -> &str {
        &self.placeholder
    }

    fn alt(&self) -> &str {
        &self.alt
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn test_id(&self) -> Option<&str> {
        self.test_id.as_deref()
    }

    fn native_id(&self) -> Option<&str> {
        self.native_id.as_deref()
    }

    fn state(&self, state: State) -> Option<bool> {
        self.state.get(state)
    }

    fn visible(&self) -> bool {
        self.visible_fraction > 0.0 && !self.state.hidden_by_a11y
    }

    fn rect(&self) -> Rect {
        self.bounds
    }

    fn context(&self) -> Context {
        self.context
    }

    fn parent(&self) -> Option<usize> {
        self.parent_index
    }

    fn css_selector(&self) -> Option<&str> {
        self.css_selector.as_deref()
    }

    fn xpath(&self) -> Option<&str> {
        self.xpath.as_deref()
    }
}

/// Why a snapshot is partial or degraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DegradedReason {
    Timeout,
    NotInspectable,
    FlutterSemanticsOff,
    Unsupported,
    Partial,
    Error,
}

/// One degradation entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Degraded {
    pub reason: DegradedReason,
    pub message: String,
    #[serde(default)]
    pub node_path: Option<String>,
    #[serde(default)]
    pub partial: bool,
}

/// Acquisition counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotStats {
    pub acquisition_ms: u64,
    pub node_count: usize,
    pub pruned_count: usize,
    pub payload_bytes: usize,
    pub delta_bytes: usize,
}

/// A normalized viewport snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub rev: u64,
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub screen: ScreenInfo,
    pub nodes: Vec<SnapshotNode>,
    #[serde(default)]
    pub degraded: Vec<Degraded>,
    #[serde(default)]
    pub stats: SnapshotStats,
    #[serde(default)]
    pub settled: bool,
    #[serde(skip)]
    id_index: BTreeMap<u64, usize>,
}

impl Snapshot {
    #[must_use]
    pub fn new(
        device_id: impl Into<String>,
        device_name: impl Into<String>,
        platform: Platform,
        screen: ScreenInfo,
        nodes: Vec<SnapshotNode>,
    ) -> Self {
        let mut snapshot = Self {
            rev: 1,
            device_id: device_id.into(),
            device_name: device_name.into(),
            platform,
            screen,
            nodes,
            degraded: Vec::new(),
            stats: SnapshotStats::default(),
            settled: true,
            id_index: BTreeMap::new(),
        };
        snapshot.rebuild();
        snapshot
    }

    #[must_use]
    pub fn with_rev(mut self, rev: u64) -> Self {
        self.rev = rev;
        self
    }

    #[must_use]
    pub fn with_settled(mut self, settled: bool) -> Self {
        self.settled = settled;
        self
    }

    #[must_use]
    pub fn with_degraded(mut self, degraded: Vec<Degraded>) -> Self {
        self.degraded = degraded;
        self
    }

    #[must_use]
    pub fn with_stats(mut self, stats: SnapshotStats) -> Self {
        self.stats = stats;
        self
    }

    /// Rebuild derived indexes after construction or deserialization.
    pub fn rebuild(&mut self) {
        self.id_index.clear();
        for (index, node) in self.nodes.iter_mut().enumerate() {
            node.ref_id = format!("e{}", node.id);
            node.parent_index = None;
            self.id_index.insert(node.id, index);
        }
        let index_map = self.id_index.clone();
        for node in &mut self.nodes {
            if node.parent_id != 0 {
                node.parent_index = index_map.get(&node.parent_id).copied();
            }
        }
        self.stats.node_count = self.nodes.len();
    }

    /// Validate stable ids and parent references.
    pub fn validate(&self) -> Result<(), SnapshotError> {
        let mut seen = BTreeSet::new();
        for node in &self.nodes {
            if !seen.insert(node.id) {
                return Err(SnapshotError::DuplicateId(node.id));
            }
            if node.parent_id != 0 && !seen.contains(&node.parent_id) {
                let parent_exists = self
                    .nodes
                    .iter()
                    .any(|candidate| candidate.id == node.parent_id);
                if !parent_exists {
                    return Err(SnapshotError::MissingParent {
                        id: node.id,
                        parent_id: node.parent_id,
                    });
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn find_index(&self, id: u64) -> Option<usize> {
        self.id_index.get(&id).copied()
    }

    #[must_use]
    pub fn find(&self, id: u64) -> Option<&SnapshotNode> {
        self.find_index(id).and_then(|index| self.nodes.get(index))
    }

    #[must_use]
    pub fn children_of(&self, id: u64) -> Vec<&SnapshotNode> {
        let Some(parent) = self.find_index(id) else {
            return Vec::new();
        };
        self.nodes
            .iter()
            .filter(|node| node.parent_index == Some(parent))
            .collect()
    }

    #[must_use]
    pub fn roots(&self) -> Vec<&SnapshotNode> {
        self.nodes
            .iter()
            .filter(|node| node.parent_index.is_none())
            .collect()
    }

    /// Resolve a locator against this snapshot with the shared matcher.
    pub fn resolve(&self, locator: &Locator) -> Result<Vec<NodeMatch>, LocatorError> {
        impresario_locator::resolve(
            locator,
            &self.nodes,
            (self.screen.logical_w, self.screen.logical_h),
        )
    }

    /// Resolve exactly one node.
    pub fn resolve_one(&self, locator: &Locator) -> Result<NodeMatch, LocatorError> {
        impresario_locator::resolve_one(
            locator,
            &self.nodes,
            (self.screen.logical_w, self.screen.logical_h),
        )
    }

    /// Ranked locators for one node.
    #[must_use]
    pub fn suggest(&self, id: u64, options: &SynthesisOptions) -> Vec<Suggestion> {
        let Some(index) = self.find_index(id) else {
            return Vec::new();
        };
        impresario_locator::synthesize(index, &self.nodes, options)
    }

    /// Serialize without the derived indexes.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize and rebuild derived indexes.
    pub fn from_json(source: &str) -> Result<Self, serde_json::Error> {
        let mut snapshot: Self = serde_json::from_str(source)?;
        snapshot.rebuild();
        Ok(snapshot)
    }

    /// True when both snapshots contain the same tree (metadata aside).
    #[must_use]
    pub fn same_nodes(&self, other: &Self) -> bool {
        self.nodes == other.nodes
    }
}

/// Snapshot validation errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    #[error("duplicate node id {0}")]
    DuplicateId(u64),
    #[error("node {id} references missing parent {parent_id}")]
    MissingParent { id: u64, parent_id: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_locator::parse;

    fn sample() -> Snapshot {
        let root = SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 100.0, 100.0));
        let button = SnapshotNode::new(
            2,
            1,
            Role::Button,
            "Save",
            Rect::new(10.0, 10.0, 30.0, 20.0),
        )
        .with_test_id("save")
        .with_text("Save");
        let input = SnapshotNode::new(
            3,
            1,
            Role::TextBox,
            "Email",
            Rect::new(10.0, 40.0, 80.0, 20.0),
        )
        .with_placeholder("Email")
        .with_secure(true);
        Snapshot::new(
            "web-1",
            "Chrome",
            Platform::Web,
            ScreenInfo::logical((100.0, 100.0)),
            vec![root, button, input],
        )
    }

    #[test]
    fn builds_indexes_and_parent_links() {
        let snapshot = sample();
        assert_eq!(snapshot.find_index(2), Some(1));
        assert_eq!(
            snapshot.find(2).map(|node| node.name.as_str()),
            Some("Save")
        );
        assert_eq!(snapshot.children_of(1).len(), 2);
        assert_eq!(snapshot.roots().len(), 1);
        assert_eq!(snapshot.find(2).map(SnapshotNode::ref_id), Some("e2"));
        assert!(snapshot.validate().is_ok());
    }

    #[test]
    fn resolves_with_the_shared_matcher() {
        let snapshot = sample();
        let locator = parse("testid=save").expect("parse");
        let matched = snapshot.resolve(&locator).expect("resolve");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched.first().map(|node| node.id.as_str()), Some("e2"));
    }

    #[test]
    fn suggests_locators_for_a_node() {
        let snapshot = sample();
        let suggestions = snapshot.suggest(2, &SynthesisOptions::default());
        assert!(suggestions
            .iter()
            .any(|suggestion| suggestion.locator == "testid=save"));
    }

    #[test]
    fn serializes_and_rebuilds() {
        let snapshot = sample();
        let json = snapshot.to_json().expect("json");
        let restored = Snapshot::from_json(&json).expect("restore");
        assert_eq!(restored.find_index(2), Some(1));
        assert_eq!(restored.nodes, snapshot.nodes);
    }

    #[test]
    fn secure_values_are_masked() {
        let snapshot = sample();
        let input = snapshot.find(3).expect("input");
        assert_eq!(input.display_value(), "***");
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let a = SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 1.0, 1.0));
        let b = SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 1.0, 1.0));
        let snapshot = Snapshot::new(
            "d",
            "d",
            Platform::Web,
            ScreenInfo::logical((10.0, 10.0)),
            vec![a, b],
        );
        assert_eq!(snapshot.validate(), Err(SnapshotError::DuplicateId(1)));
    }
}
