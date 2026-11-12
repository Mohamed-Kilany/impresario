//! Selection fingerprints and re-resolution after every snapshot/delta.

use impresario_locator::{Rect, Role};
use serde::{Deserialize, Serialize};

use crate::snapshot::{Snapshot, SnapshotNode};

/// A cheap identity fingerprint used when a stable id disappears.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub role: Role,
    pub name: String,
    pub text: String,
    pub test_id: Option<String>,
    pub native_id: Option<String>,
    pub parent_id: u64,
    pub bounds: Rect,
}

impl Fingerprint {
    #[must_use]
    pub fn of(node: &SnapshotNode) -> Self {
        Self {
            role: node.role,
            name: node.name.clone(),
            text: node.text.clone(),
            test_id: node.test_id.clone(),
            native_id: node.native_id.clone(),
            parent_id: node.parent_id,
            bounds: node.bounds,
        }
    }

    /// Match score; `None` means a hard identity constraint failed.
    #[must_use]
    pub fn score(&self, node: &SnapshotNode) -> Option<u32> {
        if let Some(expected) = &self.test_id {
            if node.test_id.as_deref() != Some(expected.as_str()) {
                return None;
            }
        }
        if let Some(expected) = &self.native_id {
            if node.native_id.as_deref() != Some(expected.as_str()) {
                return None;
            }
        }
        if self.role != node.role {
            return None;
        }
        let mut score = 2;
        if !self.name.is_empty() && self.name == node.name {
            score += 3;
        }
        if !self.text.is_empty() && self.text == node.text {
            score += 1;
        }
        if self.parent_id == node.parent_id {
            score += 1;
        }
        let (fx, fy) = self.bounds.center();
        let (nx, ny) = node.bounds.center();
        let distance = ((fx - nx).powi(2) + (fy - ny).powi(2)).sqrt();
        if distance < 20.0 {
            score += 2;
        } else if distance < 100.0 {
            score += 1;
        }
        Some(score)
    }
}

/// A selected element and the fingerprint captured with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub id: u64,
    pub fingerprint: Fingerprint,
}

impl Selection {
    #[must_use]
    pub fn new(node: &SnapshotNode) -> Self {
        Self {
            id: node.id,
            fingerprint: Fingerprint::of(node),
        }
    }
}

/// The outcome of re-resolving a selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SelectionState {
    Alive { id: u64 },
    Remapped { id: u64 },
    Offscreen { id: u64 },
    Stale { id: u64 },
}

impl SelectionState {
    #[must_use]
    pub fn id(&self) -> u64 {
        match self {
            SelectionState::Alive { id }
            | SelectionState::Remapped { id }
            | SelectionState::Offscreen { id }
            | SelectionState::Stale { id } => *id,
        }
    }

    #[must_use]
    pub fn is_alive(&self) -> bool {
        matches!(self, SelectionState::Alive { .. })
    }
}

/// Re-resolve a selection against a snapshot.
#[must_use]
pub fn resolve_selection(selection: &Selection, snapshot: &Snapshot) -> SelectionState {
    if let Some(node) = snapshot.find(selection.id) {
        if node.visible_fraction > 0.0 && !node.state.hidden_by_a11y {
            return SelectionState::Alive { id: selection.id };
        }
        return SelectionState::Offscreen { id: selection.id };
    }

    let mut best: Option<(u32, u64)> = None;
    let mut tied = false;
    for node in &snapshot.nodes {
        let Some(score) = selection.fingerprint.score(node) else {
            continue;
        };
        if score < 4 {
            continue;
        }
        match best {
            None => best = Some((score, node.id)),
            Some((best_score, _)) if score > best_score => {
                best = Some((score, node.id));
                tied = false;
            }
            Some((best_score, _)) if score == best_score => tied = true,
            _ => {}
        }
    }

    match best {
        Some((_, id)) if !tied => SelectionState::Remapped { id },
        _ => SelectionState::Stale { id: selection.id },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{ScreenInfo, SnapshotNode};
    use impresario_core::platforms::Platform;

    fn button(id: u64, parent_id: u64, test_id: &str) -> SnapshotNode {
        SnapshotNode::new(
            id,
            parent_id,
            Role::Button,
            "Save",
            Rect::new(10.0, 10.0, 30.0, 20.0),
        )
        .with_test_id(test_id)
        .with_text("Save")
    }

    fn snapshot(nodes: Vec<SnapshotNode>) -> Snapshot {
        Snapshot::new(
            "web-1",
            "Chrome",
            Platform::Web,
            ScreenInfo::logical((100.0, 100.0)),
            nodes,
        )
    }

    #[test]
    fn same_id_is_alive() {
        let first = snapshot(vec![
            SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 100.0, 100.0)),
            button(2, 1, "save"),
        ]);
        let selection = Selection::new(first.find(2).expect("button"));
        assert_eq!(
            resolve_selection(&selection, &first),
            SelectionState::Alive { id: 2 }
        );
    }

    #[test]
    fn same_id_offscreen_is_reported() {
        let first = snapshot(vec![
            SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 100.0, 100.0)),
            button(2, 1, "save"),
        ]);
        let selection = Selection::new(first.find(2).expect("button"));
        let mut second = first.clone();
        if let Some(node) = second.nodes.get_mut(1) {
            node.visible_fraction = 0.0;
        }
        assert_eq!(
            resolve_selection(&selection, &second),
            SelectionState::Offscreen { id: 2 }
        );
    }

    #[test]
    fn changed_id_is_remapped_by_test_id() {
        let first = snapshot(vec![
            SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 100.0, 100.0)),
            button(2, 1, "save"),
        ]);
        let selection = Selection::new(first.find(2).expect("button"));
        let second = snapshot(vec![
            SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 100.0, 100.0)),
            button(9, 1, "save"),
        ]);
        assert_eq!(
            resolve_selection(&selection, &second),
            SelectionState::Remapped { id: 9 }
        );
    }

    #[test]
    fn missing_or_ambiguous_fingerprints_are_stale() {
        let first = snapshot(vec![
            SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 100.0, 100.0)),
            button(2, 1, "save"),
        ]);
        let selection = Selection::new(first.find(2).expect("button"));

        let missing = snapshot(vec![
            SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 100.0, 100.0)),
            button(9, 1, "delete"),
        ]);
        assert_eq!(
            resolve_selection(&selection, &missing),
            SelectionState::Stale { id: 2 }
        );

        let ambiguous = snapshot(vec![
            SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 100.0, 100.0)),
            button(9, 1, "save"),
            button(10, 1, "save"),
        ]);
        assert_eq!(
            resolve_selection(&selection, &ambiguous),
            SelectionState::Stale { id: 2 }
        );
    }
}
