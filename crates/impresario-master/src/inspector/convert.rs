//! Conversion from the wire `InspectorSnapshotResponse` to the shared
//! `impresario_inspector_core::Snapshot` model used by the UI, SSE deltas and
//! MCP views.

use std::collections::BTreeMap;
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

/// Convert one wire snapshot into the normalized model.
#[must_use]
pub fn snapshot_from_proto(
    response: &pb::InspectorSnapshotResponse,
    device: &DeviceConfig,
) -> Snapshot {
    let platform = platform_from_proto(response.platform).unwrap_or(device.platform);
    let pixel_ratio = if response.pixel_ratio > 0.0 {
        response.pixel_ratio
    } else {
        1.0
    };
    let screen = ScreenInfo {
        logical_w: response.logical_width,
        logical_h: response.logical_height,
        pixel_w: (response.logical_width * pixel_ratio).round().max(0.0) as u32,
        pixel_h: (response.logical_height * pixel_ratio).round().max(0.0) as u32,
        scale: pixel_ratio,
        rotation: 0,
        safe_insets: Default::default(),
        focus_model: focus_model(platform),
    };

    let nodes: Vec<SnapshotNode> = response
        .nodes
        .iter()
        .map(|node| node_from_proto(node, platform, response.focused_id))
        .collect();
    let mut degraded = response
        .degraded
        .iter()
        .map(|entry| degraded_from_wire(entry, response.truncated))
        .collect::<Vec<_>>();
    if response.truncated
        && !degraded
            .iter()
            .any(|entry| entry.reason == DegradedReason::Partial)
    {
        degraded.push(Degraded {
            reason: DegradedReason::Partial,
            message: "the snapshot hit the node or depth budget".to_string(),
            node_path: None,
            partial: true,
        });
    }
    if let Some(error) = &response.error {
        degraded.push(Degraded {
            reason: DegradedReason::Error,
            message: if error.message.is_empty() {
                format!("executor error {}", error.code)
            } else {
                error.message.clone()
            },
            node_path: None,
            partial: true,
        });
    }

    let stats = SnapshotStats {
        acquisition_ms: u64::from(response.acquisition_ms),
        node_count: nodes.len(),
        pruned_count: 0,
        payload_bytes: 0,
        delta_bytes: 0,
    };
    Snapshot::new(
        device.name.clone(),
        response.device_name.clone(),
        platform,
        screen,
        nodes,
    )
    .with_rev(response.rev)
    .with_settled(response.settled)
    .with_degraded(degraded)
    .with_stats(stats)
}

fn node_from_proto(node: &pb::InspectorNode, platform: Platform, focused_id: u64) -> SnapshotNode {
    let role = Role::from_str(&node.role).unwrap_or(Role::Generic);
    let mut normalized = SnapshotNode::new(
        node.id,
        node.parent_id,
        role,
        node.name.clone(),
        rect(&node.bounds),
    );
    normalized.text = node.text.clone();
    normalized.text_truncated = node.text_truncated;
    normalized.value = node.value.clone();
    normalized.label = node.label.clone();
    normalized.description = node.description.clone();
    normalized.placeholder = node.placeholder.clone();
    normalized.alt = node.alt.clone();
    normalized.title = node.title.clone();
    normalized.test_id = non_empty(&node.test_id);
    normalized.native_id = non_empty(&node.raw_id);
    normalized.visible_fraction = node.visible_fraction.clamp(0.0, 1.0);
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
    normalized.css_selector = non_empty(&node.css_selector);
    normalized.xpath = non_empty(&node.xpath);
    if !node.raw.is_empty() {
        let mut raw = BTreeMap::new();
        raw.insert("raw".to_string(), node.raw.clone());
        normalized.raw = raw;
    }
    normalized
}

fn rect(value: &Option<pb::InspectorRect>) -> Rect {
    value.as_ref().map_or_else(
        || Rect::new(0.0, 0.0, 0.0, 0.0),
        |rect| Rect::new(rect.x, rect.y, rect.w, rect.h),
    )
}

fn non_empty(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
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
    Degraded {
        reason,
        message: message.trim().to_string(),
        node_path: None,
        partial: truncated || matches!(reason, DegradedReason::Timeout | DegradedReason::Partial),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceConfig {
        DeviceConfig::new("tv", Platform::WebOs, "127.0.0.1")
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
            nodes: vec![pb::InspectorNode {
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
            }],
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
}
