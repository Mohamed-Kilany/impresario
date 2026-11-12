//! Server-side pruning geometry and safety pass.
//!
//! Agents prune before transport; this pass keeps the invariant true for
//! adapters that cannot prune perfectly (and for recorded fixtures). It also
//! computes `visible_fraction`, keeps focused nodes and their ancestor chain,
//! and records `children_hidden_count` hints.

use std::collections::BTreeMap;

use impresario_locator::Rect;

use crate::snapshot::{ScreenInfo, Snapshot};

/// Pruning knobs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PruningOptions {
    /// Viewport in logical units.
    pub viewport: Rect,
    /// Minimum fraction of the node's area inside the clipped viewport.
    /// `0.0` means any non-zero intersection.
    pub min_visible_fraction: f32,
    /// Include status bars, navigation bars and keyboards.
    pub include_system_ui: bool,
    /// Always keep the focused node and its ancestor chain.
    pub always_include_focused: bool,
}

impl PruningOptions {
    #[must_use]
    pub fn for_screen(screen: &ScreenInfo) -> Self {
        Self {
            viewport: screen.viewport(),
            min_visible_fraction: 0.0,
            include_system_ui: false,
            always_include_focused: true,
        }
    }
}

impl Default for PruningOptions {
    fn default() -> Self {
        Self {
            viewport: Rect::new(0.0, 0.0, 0.0, 0.0),
            min_visible_fraction: 0.0,
            include_system_ui: false,
            always_include_focused: true,
        }
    }
}

/// What pruning removed and why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub kept: usize,
    pub removed: usize,
    pub hidden_counts: BTreeMap<u64, u32>,
}

/// Prune `snapshot` in place.
pub fn prune_snapshot(snapshot: &mut Snapshot, options: PruningOptions) -> PruneReport {
    snapshot.rebuild();
    let old_len = snapshot.nodes.len();
    if old_len == 0 {
        return PruneReport::default();
    }

    let mut fractions = vec![0.0f32; old_len];
    for (index, node) in snapshot.nodes.iter().enumerate() {
        let clip = clip_rect(&snapshot.nodes, index, options.viewport);
        if let Some(slot) = fractions.get_mut(index) {
            *slot = visible_fraction(node.bounds, clip);
        }
    }

    let mut keep = vec![false; old_len];
    for (index, node) in snapshot.nodes.iter().enumerate() {
        if node.system_ui && !options.include_system_ui {
            continue;
        }
        if node.state.hidden_by_a11y && !node.state.focused {
            continue;
        }
        if node.bounds.area() <= 0.0 {
            continue;
        }
        if options.always_include_focused && node.state.focused {
            if let Some(slot) = keep.get_mut(index) {
                *slot = true;
            }
            continue;
        }
        let fraction = fractions.get(index).copied().unwrap_or(0.0);
        let visible = if options.min_visible_fraction > 0.0 {
            fraction >= options.min_visible_fraction
        } else {
            fraction > 0.0
        };
        if visible {
            if let Some(slot) = keep.get_mut(index) {
                *slot = true;
            }
        }
    }

    // Ancestors of every kept node stay connected.
    for (index, node) in snapshot.nodes.iter().enumerate() {
        if !keep.get(index).copied().unwrap_or(false) {
            continue;
        }
        let mut current = node.parent_index;
        let mut guard = 0;
        while let Some(parent) = current {
            if guard > 512 {
                break;
            }
            if let Some(slot) = keep.get_mut(parent) {
                *slot = true;
            }
            current = snapshot
                .nodes
                .get(parent)
                .and_then(|node| node.parent_index);
            guard += 1;
        }
    }

    let mut hidden_counts: BTreeMap<u64, u32> = BTreeMap::new();
    for (index, node) in snapshot.nodes.iter().enumerate() {
        if keep.get(index).copied().unwrap_or(false) {
            continue;
        }
        let parent_kept = node
            .parent_index
            .and_then(|parent| keep.get(parent))
            .copied()
            .unwrap_or(false);
        if parent_kept {
            if let Some(parent) = node
                .parent_index
                .and_then(|parent| snapshot.nodes.get(parent))
            {
                *hidden_counts.entry(parent.id).or_insert(0) += 1;
            }
        }
    }

    let mut kept_nodes = Vec::with_capacity(old_len);
    for (index, node) in snapshot.nodes.iter().enumerate() {
        if !keep.get(index).copied().unwrap_or(false) {
            continue;
        }
        let mut kept = node.clone();
        kept.visible_fraction = fractions.get(index).copied().unwrap_or(0.0);
        if let Some(hidden) = hidden_counts.get(&kept.id) {
            kept.children_hidden_count = kept.children_hidden_count.saturating_add(*hidden);
        }
        kept_nodes.push(kept);
    }

    let removed = old_len.saturating_sub(kept_nodes.len());
    snapshot.nodes = kept_nodes;
    snapshot.rebuild();
    snapshot.stats.pruned_count = snapshot.stats.pruned_count.saturating_add(removed);
    PruneReport {
        kept: snapshot.nodes.len(),
        removed,
        hidden_counts,
    }
}

/// The clip rectangle for one node, in screen coordinates.
#[must_use]
pub fn clip_rect(nodes: &[crate::snapshot::SnapshotNode], index: usize, viewport: Rect) -> Rect {
    let Some(node) = nodes.get(index) else {
        return viewport;
    };
    if node.escapes_clip {
        return viewport;
    }
    let mut clip = viewport;
    let mut current = node.parent_index;
    let mut guard = 0;
    while let Some(parent) = current {
        if guard > 512 {
            break;
        }
        let Some(parent_node) = nodes.get(parent) else {
            break;
        };
        clip = clip.intersection(parent_node.bounds);
        if parent_node.escapes_clip {
            break;
        }
        current = parent_node.parent_index;
        guard += 1;
    }
    clip
}

/// Fraction of `bounds` inside `clip`, in `0..=1`.
#[must_use]
pub fn visible_fraction(bounds: Rect, clip: Rect) -> f32 {
    let area = bounds.area();
    if area <= 0.0 {
        return 0.0;
    }
    (bounds.intersection(clip).area() / area).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{NodeState, ScreenInfo, SnapshotNode};
    use impresario_core::platforms::Platform;
    use impresario_locator::Role;

    fn node(id: u64, parent_id: u64, bounds: Rect) -> SnapshotNode {
        SnapshotNode::new(id, parent_id, Role::Generic, "", bounds)
    }

    fn sample() -> Snapshot {
        let focused_state = NodeState {
            focused: true,
            ..NodeState::default()
        };
        Snapshot::new(
            "web-1",
            "Chrome",
            Platform::Web,
            ScreenInfo::logical((100.0, 100.0)),
            vec![
                node(1, 0, Rect::new(0.0, 0.0, 100.0, 100.0)),
                node(2, 1, Rect::new(0.0, 0.0, 100.0, 20.0)),
                node(3, 2, Rect::new(0.0, 0.0, 100.0, 20.0)),
                node(4, 2, Rect::new(0.0, 200.0, 100.0, 20.0)),
                node(5, 1, Rect::new(0.0, 200.0, 10.0, 10.0)),
                node(6, 2, Rect::new(0.0, 50.0, 20.0, 20.0)).with_escapes_clip(true),
                node(7, 4, Rect::new(0.0, 200.0, 100.0, 20.0)).with_state(focused_state),
                node(8, 1, Rect::new(0.0, 0.0, 100.0, 10.0)).with_system_ui(true),
                node(9, 2, Rect::new(0.0, 50.0, 20.0, 20.0)),
            ],
        )
    }

    #[test]
    fn prunes_offscreen_and_system_ui_but_keeps_focus_chain() {
        let mut snapshot = sample();
        let report = prune_snapshot(&mut snapshot, PruningOptions::for_screen(&snapshot.screen));
        assert_eq!(report.removed, 3);
        assert!(snapshot.find(3).is_some());
        assert!(snapshot.find(5).is_none(), "offscreen node");
        assert!(snapshot.find(8).is_none(), "system ui");
        assert!(snapshot.find(6).is_some(), "fixed/escape node");
        assert!(snapshot.find(7).is_some(), "focused node");
        assert!(snapshot.find(4).is_some(), "ancestor of focused node");
        assert_eq!(
            snapshot.find(1).map(|node| node.children_hidden_count),
            Some(2)
        );
        assert_eq!(
            snapshot.find(2).map(|node| node.children_hidden_count),
            Some(1)
        );
    }

    #[test]
    fn system_ui_can_be_included() {
        let mut snapshot = sample();
        let mut options = PruningOptions::for_screen(&snapshot.screen);
        options.include_system_ui = true;
        prune_snapshot(&mut snapshot, options);
        assert!(snapshot.find(8).is_some());
    }

    #[test]
    fn clipping_respects_ancestor_bounds_and_escape_hatches() {
        let snapshot = sample();
        let clip = clip_rect(&snapshot.nodes, 9, snapshot.screen.viewport());
        assert_eq!(clip, Rect::new(0.0, 0.0, 100.0, 20.0));
        assert_eq!(
            visible_fraction(Rect::new(0.0, 50.0, 20.0, 20.0), clip),
            0.0
        );

        let clip = clip_rect(&snapshot.nodes, 6, snapshot.screen.viewport());
        assert_eq!(clip, snapshot.screen.viewport());
        assert!(visible_fraction(Rect::new(0.0, 50.0, 20.0, 20.0), clip) > 0.0);
    }

    #[test]
    fn min_visible_fraction_is_configurable() {
        let mut snapshot = sample();
        let mut options = PruningOptions::for_screen(&snapshot.screen);
        options.min_visible_fraction = 0.5;
        prune_snapshot(&mut snapshot, options);
        assert!(snapshot.find(3).is_some());
        assert!(snapshot.find(9).is_none(), "below the visibility threshold");
    }
}
