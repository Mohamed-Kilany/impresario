#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::todo,
        clippy::unreachable
    )
)]

//! `impresario-inspector-core`: the pure, shared engine behind the inspector
//! Web UI and MCP frontend.
//!
//! The crate is deliberately synchronous and device-free:
//!
//! - [`snapshot`] — the normalized, viewport-only tree and its locator bridge.
//! - [`diff`] — deltas with `apply(diff(prev, next), prev) == next`.
//! - [`prune`] — clip geometry, visibility fractions and focus retention.
//! - [`selection`] — fingerprints and selection re-resolution.
//! - [`compact`] — the token-efficient MCP text view.
//! - [`columns`] — scene column resolution with structured errors.
//! - [`mock`] — scripted agents and bounded coalescing queues.
//!
//! The live acquisition layer, HTTP/SSE server and MCP tools live in the
//! process crates and only depend on these pure functions.

pub mod columns;
pub mod compact;
pub mod diff;
pub mod mock;
pub mod prune;
pub mod selection;
pub mod snapshot;

pub use columns::{resolve_column, ColumnError, ColumnResolution};
pub use compact::{compact, compact_json, CompactOptions};
pub use diff::{
    apply as apply_delta, apply_plan, diff as diff_snapshots, plan_update, DeltaOp, DiffError,
    SnapshotDelta, UpdatePlan,
};
pub use mock::{CoalescingQueue, Fanout, MockAgent, MockStep, PushOutcome};
pub use prune::{prune_snapshot, visible_fraction, PruneReport, PruningOptions};
pub use selection::{resolve_selection, Fingerprint, Selection, SelectionState};
pub use snapshot::{
    Degraded, DegradedReason, FocusModel, Insets, NodeState, ScreenInfo, ScrollAxis, ScrollInfo,
    Snapshot, SnapshotError, SnapshotNode, SnapshotStats,
};
