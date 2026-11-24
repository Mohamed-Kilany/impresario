//! Runner execution of canonical `LOCATOR` selectors (INS-R4).
//!
//! A scene cell written as `role=button[name="Save"]` reaches the connector as
//! `SelectorStrategy::LOCATOR`. The existing command executor only understands
//! the legacy `prefix:value` strategies, so this module resolves the locator
//! against the same viewport snapshot and the same `impresario-locator` matcher
//! the inspector uses, then rewrites it to a unique CSS handle. There is no
//! second matcher: acquisition, matching and role/name normalization are the
//! shared code.
//!
//! Semantics (spec 7.4):
//! - actions and ordinary checks require exactly one in-view match; a non-zero
//!   selector `index` picks that match instead (the DSL's existing `index=N`);
//! - `Count` checks poll the snapshot and compare the full match count;
//! - `Hidden` checks succeed immediately when the locator matches nothing;
//! - probes are best-effort: no match uses a never-matching CSS selector so
//!   the probe reports the element as absent, and the first match is used
//!   otherwise.
//!
//! A resolved element is addressed through
//! `window.__impresarioInspector.selectorFor(id)`, which returns a selector
//! that is verified unique across the composed DOM. When no stable CSS exists
//! (shadow DOM without an id, anonymous wrappers) the element is tagged with a
//! transient `data-impresario-locator` attribute that is removed after the
//! command.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use impresario_core::platforms::Platform;
use impresario_locator::{
    parse as parse_locator, resolve, Context, LocatorError, Node, NodeMatch, Rect, Role, State,
};
use impresario_proto::v1 as pb;
use tokio_util::sync::CancellationToken;

use crate::errors::ConnectorError;
use crate::exec::{CommandOutput, Executor};
use crate::inspector;

/// What `prepare` wants `Session::execute` to do.
pub enum LocatorPlan {
    /// No locator selector in the command: dispatch it unchanged.
    None,
    /// An immediate result assembled from snapshots (`Count`, `Hidden` with
    /// zero matches).
    Done(CommandOutput),
    /// Dispatch the rewritten command, then release a transient tag.
    Run {
        body: pb::command::Body,
        token: Option<String>,
        timeout_ms: u32,
    },
}

/// True when the wire selector asks for the shared locator engine.
#[must_use]
pub fn is_locator(selector: &pb::Selector) -> bool {
    selector.strategy == pb::SelectorStrategy::Locator as i32
}

/// Rewrite a locator command before the legacy executor sees it.
pub async fn prepare(
    executor: &Executor,
    platform: Platform,
    device: &pb::DeviceInfo,
    rev: &AtomicU64,
    body: &pb::command::Body,
    timeout_ms: u32,
    cancel: &CancellationToken,
) -> Result<LocatorPlan, ConnectorError> {
    match body {
        pb::command::Body::RunAction(action) => {
            let Some(selector) = action.selector.as_ref() else {
                return Ok(LocatorPlan::None);
            };
            if !is_locator(selector) {
                return Ok(LocatorPlan::None);
            }
            let resolved = wait_for_single(
                executor,
                platform,
                device,
                rev,
                &selector.value,
                selector.index,
                timeout_ms,
                cancel,
            )
            .await?;
            let mut rewritten = action.clone();
            rewritten.selector = Some(resolved.selector);
            Ok(LocatorPlan::Run {
                body: pb::command::Body::RunAction(rewritten),
                token: resolved.token,
                timeout_ms: resolved.remaining_ms,
            })
        }
        pb::command::Body::ValidateCheck(check) => {
            let Some(selector) = check.selector.as_ref() else {
                return Ok(LocatorPlan::None);
            };
            if !is_locator(selector) {
                return Ok(LocatorPlan::None);
            }
            let kind = pb::CheckKind::try_from(check.kind).map_err(|_| {
                ConnectorError::protocol(format!("unknown check kind {}", check.kind))
            })?;
            match kind {
                pb::CheckKind::Count => {
                    let output = wait_for_count(
                        executor,
                        platform,
                        device,
                        rev,
                        &selector.value,
                        check.expected_count,
                        timeout_ms,
                        cancel,
                    )
                    .await?;
                    Ok(LocatorPlan::Done(output))
                }
                pb::CheckKind::Hidden => {
                    let plan = wait_for_hidden(
                        executor, platform, device, rev, selector, check, timeout_ms, cancel,
                    )
                    .await?;
                    Ok(plan)
                }
                _ => {
                    let resolved = wait_for_single(
                        executor,
                        platform,
                        device,
                        rev,
                        &selector.value,
                        selector.index,
                        timeout_ms,
                        cancel,
                    )
                    .await
                    .map_err(|error| check_resolution_error(check, &selector.value, error))?;
                    let mut rewritten = check.clone();
                    rewritten.selector = Some(resolved.selector);
                    Ok(LocatorPlan::Run {
                        body: pb::command::Body::ValidateCheck(rewritten),
                        token: resolved.token,
                        timeout_ms: resolved.remaining_ms,
                    })
                }
            }
        }
        pb::command::Body::Probe(probe) => {
            let Some(selector) = probe.selector.as_ref() else {
                return Ok(LocatorPlan::None);
            };
            if !is_locator(selector) {
                return Ok(LocatorPlan::None);
            }
            let rewritten_selector =
                resolve_probe_selector(executor, platform, device, rev, &selector.value, cancel)
                    .await?;
            let mut rewritten = probe.clone();
            rewritten.selector = Some(rewritten_selector);
            Ok(LocatorPlan::Run {
                body: pb::command::Body::Probe(rewritten),
                token: None,
                timeout_ms,
            })
        }
        _ => Ok(LocatorPlan::None),
    }
}

/// Remove a transient selector tag after the command that used it.
pub async fn release(
    executor: &Executor,
    token: &str,
    cancel: &CancellationToken,
) -> Result<(), ConnectorError> {
    if token.is_empty() {
        return Ok(());
    }
    let expression = format!(
        "window.__impresarioInspector.releaseSelector({})",
        crate::script::js_string(token)
    );
    executor
        .evaluate_json::<serde_json::Value>(&expression, cancel)
        .await
        .map(|_| ())
}

struct Resolved {
    selector: pb::Selector,
    token: Option<String>,
    remaining_ms: u32,
}

/// Poll until the locator has exactly one match (or the DSL index picks one)
/// and a unique CSS handle is available.
#[allow(clippy::too_many_arguments)]
async fn wait_for_single(
    executor: &Executor,
    platform: Platform,
    device: &pb::DeviceInfo,
    rev: &AtomicU64,
    value: &str,
    index: u32,
    timeout_ms: u32,
    cancel: &CancellationToken,
) -> Result<Resolved, ConnectorError> {
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_millis(u64::from(timeout_ms.max(1)));
    loop {
        cancel_or(cancel).await?;
        let state = snapshot_matches(executor, platform, device, rev, value, cancel).await?;
        if index == 0 && state.matches.len() > 1 {
            return Err(ambiguous(value, state.matches.len()));
        }
        if let Some(chosen) = state.pick(index) {
            if let Some(node_id) = state.node_id(chosen) {
                if let Some(handle) = inspector::selector_for(executor, node_id, cancel).await? {
                    return Ok(Resolved {
                        selector: css_selector(handle.selector),
                        token: non_empty(handle.token),
                        remaining_ms: remaining_ms(deadline),
                    });
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(missing(value, index, state.matches.len()));
        }
        sleep(executor.poll_interval(), cancel).await?;
    }
}

/// Poll the snapshot until the full match count equals `expected`.
#[allow(clippy::too_many_arguments)]
async fn wait_for_count(
    executor: &Executor,
    platform: Platform,
    device: &pb::DeviceInfo,
    rev: &AtomicU64,
    value: &str,
    expected: u32,
    timeout_ms: u32,
    cancel: &CancellationToken,
) -> Result<CommandOutput, ConnectorError> {
    let deadline =
        tokio::time::Instant::now() + Duration::from_millis(u64::from(timeout_ms.max(1)));
    loop {
        cancel_or(cancel).await?;
        let state = snapshot_matches(executor, platform, device, rev, value, cancel).await?;
        let count = u32::try_from(state.matches.len()).unwrap_or(u32::MAX);
        if count == expected {
            return Ok(CommandOutput {
                observed_text: count.to_string(),
                observed_count: count,
                ..CommandOutput::default()
            });
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(check_failed(
                value,
                &expected.to_string(),
                &count.to_string(),
                count,
            ));
        }
        sleep(executor.poll_interval(), cancel).await?;
    }
}

/// A locator `hidden` check succeeds when nothing matches; otherwise the
/// single match is checked for visibility by the legacy executor.
#[allow(clippy::too_many_arguments)]
async fn wait_for_hidden(
    executor: &Executor,
    platform: Platform,
    device: &pb::DeviceInfo,
    rev: &AtomicU64,
    selector: &pb::Selector,
    check: &pb::ValidateCheck,
    timeout_ms: u32,
    cancel: &CancellationToken,
) -> Result<LocatorPlan, ConnectorError> {
    let deadline =
        tokio::time::Instant::now() + Duration::from_millis(u64::from(timeout_ms.max(1)));
    loop {
        cancel_or(cancel).await?;
        let state =
            snapshot_matches(executor, platform, device, rev, &selector.value, cancel).await?;
        if state.matches.is_empty() {
            return Ok(LocatorPlan::Done(CommandOutput {
                observed_text: "missing".to_string(),
                observed_count: 0,
                ..CommandOutput::default()
            }));
        }
        if selector.index == 0 && state.matches.len() > 1 {
            return Err(ambiguous(&selector.value, state.matches.len()));
        }
        if let Some(chosen) = state.pick(selector.index) {
            if let Some(node_id) = state.node_id(chosen) {
                if let Some(handle) = inspector::selector_for(executor, node_id, cancel).await? {
                    let mut rewritten = check.clone();
                    rewritten.selector = Some(css_selector(handle.selector));
                    return Ok(LocatorPlan::Run {
                        body: pb::command::Body::ValidateCheck(rewritten),
                        token: non_empty(handle.token),
                        timeout_ms: remaining_ms(deadline),
                    });
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(check_failed(&selector.value, "hidden", "missing", 0));
        }
        sleep(executor.poll_interval(), cancel).await?;
    }
}

/// Resolve a probe selector best-effort so the probe can still report absence.
async fn resolve_probe_selector(
    executor: &Executor,
    platform: Platform,
    device: &pb::DeviceInfo,
    rev: &AtomicU64,
    value: &str,
    cancel: &CancellationToken,
) -> Result<pb::Selector, ConnectorError> {
    let state = snapshot_matches(executor, platform, device, rev, value, cancel).await?;
    if let Some(chosen) = state.matches.first() {
        if let Some(node_id) = state.node_id(chosen) {
            if let Some(handle) = inspector::selector_for(executor, node_id, cancel).await? {
                return Ok(css_selector(handle.selector));
            }
        }
    }
    // `:not(*)` always matches nothing, so every existing probe path reports
    // the element as absent instead of mistaking a stale CSS path for it.
    Ok(css_selector(":not(*)".to_string()))
}

/// One snapshot plus the shared-matcher matches over it.
struct MatchState {
    snapshot: pb::InspectorSnapshotResponse,
    matches: Vec<NodeMatch>,
}

impl MatchState {
    fn pick(&self, index: u32) -> Option<&NodeMatch> {
        let index = usize::try_from(index).ok()?;
        self.matches.get(index)
    }

    fn node_id(&self, matched: &NodeMatch) -> Option<u64> {
        self.snapshot.nodes.get(matched.index).map(|node| node.id)
    }
}

async fn snapshot_matches(
    executor: &Executor,
    platform: Platform,
    device: &pb::DeviceInfo,
    rev: &AtomicU64,
    value: &str,
    cancel: &CancellationToken,
) -> Result<MatchState, ConnectorError> {
    let request = pb::InspectorSnapshotRequest {
        min_visible_fraction: 0.0,
        ..pb::InspectorSnapshotRequest::default()
    };
    let snapshot =
        inspector::snapshot_with_cancel(executor, platform, device, rev, &request, cancel).await?;
    let matches = match_snapshot(&snapshot, value)?;
    Ok(MatchState { snapshot, matches })
}

/// Resolve `value` against one wire snapshot with the shared matcher.
fn match_snapshot(
    snapshot: &pb::InspectorSnapshotResponse,
    value: &str,
) -> Result<Vec<NodeMatch>, ConnectorError> {
    let locator = parse_locator(value).map_err(|error| invalid(value, &error))?;
    let nodes = wire_nodes(snapshot);
    let screen = (snapshot.logical_width, snapshot.logical_height);
    resolve(&locator, &nodes, screen).map_err(|error| invalid(value, &error))
}

fn wire_nodes(snapshot: &pb::InspectorSnapshotResponse) -> Vec<WireNode> {
    let by_id: HashMap<u64, usize> = snapshot
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id, index))
        .collect();
    snapshot
        .nodes
        .iter()
        .map(|node| WireNode {
            ref_id: format!("e{}", node.id),
            parent: if node.parent_id == 0 {
                None
            } else {
                by_id.get(&node.parent_id).copied()
            },
            node: node.clone(),
        })
        .collect()
}

/// The shared matcher's view of one wire `InspectorNode`.
struct WireNode {
    ref_id: String,
    parent: Option<usize>,
    node: pb::InspectorNode,
}

impl Node for WireNode {
    fn id(&self) -> &str {
        &self.ref_id
    }

    fn role(&self) -> Role {
        Role::from_str(&self.node.role).unwrap_or(Role::Generic)
    }

    fn name(&self) -> &str {
        &self.node.name
    }

    fn text(&self) -> &str {
        &self.node.text
    }

    fn label(&self) -> &str {
        &self.node.label
    }

    fn placeholder(&self) -> &str {
        &self.node.placeholder
    }

    fn alt(&self) -> &str {
        &self.node.alt
    }

    fn title(&self) -> &str {
        &self.node.title
    }

    fn test_id(&self) -> Option<&str> {
        non_empty_ref(&self.node.test_id)
    }

    fn native_id(&self) -> Option<&str> {
        non_empty_ref(&self.node.raw_id)
    }

    fn state(&self, state: State) -> Option<bool> {
        match state {
            State::Enabled => Some(self.node.enabled),
            State::Disabled => Some(!self.node.enabled),
            State::Focused => Some(self.node.focused),
            State::Selected => Some(self.node.selected),
            State::Checked => self.node.checked,
            State::Expanded => self.node.expanded,
            State::Pressed => self.node.pressed,
            State::ReadOnly => Some(self.node.readonly),
            State::Required => Some(self.node.required),
        }
    }

    fn visible(&self) -> bool {
        !self.node.hidden && self.node.visible_fraction > 0.0
    }

    fn rect(&self) -> Rect {
        self.node.bounds.as_ref().map_or_else(
            || Rect::new(0.0, 0.0, 0.0, 0.0),
            |bounds| Rect::new(bounds.x, bounds.y, bounds.w, bounds.h),
        )
    }

    fn context(&self) -> Context {
        Context::Web
    }

    fn parent(&self) -> Option<usize> {
        self.parent
    }
}

fn css_selector(value: String) -> pb::Selector {
    pb::Selector {
        strategy: pb::SelectorStrategy::Css as i32,
        value,
        index: 0,
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn non_empty_ref(value: &str) -> Option<&str> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

async fn cancel_or(cancel: &CancellationToken) -> Result<(), ConnectorError> {
    if cancel.is_cancelled() {
        return Err(ConnectorError::cancelled());
    }
    Ok(())
}

async fn sleep(interval: Duration, cancel: &CancellationToken) -> Result<(), ConnectorError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ConnectorError::cancelled()),
        _ = tokio::time::sleep(interval) => Ok(()),
    }
}

fn remaining_ms(deadline: tokio::time::Instant) -> u32 {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    u32::try_from(remaining.as_millis().max(1)).unwrap_or(u32::MAX)
}

fn invalid(value: &str, error: &LocatorError) -> ConnectorError {
    ConnectorError::invalid_selector(format!("locator {value:?} is invalid: {error}"))
        .with_detail("selector", format!("locator:{value}"))
}

fn ambiguous(value: &str, count: usize) -> ConnectorError {
    ConnectorError::invalid_selector(format!(
        "locator {value:?} matched {count} in-view elements; scene execution requires exactly one"
    ))
    .with_detail("selector", format!("locator:{value}"))
    .with_detail("match_count", count.to_string())
}

fn missing(value: &str, index: u32, count: usize) -> ConnectorError {
    let message = if index == 0 {
        format!("locator {value:?} matched no in-view element within the timeout")
    } else {
        format!(
            "locator {value:?} matched {count} in-view element(s); index {index} is out of range"
        )
    };
    ConnectorError::element_not_found(message)
        .with_detail("selector", format!("locator:{value}"))
        .with_detail("match_count", count.to_string())
}

fn check_failed(value: &str, expected: &str, observed: &str, count: u32) -> ConnectorError {
    ConnectorError::check_failed(format!(
        "check failed for locator:{value} (expected {expected}, observed {observed})"
    ))
    .with_detail("selector", format!("locator:{value}"))
    .with_detail("expected", expected.to_string())
    .with_detail("observed", observed.to_string())
    .with_detail("matched_count", count.to_string())
}

fn check_resolution_error(
    check: &pb::ValidateCheck,
    value: &str,
    error: ConnectorError,
) -> ConnectorError {
    if error.code == impresario_core::command::WireErrorCode::ElementNotFound {
        let count = error
            .details
            .get("match_count")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        return check_failed(value, &check_name(check), "missing", count);
    }
    error
}

fn check_name(check: &pb::ValidateCheck) -> String {
    match pb::CheckKind::try_from(check.kind) {
        Ok(pb::CheckKind::Visible) => "visible",
        Ok(pb::CheckKind::Hidden) => "hidden",
        Ok(pb::CheckKind::Enabled) => "enabled",
        Ok(pb::CheckKind::Disabled) => "disabled",
        Ok(pb::CheckKind::Focused) => "focused",
        Ok(pb::CheckKind::TextEquals) => &check.expected_text,
        Ok(pb::CheckKind::TextContainsCheck) => &check.expected_text,
        Ok(pb::CheckKind::Count) => "count",
        Ok(pb::CheckKind::Unspecified) | Err(_) => "check",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Option<pb::InspectorRect> {
        Some(pb::InspectorRect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        })
    }

    fn node(id: u64, parent: u64, role: &str, name: &str) -> pb::InspectorNode {
        pb::InspectorNode {
            id,
            parent_id: parent,
            role: role.to_string(),
            name: name.to_string(),
            bounds: rect(),
            viewport_bounds: rect(),
            enabled: true,
            visible_fraction: 1.0,
            ..pb::InspectorNode::default()
        }
    }

    fn snapshot(nodes: Vec<pb::InspectorNode>) -> pb::InspectorSnapshotResponse {
        pb::InspectorSnapshotResponse {
            logical_width: 100.0,
            logical_height: 100.0,
            nodes,
            ..pb::InspectorSnapshotResponse::default()
        }
    }

    #[test]
    fn wire_nodes_resolve_the_shared_locator_kinds() {
        let mut button = node(3, 2, "button", "Click me");
        button.raw_id = "click-button".to_string();
        button.test_id = "login.submit".to_string();
        let snap = snapshot(vec![
            node(1, 0, "generic", ""),
            node(2, 1, "generic", ""),
            button,
        ]);

        let role = match_snapshot(&snap, "role=button[name=\"Click me\"]").expect("role locator");
        assert_eq!(role.len(), 1);
        assert_eq!(role[0].index, 2);
        assert_eq!(role[0].role, Role::Button);

        let id = match_snapshot(&snap, "id=click-button").expect("id locator");
        assert_eq!(id.len(), 1);
        assert_eq!(id[0].index, 2);

        let testid = match_snapshot(&snap, "testid=login.submit").expect("testid locator");
        assert_eq!(testid.len(), 1);
        assert_eq!(testid[0].index, 2);
    }

    #[test]
    fn within_scopes_descendants_through_wire_parents() {
        let mut form = node(2, 1, "generic", "");
        form.raw_id = "form".to_string();
        let snap = snapshot(vec![
            node(1, 0, "generic", ""),
            form,
            node(3, 2, "button", "Save"),
            node(4, 1, "button", "Save"),
        ]);
        let found = match_snapshot(&snap, "within(id=form) >> role=button[name=\"Save\"]")
            .expect("scoped locator");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "e3");
    }

    #[test]
    fn wire_state_and_visibility_map_onto_the_matcher() {
        let mut disabled = node(2, 1, "button", "Off");
        disabled.enabled = false;
        let mut hidden = node(3, 1, "button", "Ghost");
        hidden.hidden = true;
        let snap = snapshot(vec![node(1, 0, "generic", ""), disabled, hidden]);

        assert_eq!(
            match_snapshot(&snap, "role=button[disabled=true]")
                .expect("disabled")
                .len(),
            1
        );
        assert!(match_snapshot(&snap, "role=button[enabled=true]")
            .expect("enabled")
            .is_empty());
        assert!(match_snapshot(&snap, "role=button[name=\"Ghost\"]")
            .expect("visible default")
            .is_empty());
        assert_eq!(
            match_snapshot(&snap, "role=button[name=\"Ghost\"][visible=false]")
                .expect("explicit visibility")
                .len(),
            1
        );
    }

    #[test]
    fn invalid_locator_text_maps_to_invalid_selector() {
        let snap = snapshot(vec![node(1, 0, "generic", "")]);
        let error = match_snapshot(&snap, "role=nope").expect_err("unknown role");
        assert_eq!(
            error.code,
            impresario_core::command::WireErrorCode::InvalidSelector
        );
        assert_eq!(
            error.details.get("selector").map(String::as_str),
            Some("locator:role=nope")
        );
    }

    #[test]
    fn missing_parents_degrade_to_roots() {
        let error_parent = node(2, 99, "button", "Orphan");
        let snap = snapshot(vec![error_parent]);
        let found = match_snapshot(&snap, "role=button").expect("orphan");
        assert_eq!(found.len(), 1);
    }

    /// INS-R12 run-path latency gate: a large viewport snapshot must resolve a
    /// canonical locator well inside the per-command budget. This exercises
    /// the exact path a `LOCATOR` selector takes (`wire_nodes` +
    /// `impresario_locator::resolve`), not a synthetic matcher loop.
    #[test]
    fn run_path_locator_resolution_stays_within_budget() {
        use std::time::Instant;

        const NODES: u64 = 2_000;
        const RUNS: usize = 40;
        const BUDGET_MS: u128 = 100;

        let mut nodes = Vec::with_capacity(usize::try_from(NODES).expect("capacity"));
        nodes.push(node(1, 0, "generic", ""));
        for id in 2..=NODES {
            let name = if id == NODES {
                "Target".to_string()
            } else {
                format!("Button {id}")
            };
            nodes.push(node(id, 1, "button", &name));
        }
        let snap = snapshot(nodes);

        // Warm caches and prove the locator selects exactly the target.
        let warm = match_snapshot(&snap, "role=button[name=\"Target\"]").expect("warmup");
        assert_eq!(warm.len(), 1);
        assert_eq!(warm[0].id, format!("e{NODES}"));

        let mut samples = Vec::with_capacity(RUNS);
        for _ in 0..RUNS {
            let started = Instant::now();
            let found =
                match_snapshot(&snap, "role=button[name=\"Target\"]").expect("measured run");
            samples.push(started.elapsed());
            assert_eq!(found.len(), 1);
        }
        samples.sort_unstable();
        let p95_index = samples.len().saturating_mul(95) / 100;
        let p95 = samples
            .get(p95_index.saturating_sub(1))
            .copied()
            .expect("p95 sample");
        assert!(
            p95.as_millis() <= BUDGET_MS,
            "p95 run-path locator resolution was {p95:?} over {RUNS} runs of {NODES} nodes \
             (budget {BUDGET_MS} ms)"
        );
    }
}
