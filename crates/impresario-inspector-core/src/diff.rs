//! Snapshot diffing, delta application and full-vs-delta selection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::snapshot::{Snapshot, SnapshotNode};

/// One delta operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DeltaOp {
    Add {
        node: SnapshotNode,
    },
    Remove {
        id: u64,
    },
    Update {
        node: SnapshotNode,
    },
    Move {
        id: u64,
        parent_id: u64,
    },
    /// Order-only change. Kept explicit so `apply(diff(a,b), a) == b`
    /// holds even when siblings are reordered without changing content.
    Reorder {
        ids: Vec<u64>,
    },
}

/// A revision-bound delta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotDelta {
    pub base_rev: u64,
    pub rev: u64,
    pub ops: Vec<DeltaOp>,
}

impl SnapshotDelta {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// Compute the delta from `prev` to `next`.
#[must_use]
pub fn diff(prev: &Snapshot, next: &Snapshot) -> SnapshotDelta {
    let previous: BTreeMap<u64, &SnapshotNode> =
        prev.nodes.iter().map(|node| (node.id, node)).collect();
    let following: BTreeMap<u64, &SnapshotNode> =
        next.nodes.iter().map(|node| (node.id, node)).collect();

    let mut ops = Vec::new();
    for id in previous.keys() {
        if !following.contains_key(id) {
            ops.push(DeltaOp::Remove { id: *id });
        }
    }
    for node in &next.nodes {
        let Some(old) = previous.get(&node.id) else {
            continue;
        };
        if old.parent_id != node.parent_id {
            ops.push(DeltaOp::Move {
                id: node.id,
                parent_id: node.parent_id,
            });
        }
        if normalized(old) != normalized(node) {
            ops.push(DeltaOp::Update { node: node.clone() });
        }
    }
    for node in &next.nodes {
        if !previous.contains_key(&node.id) {
            ops.push(DeltaOp::Add { node: node.clone() });
        }
    }

    let previous_order: Vec<u64> = prev
        .nodes
        .iter()
        .map(|node| node.id)
        .filter(|id| following.contains_key(id))
        .collect();
    let next_order: Vec<u64> = next
        .nodes
        .iter()
        .map(|node| node.id)
        .filter(|id| previous.contains_key(id))
        .collect();
    if previous_order != next_order {
        ops.push(DeltaOp::Reorder {
            ids: next.nodes.iter().map(|node| node.id).collect(),
        });
    }

    SnapshotDelta {
        base_rev: prev.rev,
        rev: next.rev,
        ops,
    }
}

fn normalized(node: &SnapshotNode) -> SnapshotNode {
    let mut copy = node.clone();
    copy.parent_id = 0;
    copy.parent_index = None;
    copy.ref_id.clear();
    copy
}

/// Apply a delta to `prev`.
pub fn apply(prev: &Snapshot, delta: &SnapshotDelta) -> Result<Snapshot, DiffError> {
    if delta.base_rev != prev.rev {
        return Err(DiffError::BaseRevisionMismatch {
            expected: prev.rev,
            got: delta.base_rev,
        });
    }
    let mut nodes = prev.nodes.clone();
    for op in &delta.ops {
        match op {
            DeltaOp::Remove { id } => nodes.retain(|node| node.id != *id),
            DeltaOp::Update { node } => {
                let Some(slot) = nodes.iter_mut().find(|candidate| candidate.id == node.id) else {
                    return Err(DiffError::MissingNode(node.id));
                };
                *slot = node.clone();
            }
            DeltaOp::Move { id, parent_id } => {
                let Some(node) = nodes.iter_mut().find(|candidate| candidate.id == *id) else {
                    return Err(DiffError::MissingNode(*id));
                };
                node.parent_id = *parent_id;
            }
            DeltaOp::Add { node } => {
                if nodes.iter().any(|candidate| candidate.id == node.id) {
                    return Err(DiffError::DuplicateNode(node.id));
                }
                nodes.push(node.clone());
            }
            DeltaOp::Reorder { ids } => {
                let mut by_id: BTreeMap<u64, SnapshotNode> =
                    nodes.into_iter().map(|node| (node.id, node)).collect();
                let mut ordered = Vec::with_capacity(ids.len());
                for id in ids {
                    let Some(node) = by_id.remove(id) else {
                        return Err(DiffError::ReorderMismatch(*id));
                    };
                    ordered.push(node);
                }
                if let Some(extra) = by_id.keys().next() {
                    return Err(DiffError::ReorderMismatch(*extra));
                }
                nodes = ordered;
            }
        }
    }
    let mut snapshot = prev.clone();
    snapshot.nodes = nodes;
    snapshot.rev = delta.rev;
    snapshot.rebuild();
    Ok(snapshot)
}

/// The update chosen for a client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdatePlan {
    Full(Snapshot),
    Delta(SnapshotDelta),
}

/// Choose a delta only when it is smaller than 60% of the full snapshot.
#[must_use]
pub fn plan_update(prev: &Snapshot, next: &Snapshot) -> UpdatePlan {
    let delta = diff(prev, next);
    let delta_len = serde_json::to_string(&delta)
        .map(|json| json.len())
        .unwrap_or(usize::MAX);
    let full_len = serde_json::to_string(next)
        .map(|json| json.len())
        .unwrap_or(0);
    if full_len == 0 || delta_len.saturating_mul(100) >= full_len.saturating_mul(60) {
        UpdatePlan::Full(next.clone())
    } else {
        UpdatePlan::Delta(delta)
    }
}

/// Apply whichever plan was chosen.
pub fn apply_plan(prev: &Snapshot, plan: &UpdatePlan) -> Result<Snapshot, DiffError> {
    match plan {
        UpdatePlan::Full(snapshot) => Ok(snapshot.clone()),
        UpdatePlan::Delta(delta) => apply(prev, delta),
    }
}

/// Delta application errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiffError {
    #[error("delta base revision {got} does not match snapshot revision {expected}")]
    BaseRevisionMismatch { expected: u64, got: u64 },
    #[error("delta references missing node {0}")]
    MissingNode(u64),
    #[error("delta adds duplicate node {0}")]
    DuplicateNode(u64),
    #[error("delta reorder list does not match the node set (unexpected id {0})")]
    ReorderMismatch(u64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{ScreenInfo, SnapshotNode};
    use impresario_core::platforms::Platform;
    use impresario_locator::{Rect, Role};

    fn node(id: u64, parent_id: u64, role: Role, name: &str) -> SnapshotNode {
        SnapshotNode::new(id, parent_id, role, name, Rect::new(0.0, 0.0, 10.0, 10.0))
    }

    fn base() -> Snapshot {
        Snapshot::new(
            "web-1",
            "Chrome",
            Platform::Web,
            ScreenInfo::logical((100.0, 100.0)),
            vec![
                node(1, 0, Role::Generic, ""),
                node(2, 1, Role::Button, "Save"),
                node(3, 1, Role::Button, "Cancel"),
                node(5, 1, Role::Button, "Old"),
            ],
        )
        .with_rev(10)
    }

    fn next() -> Snapshot {
        Snapshot::new(
            "web-1",
            "Chrome",
            Platform::Web,
            ScreenInfo::logical((100.0, 100.0)),
            vec![
                node(1, 0, Role::Generic, ""),
                node(3, 2, Role::Button, "Cancel"),
                node(2, 1, Role::Button, "Save now"),
                node(4, 1, Role::Button, "Delete"),
            ],
        )
        .with_rev(11)
    }

    #[test]
    fn diff_contains_every_kind_of_operation() {
        let delta = diff(&base(), &next());
        assert_eq!(delta.base_rev, 10);
        assert_eq!(delta.rev, 11);
        assert!(delta
            .ops
            .iter()
            .any(|op| matches!(op, DeltaOp::Remove { id: 5 })));
        assert!(delta
            .ops
            .iter()
            .any(|op| matches!(op, DeltaOp::Add { node } if node.id == 4)));
        assert!(delta
            .ops
            .iter()
            .any(|op| matches!(op, DeltaOp::Update { node } if node.id == 2)));
        assert!(delta.ops.iter().any(|op| matches!(
            op,
            DeltaOp::Move {
                id: 3,
                parent_id: 2
            }
        )));
        assert!(delta
            .ops
            .iter()
            .any(|op| matches!(op, DeltaOp::Reorder { .. })));
    }

    #[test]
    fn apply_equals_full_snapshot() {
        let previous = base();
        let following = next();
        let delta = diff(&previous, &following);
        let applied = apply(&previous, &delta).expect("apply");
        assert_eq!(applied.rev, following.rev);
        assert_eq!(applied.nodes, following.nodes);
        assert!(applied.same_nodes(&following));
    }

    #[test]
    fn apply_round_trips_arbitrary_name_changes() {
        let previous = base();
        for revision in 0..20u64 {
            let mut following = previous.clone();
            following.rev = previous.rev + revision + 1;
            if let Some(node) = following.nodes.get_mut(1) {
                node.name = format!("Save {revision}");
            }
            let delta = diff(&previous, &following);
            let applied = apply(&previous, &delta).expect("apply");
            assert_eq!(applied.nodes, following.nodes);
        }
    }

    #[test]
    fn base_revision_mismatch_is_rejected() {
        let previous = base();
        let mut delta = diff(&previous, &next());
        delta.base_rev = 99;
        assert_eq!(
            apply(&previous, &delta),
            Err(DiffError::BaseRevisionMismatch {
                expected: 10,
                got: 99
            })
        );
    }

    #[test]
    fn small_changes_use_a_delta_and_large_changes_use_a_full_snapshot() {
        let previous = base();
        let mut following = previous.clone();
        following.rev = 11;
        if let Some(node) = following.nodes.get_mut(1) {
            node.name = "Save now".to_string();
        }
        assert!(matches!(
            plan_update(&previous, &following),
            UpdatePlan::Delta(_)
        ));

        let mut everything = previous.clone();
        everything.rev = 11;
        for node in &mut everything.nodes {
            node.name = format!("changed {}", node.id);
            node.text = "a much longer text value that makes the full snapshot larger".to_string();
        }
        assert!(matches!(
            plan_update(&previous, &everything),
            UpdatePlan::Full(_)
        ));
    }

    #[test]
    fn apply_plan_matches_apply() {
        let previous = base();
        let following = next();
        let plan = plan_update(&previous, &following);
        let applied = apply_plan(&previous, &plan).expect("apply plan");
        assert_eq!(applied.nodes, following.nodes);
    }
}
