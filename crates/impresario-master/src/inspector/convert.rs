//! Conversion from the wire `InspectorSnapshotResponse` to the shared
//! `impresario_inspector_core::Snapshot` model used by the UI, SSE deltas and
//! MCP views.
//!
//! Agent payloads are untrusted input. The converter is a total function:
//! unknown roles/axes fall back to defaults, malformed nodes are skipped and
//! recorded in `Snapshot.degraded`, and defensive limits bound both the work
//! and the resulting payload (TASK-INSPECTOR 13.1).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::str::FromStr as _;

use impresario_clients::convert::platform_from_proto;
use impresario_core::config::DeviceConfig;
use impresario_core::platforms::Platform;
use impresario_inspector_core::snapshot::{
    Degraded, DegradedReason, FocusModel, NodeState, ScreenInfo, ScrollAxis, ScrollInfo, Snapshot,
    SnapshotNode, SnapshotStats,
};
use impresario_locator::{Context, Rect, Role};
use impresario_proto::v1 as pb;

/// Hard cap on nodes kept from one acquisition (TASK-INSPECTOR 13.1).
const MAX_NODES: usize = 5_000;
/// Deepest node kept, counting a root as depth 0.
const MAX_DEPTH: u32 = 80;
/// Maximum characters kept per text field.
const MAX_TEXT_CHARS: usize = 4_096;
/// Maximum characters kept in a node's platform-native `raw` payload.
const MAX_RAW_CHARS: usize = 16_384;
/// Maximum characters kept in a device name or degraded message.
const MAX_MESSAGE_CHARS: usize = 1_024;
/// Maximum number of degraded entries emitted, including the summary.
const MAX_DEGRADED_ENTRIES: usize = 256;
/// Largest coordinate accepted from an agent, in logical units.
const MAX_COORD: f32 = 1_000_000.0;
/// Largest pixel dimension accepted after applying the pixel ratio.
const MAX_PIXEL_DIM: f32 = 100_000.0;
/// Largest accepted logical-to-pixel ratio.
const MAX_PIXEL_RATIO: f32 = 100.0;

/// Convert one wire snapshot into the normalized model.
#[must_use]
pub fn snapshot_from_proto(
    response: &pb::InspectorSnapshotResponse,
    device: &DeviceConfig,
) -> Snapshot {
    let platform = platform_from_proto(response.platform).unwrap_or(device.platform);
    let pixel_ratio = pixel_ratio(response.pixel_ratio);
    let logical_w = bounded_size(response.logical_width);
    let logical_h = bounded_size(response.logical_height);
    let screen = ScreenInfo {
        logical_w,
        logical_h,
        pixel_w: pixel_dim(logical_w, pixel_ratio),
        pixel_h: pixel_dim(logical_h, pixel_ratio),
        scale: pixel_ratio,
        rotation: 0,
        safe_insets: Default::default(),
        focus_model: focus_model(platform),
    };

    let mut log = DegradedLog::default();
    for entry in &response.degraded {
        log.push(degraded_from_wire(entry, response.truncated));
    }
    if response.truncated
        && !log
            .entries
            .iter()
            .any(|entry| entry.reason == DegradedReason::Partial)
    {
        log.push(Degraded {
            reason: DegradedReason::Partial,
            message: "the snapshot hit the node or depth budget".to_string(),
            node_path: None,
            partial: true,
        });
    }
    if let Some(error) = &response.error {
        let (message, _) = sanitize_message(&error.message, MAX_MESSAGE_CHARS);
        log.push(Degraded {
            reason: DegradedReason::Error,
            message: if message.is_empty() {
                format!("executor error {}", error.code)
            } else {
                message
            },
            node_path: None,
            partial: true,
        });
    }

    let retained = retain_nodes(response, &mut log);
    let mut nodes = Vec::with_capacity(retained.len());
    let mut text_truncated = false;
    let mut raw_truncated = false;
    let mut other_truncated = false;
    for index in retained {
        let Some(wire) = response.nodes.get(index) else {
            continue;
        };
        let (node, flags) = node_from_proto(wire, platform, response.focused_id);
        text_truncated |= flags.text;
        raw_truncated |= flags.raw;
        other_truncated |= flags.other;
        nodes.push(node);
    }
    if text_truncated || raw_truncated || other_truncated {
        log.push(Degraded {
            reason: DegradedReason::Partial,
            message: format!(
                "node text, identifiers or native payload was truncated for transport \
                 (text limit {MAX_TEXT_CHARS} characters, raw limit {MAX_RAW_CHARS} characters)"
            ),
            node_path: None,
            partial: true,
        });
    }

    let (device_name, _) = sanitize_message(&response.device_name, MAX_MESSAGE_CHARS);
    let stats = SnapshotStats {
        acquisition_ms: u64::from(response.acquisition_ms),
        node_count: nodes.len(),
        pruned_count: 0,
        payload_bytes: 0,
        delta_bytes: 0,
    };
    Snapshot::new(device.name.clone(), device_name, platform, screen, nodes)
        .with_rev(response.rev)
        .with_settled(response.settled)
        .with_degraded(log.finish())
        .with_stats(stats)
}

/// One node that survived duplicate-id filtering.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    /// Index into `response.nodes`.
    index: usize,
    id: u64,
    parent_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DepthState {
    Visiting,
    Done(u32),
    Invalid,
}

/// Filter malformed nodes and enforce the node/depth budgets.
///
/// The returned indexes point into `response.nodes` in input order. Every
/// retained node's parent is retained too, so the result is a valid forest.
fn retain_nodes(response: &pb::InspectorSnapshotResponse, log: &mut DegradedLog) -> Vec<usize> {
    let mut candidates = Vec::with_capacity(response.nodes.len());
    let mut first_by_id: BTreeMap<u64, usize> = BTreeMap::new();
    for (index, node) in response.nodes.iter().enumerate() {
        if node.id == 0 {
            log.push(malformed(index, 0, "id 0 is not a valid element id"));
            continue;
        }
        if let Some(first) = first_by_id.get(&node.id) {
            log.push(malformed(
                index,
                node.id,
                format!("duplicate id (first seen at nodes[{first}])"),
            ));
            continue;
        }
        first_by_id.insert(node.id, index);
        candidates.push(Candidate {
            index,
            id: node.id,
            parent_id: node.parent_id,
        });
    }

    let position_by_id: BTreeMap<u64, usize> = candidates
        .iter()
        .enumerate()
        .map(|(position, candidate)| (candidate.id, position))
        .collect();

    // Walk parent chains once per candidate. The state map memoizes the depth
    // and also detects cycles (a chain that revisits a `Visiting` id) and
    // missing parents.
    let mut state: BTreeMap<u64, DepthState> = BTreeMap::new();
    let mut stack: Vec<(u64, usize)> = Vec::new();
    for (start, start_candidate) in candidates.iter().enumerate() {
        if state.contains_key(&start_candidate.id) {
            continue;
        }
        stack.clear();
        let mut current = start;
        let base = loop {
            let Some(candidate) = candidates.get(current).copied() else {
                break None;
            };
            match state.get(&candidate.id).copied() {
                Some(DepthState::Done(depth)) => break Some(depth),
                Some(DepthState::Invalid) => {
                    for (id, position) in stack.drain(..) {
                        state.insert(id, DepthState::Invalid);
                        if let Some(position) = candidates.get(position) {
                            log.push(malformed(position.index, id, "parent chain is invalid"));
                        }
                    }
                    break None;
                }
                Some(DepthState::Visiting) => {
                    for (id, position) in stack.drain(..) {
                        state.insert(id, DepthState::Invalid);
                        if let Some(position) = candidates.get(position) {
                            log.push(malformed(
                                position.index,
                                id,
                                "parent chain contains a cycle",
                            ));
                        }
                    }
                    break None;
                }
                None => {
                    state.insert(candidate.id, DepthState::Visiting);
                    stack.push((candidate.id, current));
                    if candidate.parent_id == 0 {
                        state.insert(candidate.id, DepthState::Done(0));
                        stack.pop();
                        break Some(0);
                    }
                    match position_by_id.get(&candidate.parent_id).copied() {
                        Some(parent) => current = parent,
                        None => {
                            let missing = candidate.parent_id;
                            for (id, position) in stack.drain(..) {
                                state.insert(id, DepthState::Invalid);
                                let reason = if id == candidate.id {
                                    format!("references missing parent {missing}")
                                } else {
                                    format!(
                                        "ancestor {} references missing parent {missing}",
                                        candidate.id
                                    )
                                };
                                if let Some(position) = candidates.get(position) {
                                    log.push(malformed(position.index, id, reason));
                                }
                            }
                            break None;
                        }
                    }
                }
            }
        };
        if let Some(mut depth) = base {
            while let Some((id, position)) = stack.pop() {
                depth = depth.saturating_add(1);
                if depth > MAX_DEPTH {
                    state.insert(id, DepthState::Invalid);
                    if let Some(position) = candidates.get(position) {
                        log.push(malformed(
                            position.index,
                            id,
                            format!("exceeds the maximum depth of {MAX_DEPTH}"),
                        ));
                    }
                } else {
                    state.insert(id, DepthState::Done(depth));
                }
            }
        }
    }

    let valid: Vec<usize> = candidates
        .iter()
        .filter(|candidate| matches!(state.get(&candidate.id), Some(DepthState::Done(_))))
        .map(|candidate| candidate.index)
        .collect();

    if valid.len() <= MAX_NODES {
        return valid;
    }

    // Keep a parent-closed subset, breadth-first from the roots, so a kept
    // node never references a dropped parent. The final list is restored to
    // input order for stable rendering.
    let valid_ids: BTreeSet<u64> = valid
        .iter()
        .filter_map(|&index| response.nodes.get(index).map(|node| node.id))
        .collect();
    let mut children: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    for &index in &valid {
        let Some(node) = response.nodes.get(index) else {
            continue;
        };
        if node.parent_id == 0 {
            queue.push_back(index);
        } else if valid_ids.contains(&node.parent_id) {
            children.entry(node.parent_id).or_default().push(index);
        }
    }
    let mut kept = Vec::with_capacity(MAX_NODES);
    while let Some(index) = queue.pop_front() {
        if kept.len() >= MAX_NODES {
            break;
        }
        let Some(node) = response.nodes.get(index) else {
            continue;
        };
        kept.push(index);
        if let Some(kids) = children.get(&node.id) {
            queue.extend(kids.iter().copied());
        }
    }
    let dropped = valid.len().saturating_sub(kept.len());
    kept.sort_unstable();
    log.push(Degraded {
        reason: DegradedReason::Partial,
        message: format!(
            "snapshot exceeded the {MAX_NODES}-node limit; {dropped} nodes were dropped"
        ),
        node_path: None,
        partial: true,
    });
    kept
}

#[derive(Debug, Clone, Copy, Default)]
struct NodeFlags {
    text: bool,
    other: bool,
    raw: bool,
}

fn node_from_proto(
    node: &pb::InspectorNode,
    platform: Platform,
    focused_id: u64,
) -> (SnapshotNode, NodeFlags) {
    let mut flags = NodeFlags::default();
    let role = Role::from_str(&node.role).unwrap_or(Role::Generic);
    let (name, truncated) = sanitize_text(&node.name, MAX_TEXT_CHARS);
    flags.other |= truncated;
    let mut normalized = SnapshotNode::new(node.id, node.parent_id, role, name, rect(&node.bounds));

    let (text, truncated) = sanitize_text(&node.text, MAX_TEXT_CHARS);
    normalized.text = text;
    normalized.text_truncated = node.text_truncated || truncated;
    flags.text = truncated;

    sanitize_into(&mut normalized.value, &node.value, &mut flags);
    sanitize_into(&mut normalized.label, &node.label, &mut flags);
    sanitize_into(&mut normalized.description, &node.description, &mut flags);
    sanitize_into(&mut normalized.placeholder, &node.placeholder, &mut flags);
    sanitize_into(&mut normalized.alt, &node.alt, &mut flags);
    sanitize_into(&mut normalized.title, &node.title, &mut flags);
    sanitize_optional_into(&mut normalized.test_id, &node.test_id, &mut flags);
    sanitize_optional_into(&mut normalized.native_id, &node.raw_id, &mut flags);
    normalized.visible_fraction = finite_fraction(node.visible_fraction);
    normalized.state = NodeState {
        enabled: node.enabled,
        disabled: !node.enabled,
        focusable: false,
        focused: node.focused || (focused_id != 0 && node.id == focused_id),
        selected: node.selected,
        checked: node.checked,
        expanded: node.expanded,
        pressed: node.pressed,
        readonly: node.readonly,
        required: node.required,
        hidden_by_a11y: node.hidden,
    };
    normalized.scroll = node.scrollable.then(|| ScrollInfo {
        can_scroll_forward: node.can_scroll_forward,
        can_scroll_backward: node.can_scroll_backward,
        axis: scroll_axis(&node.scroll_axis),
    });
    normalized.context = if matches!(platform, Platform::Web | Platform::WebOs | Platform::Tizen) {
        Context::Web
    } else {
        Context::Native
    };
    normalized.children_hidden_count = node.children_hidden_count;
    normalized.escapes_clip = node.escapes_clip;
    normalized.system_ui = node.system_ui;
    normalized.secure = node.secure;
    sanitize_optional_into(&mut normalized.css_selector, &node.css_selector, &mut flags);
    sanitize_optional_into(&mut normalized.xpath, &node.xpath, &mut flags);

    let (raw, truncated) = sanitize_text(&node.raw, MAX_RAW_CHARS);
    flags.raw = truncated;
    if !raw.is_empty() {
        normalized.raw.insert("raw".to_string(), raw);
    }
    (normalized, flags)
}

fn rect(value: &Option<pb::InspectorRect>) -> Rect {
    value.as_ref().map_or_else(
        || Rect::new(0.0, 0.0, 0.0, 0.0),
        |rect| {
            Rect::new(
                bounded_coord(rect.x),
                bounded_coord(rect.y),
                bounded_size(rect.w),
                bounded_size(rect.h),
            )
        },
    )
}

fn sanitize_into(target: &mut String, value: &str, flags: &mut NodeFlags) {
    let (text, truncated) = sanitize_text(value, MAX_TEXT_CHARS);
    *target = text;
    flags.other |= truncated;
}

fn sanitize_optional_into(target: &mut Option<String>, value: &str, flags: &mut NodeFlags) {
    let (text, truncated) = sanitize_optional(value);
    *target = text;
    flags.other |= truncated;
}

fn sanitize_optional(value: &str) -> (Option<String>, bool) {
    let (text, truncated) = sanitize_message(value, MAX_TEXT_CHARS);
    if text.trim().is_empty() {
        (None, truncated)
    } else {
        (Some(text), truncated)
    }
}

/// Strip all control characters and truncate to `limit` characters. Used for
/// messages and identifiers, which must stay on one compact line.
fn sanitize_message(value: &str, limit: usize) -> (String, bool) {
    sanitize_with(value, limit, false)
}

/// Strip control characters (except common whitespace) and truncate to
/// `limit` characters. Bidi/format characters are preserved so RTL labels
/// keep their direction.
fn sanitize_text(value: &str, limit: usize) -> (String, bool) {
    sanitize_with(value, limit, true)
}

fn sanitize_with(value: &str, limit: usize, keep_whitespace: bool) -> (String, bool) {
    let mut out = String::with_capacity(value.len().min(limit.saturating_mul(4)));
    let mut count = 0usize;
    for ch in value.chars() {
        if ch.is_control() && !(keep_whitespace && matches!(ch, '\n' | '\r' | '\t')) {
            continue;
        }
        if count == limit {
            return (out, true);
        }
        out.push(ch);
        count += 1;
    }
    (out, false)
}

fn bounded_coord(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-MAX_COORD, MAX_COORD)
    } else {
        0.0
    }
}

fn bounded_size(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_COORD)
    } else {
        0.0
    }
}

fn finite_fraction(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn pixel_ratio(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value.min(MAX_PIXEL_RATIO)
    } else {
        1.0
    }
}

fn pixel_dim(logical: f32, ratio: f32) -> u32 {
    let pixels = (logical * ratio).round();
    if pixels.is_finite() {
        pixels.clamp(0.0, MAX_PIXEL_DIM) as u32
    } else {
        0
    }
}

fn focus_model(platform: Platform) -> FocusModel {
    match platform {
        Platform::Web => FocusModel::Pointer,
        Platform::WebOs | Platform::Tizen | Platform::AndroidTv | Platform::Tvos => {
            FocusModel::Dpad
        }
        Platform::Android | Platform::Ios => FocusModel::Both,
    }
}

fn scroll_axis(value: &str) -> ScrollAxis {
    match value.to_ascii_lowercase().as_str() {
        "horizontal" => ScrollAxis::Horizontal,
        "both" => ScrollAxis::Both,
        _ => ScrollAxis::Vertical,
    }
}

fn degraded_from_wire(entry: &str, truncated: bool) -> Degraded {
    let (code, message) = entry.split_once(':').unwrap_or(("ERROR", entry));
    let reason = match code.trim().to_ascii_uppercase().as_str() {
        "TIMEOUT" => DegradedReason::Timeout,
        "NOT_INSPECTABLE" => DegradedReason::NotInspectable,
        "FLUTTER_SEMANTICS_OFF" => DegradedReason::FlutterSemanticsOff,
        "UNSUPPORTED" => DegradedReason::Unsupported,
        "PARTIAL" => DegradedReason::Partial,
        _ => DegradedReason::Error,
    };
    let (message, _) = sanitize_message(message.trim(), MAX_MESSAGE_CHARS);
    Degraded {
        reason,
        message,
        node_path: None,
        partial: truncated || matches!(reason, DegradedReason::Timeout | DegradedReason::Partial),
    }
}

fn malformed(index: usize, id: u64, message: impl Into<String>) -> Degraded {
    let message: String = message.into();
    let (message, _) = sanitize_message(&format!("node {id}: {message}"), MAX_MESSAGE_CHARS);
    Degraded {
        reason: DegradedReason::Error,
        message,
        node_path: Some(format!("nodes[{index}]")),
        partial: true,
    }
}

#[derive(Debug, Default)]
struct DegradedLog {
    entries: Vec<Degraded>,
    omitted: usize,
}

impl DegradedLog {
    fn push(&mut self, entry: Degraded) {
        if self.entries.len() < MAX_DEGRADED_ENTRIES.saturating_sub(1) {
            self.entries.push(entry);
        } else {
            self.omitted = self.omitted.saturating_add(1);
        }
    }

    fn finish(mut self) -> Vec<Degraded> {
        if self.omitted > 0 {
            self.entries.push(Degraded {
                reason: DegradedReason::Partial,
                message: format!(
                    "{} additional degraded entries were omitted (limit {MAX_DEGRADED_ENTRIES})",
                    self.omitted
                ),
                node_path: None,
                partial: true,
            });
        }
        self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_inspector_core::compact::{compact, CompactOptions};
    use proptest::prelude::*;

    fn device() -> DeviceConfig {
        DeviceConfig::new("tv", Platform::WebOs, "127.0.0.1")
    }

    fn node(id: u64, parent_id: u64) -> pb::InspectorNode {
        pb::InspectorNode {
            id,
            parent_id,
            ..Default::default()
        }
    }

    #[test]
    fn converts_nodes_states_and_degradation() {
        let response = pb::InspectorSnapshotResponse {
            rev: 7,
            device_name: "Living room".to_string(),
            platform: pb::Platform::Webos as i32,
            logical_width: 1280.0,
            logical_height: 720.0,
            pixel_ratio: 2.0,
            settled: true,
            acquisition_ms: 12,
            nodes: vec![
                node(1, 0),
                pb::InspectorNode {
                    id: 4,
                    parent_id: 1,
                    role: "button".to_string(),
                    name: "Play".to_string(),
                    bounds: Some(pb::InspectorRect {
                        x: 10.0,
                        y: 20.0,
                        w: 100.0,
                        h: 40.0,
                    }),
                    focused: true,
                    enabled: true,
                    scrollable: true,
                    scroll_axis: "vertical".to_string(),
                    visible_fraction: 0.5,
                    ..Default::default()
                },
            ],
            focused_id: 4,
            degraded: vec!["PARTIAL: iframe".to_string()],
            truncated: true,
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        assert_eq!(snapshot.rev, 7);
        assert_eq!(snapshot.screen.pixel_w, 2560);
        assert_eq!(snapshot.platform, Platform::WebOs);
        let node = snapshot.find(4).expect("node");
        assert_eq!(node.role, Role::Button);
        assert!(node.state.focused);
        assert!(node.scroll.is_some());
        assert_eq!(snapshot.degraded.len(), 1);
        assert!(snapshot.degraded.first().is_some_and(|entry| entry.partial));
        assert!(snapshot.validate().is_ok());
    }

    #[test]
    fn focus_model_tracks_each_platform_interaction_style() {
        for (platform, expected) in [
            (Platform::Web, FocusModel::Pointer),
            (Platform::WebOs, FocusModel::Dpad),
            (Platform::Tizen, FocusModel::Dpad),
            (Platform::AndroidTv, FocusModel::Dpad),
            (Platform::Tvos, FocusModel::Dpad),
            (Platform::Android, FocusModel::Both),
            (Platform::Ios, FocusModel::Both),
        ] {
            assert_eq!(
                focus_model(platform),
                expected,
                "unexpected focus model for {platform}"
            );
        }
    }

    #[test]
    fn unknown_roles_and_axes_are_total() {
        let response = pb::InspectorSnapshotResponse {
            platform: pb::Platform::Android as i32,
            nodes: vec![pb::InspectorNode {
                id: 1,
                role: "future-role".to_string(),
                scrollable: true,
                scroll_axis: "diagonal".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        let node = snapshot.find(1).expect("node");
        assert_eq!(node.role, Role::Generic);
        assert_eq!(
            node.scroll.map(|scroll| scroll.axis),
            Some(ScrollAxis::Vertical)
        );
    }

    #[test]
    fn skips_malformed_nodes_and_records_paths() {
        let response = pb::InspectorSnapshotResponse {
            platform: pb::Platform::Android as i32,
            nodes: vec![
                node(1, 0),
                node(0, 0),
                node(1, 0),
                node(2, 9),
                node(3, 3),
                node(4, 5),
                node(5, 4),
                node(7, 4),
                node(6, 0),
            ],
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        let ids: Vec<u64> = snapshot.nodes.iter().map(|node| node.id).collect();
        assert_eq!(ids, vec![1, 6]);
        assert!(snapshot.validate().is_ok());
        assert!(snapshot
            .degraded
            .iter()
            .all(|entry| { entry.reason == DegradedReason::Error && entry.node_path.is_some() }));
        for index in [1usize, 2, 3, 4, 5, 6, 7] {
            let path = format!("nodes[{index}]");
            assert!(
                snapshot
                    .degraded
                    .iter()
                    .any(|entry| entry.node_path.as_deref() == Some(path.as_str())),
                "missing degraded entry for {path}"
            );
        }
    }

    #[test]
    fn accepts_children_before_parents() {
        let response = pb::InspectorSnapshotResponse {
            nodes: vec![node(2, 1), node(3, 2), node(1, 0)],
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        let ids: Vec<u64> = snapshot.nodes.iter().map(|node| node.id).collect();
        assert_eq!(ids, vec![2, 3, 1]);
        assert!(snapshot.validate().is_ok());
        assert_eq!(
            snapshot.find(2).and_then(|node| node.parent_index),
            snapshot.find_index(1)
        );
        assert_eq!(
            snapshot.find(3).and_then(|node| node.parent_index),
            snapshot.find_index(2)
        );
    }

    #[test]
    fn depth_limit_keeps_the_shallow_prefix() {
        let nodes = (1..=100).map(|id| node(id, id - 1)).collect();
        let response = pb::InspectorSnapshotResponse {
            nodes,
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        assert_eq!(snapshot.nodes.len(), (MAX_DEPTH as usize) + 1);
        assert_eq!(snapshot.nodes.last().map(|node| node.id), Some(81));
        assert!(snapshot.validate().is_ok());
        assert!(snapshot.degraded.iter().any(|entry| {
            entry.node_path.as_deref() == Some("nodes[81]")
                && entry.message.contains("maximum depth")
        }));
    }

    #[test]
    fn node_cap_is_parent_closed() {
        let mut nodes = Vec::new();
        for root in 0..3_000u64 {
            let root_id = 1 + root * 2;
            nodes.push(node(root_id, 0));
            nodes.push(node(root_id + 1, root_id));
        }
        let response = pb::InspectorSnapshotResponse {
            nodes,
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        assert_eq!(snapshot.nodes.len(), MAX_NODES);
        assert!(snapshot.validate().is_ok());
        assert_eq!(
            snapshot
                .nodes
                .iter()
                .filter(|node| node.parent_id == 0)
                .count(),
            3_000
        );
        for node in &snapshot.nodes {
            if node.parent_id != 0 {
                assert!(
                    snapshot.find(node.parent_id).is_some(),
                    "node {} lost parent {}",
                    node.id,
                    node.parent_id
                );
            }
        }
        assert!(snapshot.degraded.iter().any(|entry| {
            entry.reason == DegradedReason::Partial && entry.message.contains("5000-node limit")
        }));
    }

    #[test]
    fn sanitizes_geometry_text_and_raw() {
        let response = pb::InspectorSnapshotResponse {
            logical_width: f32::NAN,
            logical_height: f32::INFINITY,
            pixel_ratio: f32::NAN,
            nodes: vec![pb::InspectorNode {
                id: 1,
                parent_id: 0,
                role: "button".to_string(),
                name: "ok\u{0}\u{7}name".to_string(),
                text: "x".repeat(MAX_TEXT_CHARS + 10),
                bounds: Some(pb::InspectorRect {
                    x: f32::NAN,
                    y: f32::INFINITY,
                    w: -10.0,
                    h: f32::NAN,
                }),
                visible_fraction: f32::NAN,
                raw: "r".repeat(MAX_RAW_CHARS + 10),
                ..Default::default()
            }],
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        assert_eq!(snapshot.screen.logical_w, 0.0);
        assert_eq!(snapshot.screen.logical_h, 0.0);
        assert_eq!(snapshot.screen.scale, 1.0);
        assert_eq!(snapshot.screen.pixel_w, 0);
        assert_eq!(snapshot.screen.pixel_h, 0);

        let node = snapshot.find(1).expect("node");
        assert_eq!(node.name, "okname");
        assert_eq!(node.text.chars().count(), MAX_TEXT_CHARS);
        assert!(node.text_truncated);
        assert_eq!(node.bounds, Rect::new(0.0, 0.0, 0.0, 0.0));
        assert_eq!(node.visible_fraction, 0.0);
        assert_eq!(
            node.raw.get("raw").map(|raw| raw.chars().count()),
            Some(MAX_RAW_CHARS)
        );
        assert!(snapshot.degraded.iter().any(|entry| {
            entry.reason == DegradedReason::Partial && entry.message.contains("truncated")
        }));
    }

    #[test]
    fn clamps_large_finite_geometry() {
        let response = pb::InspectorSnapshotResponse {
            logical_width: f32::MAX,
            logical_height: f32::MAX,
            pixel_ratio: f32::MAX,
            nodes: vec![pb::InspectorNode {
                id: 1,
                parent_id: 0,
                bounds: Some(pb::InspectorRect {
                    x: f32::MAX,
                    y: f32::MIN,
                    w: f32::MAX,
                    h: f32::MIN,
                }),
                visible_fraction: 2.5,
                ..Default::default()
            }],
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        assert_eq!(snapshot.screen.logical_w, MAX_COORD);
        assert_eq!(snapshot.screen.logical_h, MAX_COORD);
        assert_eq!(snapshot.screen.scale, MAX_PIXEL_RATIO);
        assert_eq!(snapshot.screen.pixel_w, MAX_PIXEL_DIM as u32);
        let node = snapshot.find(1).expect("node");
        assert_eq!(node.bounds.x, MAX_COORD);
        assert_eq!(node.bounds.y, -MAX_COORD);
        assert_eq!(node.bounds.w, MAX_COORD);
        assert_eq!(node.bounds.h, 0.0);
        assert_eq!(node.visible_fraction, 1.0);
    }

    #[test]
    fn preserves_rtl_and_bidi_text() {
        let text = "مرحبا بالعالم \u{202E}ABC\u{202C} שלום \u{200F}";
        let response = pb::InspectorSnapshotResponse {
            nodes: vec![pb::InspectorNode {
                id: 1,
                parent_id: 0,
                name: text.to_string(),
                text: text.to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        let node = snapshot.find(1).expect("node");
        assert_eq!(node.name, text);
        assert_eq!(node.text, text);
    }

    #[test]
    fn caps_degraded_entries() {
        let response = pb::InspectorSnapshotResponse {
            nodes: (0..400).map(|_| node(0, 0)).collect(),
            ..Default::default()
        };
        let snapshot = snapshot_from_proto(&response, &device());
        assert!(snapshot.nodes.is_empty());
        assert_eq!(snapshot.degraded.len(), MAX_DEGRADED_ENTRIES);
        assert!(snapshot
            .degraded
            .last()
            .is_some_and(|entry| entry.message.contains("additional degraded entries")));
    }

    fn text_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            4 => any::<String>(),
            1 => prop::collection::vec(any::<char>(), 0..600)
                .prop_map(|chars| chars.into_iter().collect()),
            1 => Just("\u{202E}مرحبا\u{202C} שלום\u{200F}".to_string()),
        ]
    }

    fn node_strategy() -> impl Strategy<Value = pb::InspectorNode> {
        (
            0u64..8,
            0u64..12,
            any::<u64>(),
            text_strategy(),
            text_strategy(),
            any::<f32>(),
            any::<f32>(),
            any::<f32>(),
            any::<f32>(),
            any::<u32>(),
        )
            .prop_map(|(id, parent_id, seed, name, text, x, y, w, h, flags)| {
                pb::InspectorNode {
                    id,
                    parent_id,
                    role: match seed % 5 {
                        0 => "button".to_string(),
                        1 => "textbox".to_string(),
                        2 => "future-role".to_string(),
                        3 => String::new(),
                        _ => format!("role-{seed}"),
                    },
                    name: name.clone(),
                    text,
                    test_id: if flags & 1 != 0 {
                        name.clone()
                    } else {
                        String::new()
                    },
                    raw_id: if flags & 2 != 0 {
                        name.clone()
                    } else {
                        String::new()
                    },
                    bounds: Some(pb::InspectorRect { x, y, w, h }),
                    viewport_bounds: Some(pb::InspectorRect { x, y, w, h }),
                    focused: flags & 4 != 0,
                    enabled: flags & 8 != 0,
                    selected: flags & 16 != 0,
                    checked: (flags & 32 != 0).then_some(flags & 64 != 0),
                    expanded: (flags & 128 != 0).then_some(flags & 256 != 0),
                    pressed: (flags & 512 != 0).then_some(flags & 1_024 != 0),
                    readonly: flags & 2_048 != 0,
                    required: flags & 4_096 != 0,
                    hidden: flags & 8_192 != 0,
                    secure: flags & 16_384 != 0,
                    system_ui: flags & 32_768 != 0,
                    scrollable: flags & 65_536 != 0,
                    can_scroll_backward: flags & 131_072 != 0,
                    can_scroll_forward: flags & 262_144 != 0,
                    scroll_axis: match flags % 4 {
                        0 => "vertical",
                        1 => "horizontal",
                        2 => "both",
                        _ => "diagonal",
                    }
                    .to_string(),
                    children_hidden_count: flags % 1_000,
                    raw: if flags & 524_288 != 0 {
                        format!("{name}{name}{name}")
                    } else {
                        String::new()
                    },
                    value: name.clone(),
                    label: name.clone(),
                    description: name.clone(),
                    placeholder: name.clone(),
                    alt: name.clone(),
                    title: name.clone(),
                    css_selector: if flags & 1_048_576 != 0 {
                        name.clone()
                    } else {
                        String::new()
                    },
                    xpath: if flags & 2_097_152 != 0 {
                        name.clone()
                    } else {
                        String::new()
                    },
                    visible_fraction: h,
                    escapes_clip: flags & 4_194_304 != 0,
                    depth: flags % 100,
                    text_truncated: flags & 8_388_608 != 0,
                }
            })
    }

    fn response_strategy() -> impl Strategy<Value = pb::InspectorSnapshotResponse> {
        (
            (
                any::<u64>(),
                text_strategy(),
                any::<i32>(),
                any::<f32>(),
                any::<f32>(),
                any::<f32>(),
            ),
            (
                any::<bool>(),
                any::<u32>(),
                prop::collection::vec(node_strategy(), 0..120),
                prop::collection::vec(text_strategy(), 0..4),
            ),
            (
                any::<u64>(),
                any::<bool>(),
                prop::option::of(text_strategy()),
            ),
        )
            .prop_map(
                |(
                    (rev, device_name, platform, logical_width, logical_height, pixel_ratio),
                    (settled, acquisition_ms, nodes, degraded),
                    (focused_id, truncated, error_message),
                )| pb::InspectorSnapshotResponse {
                    rev,
                    device_name,
                    platform,
                    logical_width,
                    logical_height,
                    pixel_ratio,
                    settled,
                    acquisition_ms,
                    nodes,
                    degraded,
                    focused_id,
                    truncated,
                    error: error_message.map(|message| pb::ErrorInfo {
                        code: pb::ErrorCode::Internal as i32,
                        message,
                        details: Default::default(),
                    }),
                    ..Default::default()
                },
            )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn arbitrary_agent_payloads_are_total_and_well_formed(
            response in response_strategy()
        ) {
            let snapshot = snapshot_from_proto(&response, &device());
            assert!(snapshot.nodes.len() <= MAX_NODES);
            assert!(snapshot.validate().is_ok());
            assert!(snapshot.screen.logical_w.is_finite());
            assert!(snapshot.screen.logical_h.is_finite());
            assert!(snapshot.screen.scale.is_finite());
            assert!(snapshot.screen.scale > 0.0);
            assert!(snapshot.device_name.chars().count() <= MAX_MESSAGE_CHARS);

            let mut ids = BTreeSet::new();
            for node in &snapshot.nodes {
                assert_ne!(node.id, 0);
                assert!(ids.insert(node.id), "duplicate id {}", node.id);
                assert!(node.bounds.x.is_finite());
                assert!(node.bounds.y.is_finite());
                assert!(node.bounds.w.is_finite() && node.bounds.w >= 0.0);
                assert!(node.bounds.h.is_finite() && node.bounds.h >= 0.0);
                assert!(node.visible_fraction.is_finite());
                assert!((0.0..=1.0).contains(&node.visible_fraction));

                for value in [
                    &node.name,
                    &node.text,
                    &node.value,
                    &node.label,
                    &node.description,
                    &node.placeholder,
                    &node.alt,
                    &node.title,
                ] {
                    assert!(value.chars().count() <= MAX_TEXT_CHARS);
                    assert!(!value
                        .chars()
                        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')));
                }
                for value in [
                    node.test_id.as_deref(),
                    node.native_id.as_deref(),
                    node.css_selector.as_deref(),
                    node.xpath.as_deref(),
                ]
                .into_iter()
                .flatten()
                {
                    assert!(value.chars().count() <= MAX_TEXT_CHARS);
                }
                for raw in node.raw.values() {
                    assert!(raw.chars().count() <= MAX_RAW_CHARS);
                }

                if node.parent_id == 0 {
                    assert!(node.parent_index.is_none());
                } else {
                    let parent = snapshot
                        .find_index(node.parent_id)
                        .expect("retained node has a retained parent");
                    assert_eq!(node.parent_index, Some(parent));
                }

                let mut depth = 0u32;
                let mut current = node;
                let mut seen = BTreeSet::new();
                loop {
                    assert!(seen.insert(current.id), "cycle through node {}", current.id);
                    if current.parent_id == 0 {
                        break;
                    }
                    assert!(depth < MAX_DEPTH, "node {} exceeds the depth limit", node.id);
                    depth += 1;
                    current = snapshot
                        .find(current.parent_id)
                        .expect("retained node has a retained parent");
                }
            }
            for entry in &snapshot.degraded {
                assert!(entry.message.chars().count() <= MAX_MESSAGE_CHARS);
            }

            let json = serde_json::to_string(&snapshot).expect("snapshot serializes");
            assert!(!json.is_empty());
            let parsed = Snapshot::from_json(&json).expect("snapshot parses");
            assert!(parsed.validate().is_ok());
            assert_eq!(parsed.nodes, snapshot.nodes);

            let compact = compact(&snapshot, &CompactOptions::default());
            assert!(compact.lines().count() <= CompactOptions::default().max_nodes + 4);
        }
    }
}
