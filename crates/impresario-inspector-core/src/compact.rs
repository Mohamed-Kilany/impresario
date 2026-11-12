//! Token-efficient compact snapshot text (MCP `inspector_get_view`).
//!
//! The format mirrors Playwright's aria snapshot: one line per node, role,
//! quoted name, `[ref=eN]`, then state/test-id/scroll hints. The snapshot is
//! already viewport-pruned; this module only renders and truncates.
//!
//! ```text
//! device: tv-lg-01 (webos)  rev: 481  settled: true  screen: 1920x1080
//! - navigation "Main" [ref=e3]
//!   - link "Home" [ref=e4] [focused]
//! ```

use std::fmt::Write as _;

use impresario_locator::Rect;

use crate::snapshot::{ScrollAxis, Snapshot, SnapshotNode};

/// Rendering limits for the compact view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactOptions {
    /// Maximum number of element lines. The header is always rendered.
    pub max_nodes: usize,
    /// Maximum tree depth (roots are depth 0).
    pub max_depth: usize,
    /// Include `[testid=...]` when the node exposes one.
    pub include_test_id: bool,
}

impl Default for CompactOptions {
    fn default() -> Self {
        Self {
            max_nodes: 400,
            max_depth: 32,
            include_test_id: true,
        }
    }
}

/// Render the compact snapshot text.
#[must_use]
pub fn compact(snapshot: &Snapshot, options: &CompactOptions) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "device: {} ({})  rev: {}  settled: {}  screen: {}x{}",
        snapshot.device_name,
        snapshot.platform,
        snapshot.rev,
        snapshot.settled,
        dimension(snapshot.screen.logical_w),
        dimension(snapshot.screen.logical_h),
    );
    if !snapshot.degraded.is_empty() {
        let reasons: Vec<&str> = snapshot
            .degraded
            .iter()
            .map(|degraded| degraded.message.as_str())
            .filter(|message| !message.is_empty())
            .collect();
        if !reasons.is_empty() {
            let _ = writeln!(out, "degraded: {}", reasons.join("; "));
        }
    }

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); snapshot.nodes.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (index, node) in snapshot.nodes.iter().enumerate() {
        match node.parent_index {
            Some(parent) if parent < snapshot.nodes.len() && parent != index => {
                if let Some(list) = children.get_mut(parent) {
                    list.push(index);
                }
            }
            _ => roots.push(index),
        }
    }

    let mut rendered = 0usize;
    let mut truncated = false;
    let mut visited = vec![false; snapshot.nodes.len()];
    for root in roots {
        render_node(
            root,
            0,
            snapshot,
            &children,
            options,
            &mut out,
            &mut visited,
            &mut rendered,
            &mut truncated,
        );
    }

    if truncated {
        let skipped = snapshot.nodes.len().saturating_sub(rendered);
        let _ = write!(out, "... truncated: {skipped} more nodes\n");
    }
    out
}

/// Render the snapshot as JSON (`format=json` in the MCP tool).
pub fn compact_json(snapshot: &Snapshot) -> Result<String, serde_json::Error> {
    serde_json::to_string(snapshot)
}

#[allow(clippy::too_many_arguments)]
fn render_node(
    index: usize,
    depth: usize,
    snapshot: &Snapshot,
    children: &[Vec<usize>],
    options: &CompactOptions,
    out: &mut String,
    visited: &mut [bool],
    rendered: &mut usize,
    truncated: &mut bool,
) {
    if *rendered >= options.max_nodes {
        *truncated = true;
        return;
    }
    if depth > options.max_depth {
        *truncated = true;
        return;
    }
    let Some(node) = snapshot.nodes.get(index) else {
        return;
    };
    if let Some(seen) = visited.get_mut(index) {
        if *seen {
            return;
        }
        *seen = true;
    } else {
        return;
    }

    render_line(node, depth, options, out);
    *rendered += 1;

    let Some(child_indices) = children.get(index) else {
        return;
    };
    if child_indices.is_empty() {
        return;
    }
    if depth >= options.max_depth {
        *truncated = true;
        return;
    }
    for child in child_indices {
        render_node(
            *child,
            depth + 1,
            snapshot,
            children,
            options,
            out,
            visited,
            rendered,
            truncated,
        );
    }
}

fn render_line(node: &SnapshotNode, depth: usize, options: &CompactOptions, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str("- ");
    out.push_str(node.role.as_str());

    let label = if !node.name.trim().is_empty() {
        node.name.as_str()
    } else {
        node.text.as_str()
    };
    if !label.trim().is_empty() {
        out.push(' ');
        out.push_str(&quote(label));
        if node.text_truncated {
            out.push_str(" [text-truncated]");
        }
    }

    out.push_str(" [ref=");
    if node.ref_id.is_empty() {
        let _ = write!(out, "e{}", node.id);
    } else {
        out.push_str(&node.ref_id);
    }
    out.push(']');

    if node.state.focused {
        out.push_str(" [focused]");
    }
    if node.state.disabled || !node.state.enabled {
        out.push_str(" [disabled]");
    }
    if node.state.selected {
        out.push_str(" [selected]");
    }
    match node.state.checked {
        Some(true) => out.push_str(" [checked]"),
        Some(false) => out.push_str(" [checked=false]"),
        None => {}
    }
    match node.state.expanded {
        Some(true) => out.push_str(" [expanded]"),
        Some(false) => out.push_str(" [expanded=false]"),
        None => {}
    }
    match node.state.pressed {
        Some(true) => out.push_str(" [pressed]"),
        Some(false) => out.push_str(" [pressed=false]"),
        None => {}
    }
    if node.state.readonly {
        out.push_str(" [readonly]");
    }
    if node.state.required {
        out.push_str(" [required]");
    }
    if node.state.hidden_by_a11y {
        out.push_str(" [hidden]");
    }
    if node.secure {
        out.push_str(" [secure]");
    }
    if node.system_ui {
        out.push_str(" [system]");
    }
    if options.include_test_id {
        if let Some(test_id) = node.test_id.as_deref().filter(|value| !value.is_empty()) {
            out.push_str(" [testid=");
            out.push_str(test_id);
            out.push(']');
        }
    }
    if let Some(scroll) = node.scroll {
        let mut hints: Vec<&str> = Vec::new();
        match scroll.axis {
            ScrollAxis::Vertical => {
                if scroll.can_scroll_backward {
                    hints.push("more above");
                }
                if scroll.can_scroll_forward {
                    hints.push("more below");
                }
            }
            ScrollAxis::Horizontal => {
                if scroll.can_scroll_backward {
                    hints.push("more left");
                }
                if scroll.can_scroll_forward {
                    hints.push("more right");
                }
            }
            ScrollAxis::Both => {
                if scroll.can_scroll_backward {
                    hints.push("more above/left");
                }
                if scroll.can_scroll_forward {
                    hints.push("more below/right");
                }
            }
        }
        if !hints.is_empty() {
            out.push_str(" [scroll: ");
            out.push_str(&hints.join(", "));
            if node.children_hidden_count > 0 {
                let _ = write!(out, " ({})", node.children_hidden_count);
            }
            out.push(']');
        }
    }
    out.push('\n');
}

fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len().saturating_add(2));
    out.push('"');
    let mut count = 0usize;
    for character in value.chars() {
        if count >= 160 {
            out.push('\u{2026}');
            break;
        }
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' | '\t' => out.push(' '),
            other => out.push(other),
        }
        count += 1;
    }
    out.push('"');
    out
}

fn dimension(value: f32) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.05 {
        format!("{rounded:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// Whether a rectangle is entirely inside the snapshot viewport.
#[must_use]
pub fn fully_in_viewport(rect: Rect, snapshot: &Snapshot) -> bool {
    let viewport = snapshot.screen.viewport();
    rect.x >= viewport.x
        && rect.y >= viewport.y
        && rect.x + rect.w <= viewport.x + viewport.w
        && rect.y + rect.h <= viewport.y + viewport.h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{NodeState, ScreenInfo, ScrollInfo, SnapshotNode};
    use impresario_core::platforms::Platform;
    use impresario_locator::Role;

    fn sample() -> Snapshot {
        let focused = NodeState {
            focused: true,
            ..NodeState::default()
        };
        let root = SnapshotNode::new(
            1,
            0,
            Role::Navigation,
            "Main",
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
        );
        let home = SnapshotNode::new(2, 1, Role::Link, "Home", Rect::new(10.0, 10.0, 100.0, 30.0))
            .with_state(focused);
        let movies = SnapshotNode::new(
            3,
            1,
            Role::Link,
            "Movies",
            Rect::new(120.0, 10.0, 100.0, 30.0),
        );
        let mut list = SnapshotNode::new(4, 0, Role::List, "", Rect::new(0.0, 50.0, 1920.0, 500.0))
            .with_scroll(ScrollInfo::vertical(true, false));
        list.children_hidden_count = 14;
        let item = SnapshotNode::new(
            5,
            4,
            Role::ListItem,
            "",
            Rect::new(0.0, 50.0, 1920.0, 100.0),
        );
        let play = SnapshotNode::new(
            6,
            5,
            Role::Button,
            "Play Dune",
            Rect::new(10.0, 60.0, 200.0, 40.0),
        )
        .with_test_id("card.play");
        Snapshot::new(
            "tv-1",
            "tv-lg-01",
            Platform::WebOs,
            ScreenInfo::logical((1920.0, 1080.0)),
            vec![root, home, movies, list, item, play],
        )
        .with_rev(481)
    }

    #[test]
    fn renders_header_tree_refs_states_and_scroll_hints() {
        let text = compact(&sample(), &CompactOptions::default());
        assert!(text
            .starts_with("device: tv-lg-01 (webos)  rev: 481  settled: true  screen: 1920x1080\n"));
        assert!(text.contains("- navigation \"Main\" [ref=e1]\n"));
        assert!(text.contains("  - link \"Home\" [ref=e2] [focused]\n"));
        assert!(text.contains("- list [ref=e4] [scroll: more below (14)]\n"));
        assert!(text.contains("    - button \"Play Dune\" [ref=e6] [testid=card.play]\n"));
    }

    #[test]
    fn max_nodes_truncates_and_reports_the_remainder() {
        let options = CompactOptions {
            max_nodes: 3,
            ..CompactOptions::default()
        };
        let text = compact(&sample(), &options);
        assert!(text.contains("truncated: 3 more nodes"));
        assert_eq!(text.matches(" [ref=").count(), 3);
    }

    #[test]
    fn max_depth_truncates_deep_trees() {
        let options = CompactOptions {
            max_depth: 1,
            ..CompactOptions::default()
        };
        let text = compact(&sample(), &options);
        assert!(!text.contains("button \"Play Dune\""));
        assert!(text.contains("truncated:"));
    }

    #[test]
    fn quotes_and_escapes_names() {
        let node = SnapshotNode::new(
            1,
            0,
            Role::Text,
            "Say \"hi\"\nnow",
            Rect::new(0.0, 0.0, 10.0, 10.0),
        );
        let snapshot = Snapshot::new(
            "d",
            "d",
            Platform::Web,
            ScreenInfo::logical((10.0, 10.0)),
            vec![node],
        );
        let text = compact(&snapshot, &CompactOptions::default());
        assert!(text.contains("- text \"Say \\\"hi\\\" now\" [ref=e1]"));
    }

    #[test]
    fn json_format_round_trips() {
        let snapshot = sample();
        let json = compact_json(&snapshot).expect("json");
        let restored = Snapshot::from_json(&json).expect("restore");
        assert_eq!(restored.rev, snapshot.rev);
        assert_eq!(restored.nodes.len(), snapshot.nodes.len());
    }

    #[test]
    fn viewport_check_handles_partial_nodes() {
        let snapshot = sample();
        assert!(fully_in_viewport(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            &snapshot
        ));
        assert!(!fully_in_viewport(
            Rect::new(1900.0, 0.0, 100.0, 10.0),
            &snapshot
        ));
    }
}
