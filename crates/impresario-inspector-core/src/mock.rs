//! Deterministic mock agent and bounded coalescing queue.
//!
//! The inspector must never queue unbounded snapshots for a slow client
//! (spec §4.2). [`CoalescingQueue`] keeps at most one pending update, drops
//! intermediate snapshots and — after any drop — sends a full snapshot so a
//! delta can never be applied to the wrong revision. [`MockAgent`] replays
//! scripted acquisitions for tests without any device or async runtime.

use crate::diff::{apply, apply_plan, plan_update, UpdatePlan};
use crate::snapshot::Snapshot;

/// One scripted acquisition step.
#[derive(Debug, Clone, PartialEq)]
pub enum MockStep {
    Snapshot(Box<Snapshot>),
    Error(String),
}

/// A deterministic snapshot source for tests and fixtures.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MockAgent {
    script: Vec<MockStep>,
    cursor: usize,
}

impl MockAgent {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_snapshots(snapshots: Vec<Snapshot>) -> Self {
        let mut agent = Self::new();
        for snapshot in snapshots {
            agent.push_snapshot(snapshot);
        }
        agent
    }

    pub fn push_snapshot(&mut self, snapshot: Snapshot) {
        self.script.push(MockStep::Snapshot(Box::new(snapshot)));
    }

    pub fn push_error(&mut self, message: impl Into<String>) {
        self.script.push(MockStep::Error(message.into()));
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.script.len().saturating_sub(self.cursor)
    }

    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.remaining() == 0
    }

    pub fn reset(&mut self) {
        self.cursor = 0;
    }

    /// Return the next scripted acquisition.
    ///
    /// Exhaustion is sticky: repeated calls keep returning the same error
    /// until [`MockAgent::reset`] is called.
    pub fn acquire(&mut self) -> Result<Snapshot, String> {
        let Some(step) = self.script.get(self.cursor).cloned() else {
            return Err("mock agent script exhausted".to_string());
        };
        self.cursor = self.cursor.saturating_add(1);
        match step {
            MockStep::Snapshot(snapshot) => Ok(*snapshot),
            MockStep::Error(message) => Err(message),
        }
    }
}

/// What happened to one pushed update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// The update is pending; nothing was dropped.
    Queued,
    /// An earlier pending update was replaced by a full snapshot.
    Coalesced {
        /// Total number of updates dropped by this queue so far.
        dropped: u64,
    },
}

/// A single-client, bounded update queue.
#[derive(Debug, Clone, Default)]
pub struct CoalescingQueue {
    last_sent: Option<Snapshot>,
    pending: Option<UpdatePlan>,
    dropped: u64,
}

impl CoalescingQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start from a snapshot the client already has.
    #[must_use]
    pub fn with_initial(snapshot: Snapshot) -> Self {
        Self {
            last_sent: Some(snapshot),
            pending: None,
            dropped: 0,
        }
    }

    /// Queue the next snapshot, coalescing with any pending update.
    pub fn push(&mut self, next: Snapshot) -> PushOutcome {
        if self.pending.is_some() {
            self.pending = Some(UpdatePlan::Full(next));
            self.dropped = self.dropped.saturating_add(1);
            return PushOutcome::Coalesced {
                dropped: self.dropped,
            };
        }
        let plan = match &self.last_sent {
            Some(previous) => plan_update(previous, &next),
            None => UpdatePlan::Full(next),
        };
        self.pending = Some(plan);
        PushOutcome::Queued
    }

    /// Take the pending update, advancing the client's revision.
    pub fn pop(&mut self) -> Option<UpdatePlan> {
        let plan = self.pending.take()?;
        let next = match &plan {
            UpdatePlan::Full(snapshot) => Some(snapshot.clone()),
            UpdatePlan::Delta(delta) => self
                .last_sent
                .as_ref()
                .and_then(|previous| apply(previous, delta).ok()),
        };
        if let Some(next) = next {
            self.last_sent = Some(next);
        }
        Some(plan)
    }

    /// True when an update is waiting.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Number of pending updates; always `0` or `1`.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        usize::from(self.pending.is_some())
    }

    /// Total number of dropped intermediate updates.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The revision the client would have after draining the queue.
    #[must_use]
    pub fn last_sent_rev(&self) -> Option<u64> {
        self.last_sent.as_ref().map(|snapshot| snapshot.rev)
    }
}

/// A set of subscriber queues for stream stress tests.
#[derive(Debug, Clone, Default)]
pub struct Fanout {
    queues: Vec<CoalescingQueue>,
}

impl Fanout {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a subscriber; `initial` is the snapshot it already has.
    pub fn subscribe(&mut self, initial: Option<Snapshot>) -> usize {
        let queue = match initial {
            Some(snapshot) => CoalescingQueue::with_initial(snapshot),
            None => CoalescingQueue::new(),
        };
        self.queues.push(queue);
        self.queues.len().saturating_sub(1)
    }

    /// Queue `next` for every subscriber.
    pub fn broadcast(&mut self, next: Snapshot) -> Vec<PushOutcome> {
        self.queues
            .iter_mut()
            .map(|queue| queue.push(next.clone()))
            .collect()
    }

    /// Drain one subscriber's pending update.
    pub fn pop(&mut self, subscriber: usize) -> Option<UpdatePlan> {
        self.queues.get_mut(subscriber)?.pop()
    }

    /// Look at one subscriber's queue.
    #[must_use]
    pub fn queue(&self, subscriber: usize) -> Option<&CoalescingQueue> {
        self.queues.get(subscriber)
    }

    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.queues.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{ScreenInfo, SnapshotNode};
    use impresario_core::platforms::Platform;
    use impresario_locator::{Rect, Role};

    fn snapshot(rev: u64, name: &str) -> Snapshot {
        let root = SnapshotNode::new(1, 0, Role::Generic, "", Rect::new(0.0, 0.0, 100.0, 100.0));
        let button = SnapshotNode::new(2, 1, Role::Button, name, Rect::new(10.0, 10.0, 30.0, 20.0))
            .with_test_id("save");
        Snapshot::new(
            "web-1",
            "Chrome",
            Platform::Web,
            ScreenInfo::logical((100.0, 100.0)),
            vec![root, button],
        )
        .with_rev(rev)
    }

    #[test]
    fn mock_agent_yields_scripted_steps_then_stays_exhausted() {
        let mut agent = MockAgent::from_snapshots(vec![snapshot(1, "Save")]);
        agent.push_error("device disconnected");
        assert_eq!(agent.remaining(), 2);
        assert_eq!(agent.acquire().map(|snapshot| snapshot.rev), Ok(1));
        assert_eq!(agent.acquire(), Err("device disconnected".to_string()));
        assert!(agent.is_exhausted());
        assert_eq!(
            agent.acquire(),
            Err("mock agent script exhausted".to_string())
        );
        agent.reset();
        assert_eq!(agent.remaining(), 2);
    }

    #[test]
    fn first_update_is_full_and_small_changes_are_deltas() {
        let first = snapshot(1, "Save");
        let second = snapshot(2, "Save now");

        let mut queue = CoalescingQueue::new();
        assert_eq!(queue.push(first.clone()), PushOutcome::Queued);
        assert_eq!(queue.pending_len(), 1);
        let plan = queue.pop().expect("initial plan");
        assert!(matches!(plan, UpdatePlan::Full(_)));
        assert_eq!(queue.last_sent_rev(), Some(1));

        assert_eq!(queue.push(second.clone()), PushOutcome::Queued);
        let plan = queue.pop().expect("delta plan");
        assert!(matches!(plan, UpdatePlan::Delta(_)));
        assert_eq!(queue.last_sent_rev(), Some(2));
        assert_eq!(apply_plan(&first, &plan).expect("apply"), second);
    }

    #[test]
    fn slow_client_keeps_only_the_latest_update() {
        let first = snapshot(1, "Save");
        let mut queue = CoalescingQueue::with_initial(first);
        assert_eq!(queue.push(snapshot(2, "a")), PushOutcome::Queued);
        assert_eq!(
            queue.push(snapshot(3, "b")),
            PushOutcome::Coalesced { dropped: 1 }
        );
        assert_eq!(
            queue.push(snapshot(4, "c")),
            PushOutcome::Coalesced { dropped: 2 }
        );
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(queue.dropped(), 2);

        let plan = queue.pop().expect("latest plan");
        assert!(matches!(plan, UpdatePlan::Full(_)));
        assert_eq!(queue.last_sent_rev(), Some(4));
        assert!(!queue.has_pending());
    }

    #[test]
    fn delta_after_coalescing_uses_the_latest_full_snapshot() {
        let mut queue = CoalescingQueue::with_initial(snapshot(1, "Save"));
        queue.push(snapshot(2, "a"));
        queue.push(snapshot(3, "b"));
        let full = queue.pop().expect("full plan");
        let latest = snapshot(4, "c");
        queue.push(latest.clone());
        let delta = queue.pop().expect("delta plan");
        assert!(matches!(delta, UpdatePlan::Delta(_)));
        let full_snapshot = match full {
            UpdatePlan::Full(snapshot) => snapshot,
            UpdatePlan::Delta(_) => panic!("expected full snapshot"),
        };
        assert_eq!(apply_plan(&full_snapshot, &delta).expect("apply"), latest);
    }

    #[test]
    fn stream_stress_with_a_slow_client_stays_bounded_and_consistent() {
        let first = snapshot(1, "0");
        let mut queue = CoalescingQueue::with_initial(first.clone());
        let mut client = first;
        let mut rev = 1u64;

        for index in 0..500u64 {
            rev += 1;
            queue.push(snapshot(rev, &format!("value {index}")));
            if index % 7 == 0 {
                if let Some(plan) = queue.pop() {
                    client = apply_plan(&client, &plan).expect("apply plan");
                }
            }
            assert!(queue.pending_len() <= 1);
        }

        if let Some(plan) = queue.pop() {
            client = apply_plan(&client, &plan).expect("final plan");
        }
        assert_eq!(client.rev, rev);
        assert_eq!(client, snapshot(rev, "value 499"));
        assert!(queue.dropped() > 0);
    }

    #[test]
    fn fanout_isolates_fast_and_slow_subscribers() {
        let mut fanout = Fanout::new();
        let fast = fanout.subscribe(Some(snapshot(1, "0")));
        let slow = fanout.subscribe(Some(snapshot(1, "0")));
        assert_eq!(fanout.subscriber_count(), 2);

        for rev in 2..=100u64 {
            fanout.broadcast(snapshot(rev, &format!("value {rev}")));
            if let Some(plan) = fanout.pop(fast) {
                assert!(matches!(plan, UpdatePlan::Full(_) | UpdatePlan::Delta(_)));
            }
        }

        let slow_queue = fanout.queue(slow).expect("slow queue");
        assert_eq!(slow_queue.pending_len(), 1);
        assert_eq!(slow_queue.dropped(), 98);

        let plan = fanout.pop(slow).expect("slow plan");
        let snapshot = match plan {
            UpdatePlan::Full(snapshot) => snapshot,
            UpdatePlan::Delta(_) => panic!("slow client must receive a full snapshot"),
        };
        assert_eq!(snapshot.rev, 100);
        assert_eq!(
            fanout.queue(slow).and_then(CoalescingQueue::last_sent_rev),
            Some(100)
        );
    }
}
