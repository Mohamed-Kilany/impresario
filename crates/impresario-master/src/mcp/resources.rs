//! MCP resources: the reference documents an agent authors tests from,
//! plus the live workspace summary. The docs are compiled into the binary
//! (`include_str!`), so a connected agent always reads the documentation
//! that matches its server build.

use std::time::Duration;

use serde_json::{json, Value};

use crate::mcp::handlers;
use crate::mcp::inspector;
use crate::mcp::protocol::{RpcError, RpcResult, Server, INTERNAL_ERROR, INVALID_PARAMS};

struct Doc {
    uri: &'static str,
    name: &'static str,
    description: &'static str,
    text: &'static str,
}

const WORKSPACE_INFO_URI: &str = "impresario://workspace/info";
const VIEW_PREFIX: &str = "impresario://inspector/";
const VIEW_SUFFIX: &str = "/view";
/// Long-poll window for the view resource watcher.
const VIEW_WAIT_MS: u64 = 5_000;

/// `impresario://inspector/{device}/view`
fn view_uri(device: &str) -> String {
    format!("{VIEW_PREFIX}{}{VIEW_SUFFIX}", inspector::encode(device))
}

/// The device segment of a view URI, or `None` when the shape is wrong.
fn view_device(uri: &str) -> Option<String> {
    let device = uri.strip_prefix(VIEW_PREFIX)?.strip_suffix(VIEW_SUFFIX)?;
    if device.is_empty() || device.contains('/') {
        return None;
    }
    inspector::decode(device)
}

const DOCS: &[Doc] = &[
    Doc {
        uri: "impresario://docs/dsl",
        name: "Impresario DSL reference",
        description: "Scene/play syntax, selectors, checks, actions, variables and the \
diagnostic code list — everything needed to author .scene/.play files.",
        text: include_str!("../../../../docs/DSL.md"),
    },
    Doc {
        uri: "impresario://docs/parity",
        name: "Platform support matrix",
        description: "Which actions and checks work on each platform (Android, Android TV, \
web, webOS, Tizen, iOS, tvOS), generated from the planner's own tables.",
        text: include_str!("../../../../docs/PARITY.md"),
    },
    Doc {
        uri: "impresario://docs/devices",
        name: "Device setup guide",
        description: "Per-platform setup and evidence: Android over adb/Wi-Fi, Android TV, \
web/headless Chrome, webOS/Tizen, emulators, stay-awake and vendor-freeze notes.",
        text: include_str!("../../../../docs/DEVICES.md"),
    },
    Doc {
        uri: "impresario://docs/mcp",
        name: "MCP server guide",
        description: "How this server is meant to be used: tool reference, client wiring, \
transport, concurrency and security notes.",
        text: include_str!("../../../../docs/MCP.md"),
    },
    Doc {
        uri: "impresario://docs/web",
        name: "Web console guide",
        description: "The embedded browser console (`impresario web`): guided start-up flow, \
screens, JSON API envelope, action catalogue, security model and AI-agent workflow.",
        text: include_str!("../../../../docs/WEB.md"),
    },
    Doc {
        uri: "impresario://docs/scheduling",
        name: "Schedules, watchers and resident delivery",
        description: "Resident runner reference: layered schedule/watcher config, local-time \
and cron firing, missed/busy policies, dumps and retention, notifications, quiet hours, rate \
limits and webhooks.",
        text: include_str!("../../../../docs/SCHEDULING.md"),
    },
    Doc {
        uri: "impresario://docs/probes",
        name: "Probe evidence and verdict model",
        description: "Probe kinds, evidence sources and confidence, per-platform reachability, \
fallback rungs and the in-app SDK contract.",
        text: include_str!("../../../../docs/PROBES.md"),
    },
];

pub(crate) fn list(server: &Server) -> Value {
    let mut resources: Vec<Value> = DOCS
        .iter()
        .map(|doc| {
            json!({
                "uri": doc.uri,
                "name": doc.name,
                "description": doc.description,
                "mimeType": "text/markdown",
                "size": doc.text.len(),
            })
        })
        .collect();
    resources.push(json!({
        "uri": WORKSPACE_INFO_URI,
        "name": "Workspace info",
        "description": "The live workspace summary: config defaults, devices with their \
    support matrices, scenes, plays, acts and diagnostics (same as the \
    impresario_workspace_info tool).",
        "mimeType": "application/json",
    }));
    if let Some(attachment) = inspector::attached(server) {
        let uri = view_uri(&attachment.device);
        resources.push(json!({
            "uri": uri,
            "name": format!("Inspector view - {}", attachment.device),
            "description": "The attached device's current compact viewport view. \
        Subscribe with resources/subscribe to receive notifications/resources/updated \
        whenever its revision changes (same as inspector_get_view).",
            "mimeType": "application/json",
            "_meta": { "impresario/device": attachment.device, "impresario/rev": attachment.rev },
        }));
    }
    json!({ "resources": resources })
}

pub(crate) fn read(server: &Server, params: &Value) -> RpcResult {
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "resources/read requires a string 'uri'",
        ));
    };
    if let Some(doc) = DOCS.iter().find(|doc| doc.uri == uri) {
        return Ok(json!({
            "contents": [{
                "uri": doc.uri,
                "mimeType": "text/markdown",
                "text": doc.text,
            }],
        }));
    }
    if uri == WORKSPACE_INFO_URI {
        return match handlers::workspace_info_value(server) {
            Ok(value) => {
                let text =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
                Ok(json!({
                    "contents": [{
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": text,
                    }],
                }))
            }
            Err(message) => Err(RpcError::new(INTERNAL_ERROR, message)),
        };
    }
    if let Some(device) = view_device(uri) {
        let value = inspector::view(server, &device).map_err(|message| {
            RpcError::new(
                INTERNAL_ERROR,
                format!("cannot read inspector view '{uri}': {message}"),
            )
        })?;
        let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
        return Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": text,
            }],
        }));
    }
    Err(RpcError::new(
        INVALID_PARAMS,
        format!("unknown resource '{uri}'"),
    ))
}

fn required_uri(params: &Value) -> Result<&str, RpcError> {
    params
        .get("uri")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            RpcError::new(
                INVALID_PARAMS,
                "resources/subscribe requires a string 'uri'",
            )
        })
}

/// `resources/subscribe`: the live inspector view is the only resource that
/// changes. Static docs and the workspace summary are read-only snapshots.
pub(crate) fn subscribe(server: &Server, params: &Value) -> RpcResult {
    let uri = required_uri(params)?;
    let Some(device) = view_device(uri) else {
        return Err(RpcError::new(
            INVALID_PARAMS,
            format!(
                "resource '{uri}' is not subscribable; use impresario://inspector/{{device}}/view"
            ),
        ));
    };
    match inspector::attached(server) {
        Some(attachment) if attachment.device == device => {}
        Some(attachment) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!(
                    "the inspector is attached to '{}', not '{device}'",
                    attachment.device
                ),
            ))
        }
        None => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "no inspector device is attached; start `impresario inspector` and attach first",
            ))
        }
    }
    server.subscribe(uri);
    start_view_watch(server, uri);
    Ok(json!({}))
}

/// `resources/unsubscribe`: idempotent, like the MCP spec's "stop sending
/// updates" semantics.
pub(crate) fn unsubscribe(server: &Server, params: &Value) -> RpcResult {
    let uri = required_uri(params)?;
    server.unsubscribe(uri);
    Ok(json!({}))
}

/// Start the single watcher for `uri`; it exits when the subscription is
/// removed. Dispatch-only tests have no notifier and never spawn a thread.
fn start_view_watch(server: &Server, uri: &str) {
    if server.notifier.is_none() || !server.begin_watch(uri) {
        return;
    }
    let Some(device) = view_device(uri) else {
        server.end_watch(uri);
        return;
    };
    let server = server.clone();
    let watched = uri.to_string();
    let spawned = std::thread::Builder::new()
        .name("impresario-mcp-inspector-watch".to_string())
        .spawn(move || {
            let mut since = None;
            while server.subscribed(&watched) {
                match inspector::wait_for_view_change(&server, &device, since, VIEW_WAIT_MS) {
                    Ok((rev, changed)) => {
                        since = Some(rev);
                        if changed && server.subscribed(&watched) {
                            server.notify_resource_updated(&watched);
                        }
                    }
                    Err(_) => {
                        // Detached, replaced by another device or a transient
                        // HTTP failure: retry without spinning and resync the
                        // revision so a restarted inspector cannot hide changes.
                        since = None;
                        std::thread::sleep(Duration::from_millis(750));
                    }
                }
            }
            server.end_watch(&watched);
        });
    if spawned.is_err() {
        server.end_watch(uri);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use impresario_core::home::Home;

    use super::*;

    fn server() -> Server {
        Server::new(
            PathBuf::from("."),
            PathBuf::from("impresario.config"),
            PathBuf::from("impresario.devices.config"),
            Home::at("."),
        )
    }

    #[test]
    fn view_uris_round_trip_and_reject_bad_shapes() {
        let uri = view_uri("android tv/one");
        assert_eq!(uri, "impresario://inspector/android%20tv%2Fone/view");
        // An encoded slash stays inside the segment, so it is still one device.
        assert_eq!(view_device(&uri).as_deref(), Some("android tv/one"));
        assert_eq!(
            view_device("impresario://inspector/phone/view").as_deref(),
            Some("phone")
        );
        assert_eq!(view_device("impresario://inspector//view"), None);
        assert_eq!(view_device("impresario://inspector/phone"), None);
        assert_eq!(view_device("impresario://inspector/a/b/view"), None);
        assert_eq!(
            view_device("impresario://inspector/%zz/view"),
            None,
            "malformed escapes are rejected"
        );
    }

    #[test]
    fn subscriptions_are_tracked_per_uri_and_static_docs_are_refused() {
        let server = server();
        let uri = view_uri("phone");
        server.subscribe(&uri);
        assert!(server.subscribed(&uri));
        server.unsubscribe(&uri);
        assert!(!server.subscribed(&uri));

        let refused = subscribe(&server, &json!({ "uri": "impresario://docs/dsl" }))
            .expect_err("static docs cannot be subscribed to");
        assert_eq!(refused.code, INVALID_PARAMS);
        let missing = subscribe(&server, &json!({})).expect_err("uri is required");
        assert_eq!(missing.code, INVALID_PARAMS);
    }

    #[test]
    fn watch_registration_is_idempotent() {
        let server = server();
        let uri = view_uri("phone");
        assert!(server.begin_watch(&uri));
        assert!(!server.begin_watch(&uri));
        server.end_watch(&uri);
        assert!(server.begin_watch(&uri));
    }
}
