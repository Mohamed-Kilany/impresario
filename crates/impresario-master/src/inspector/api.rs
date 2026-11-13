//! HTTP routing for the inspector Web UI and the loopback MCP bridge.
//!
//! The transport is the same hand-written HTTP/1.1 implementation the web
//! console uses: one request per connection, no keep-alive. Live updates use
//! Server-Sent Events because the SPA is a same-origin client and `EventSource`
//! needs no extra protocol layer. The JSON shape is stable and shared with the
//! MCP tools.

use std::io::Write as _;
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use impresario_core::ast::Direction;
use impresario_core::keys::RemoteKey;
use impresario_inspector_core::compact::{compact, CompactOptions};
use impresario_inspector_core::snapshot::{Snapshot, SnapshotNode};
use impresario_locator::{parse as parse_locator, SynthesisOptions};
use impresario_proto::v1 as pb;
use serde_json::{json, Value};

use super::{assets, scene, ApiError, DeviceCommand, Shared};
use crate::web::http::{self, Request, Response};

const WAIT_DEFAULT_MS: u64 = 20_000;
const WAIT_MAX_MS: u64 = 120_000;
const COMPACT_MAX_NODES: usize = 2_000;

/// Route one HTTP request.
pub(crate) fn handle(shared: &Shared, request: &Request) -> Response {
    if !host_allowed(shared, request.header("host")) {
        return error_response(ApiError::new(
            403,
            "HOST_NOT_ALLOWED",
            "this inspector only answers its configured host name",
            None,
        ));
    }
    if is_public_asset(&request.path) {
        return asset_response(&request.path);
    }
    if !authorized(shared, request) {
        return error_response(ApiError::unauthorized());
    }
    if request.method == "GET" || request.method == "HEAD" {
        return match route_get(shared, request) {
            Ok(response) => response,
            Err(error) => error_response(error),
        };
    }
    if request.method != "POST" {
        return error_response(ApiError::new(
            405,
            "METHOD_NOT_ALLOWED",
            format!("{} is not supported", request.method),
            None,
        ));
    }
    if !same_origin(request) {
        return error_response(ApiError::new(
            403,
            "ORIGIN_NOT_ALLOWED",
            "the Origin header must match the Host header",
            None,
        ));
    }
    if !content_type_json(request) {
        return error_response(ApiError::new(
            415,
            "UNSUPPORTED_MEDIA_TYPE",
            "mutations require Content-Type: application/json",
            None,
        ));
    }
    match route_post(shared, request) {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

fn route_get(shared: &Shared, request: &Request) -> Result<Response, ApiError> {
    match request.path.as_str() {
        "/healthz" => Ok(ok(health_json(shared))),
        "/api/status" => Ok(ok(status_json(shared))),
        "/api/devices" => Ok(ok(devices_json(shared))),
        "/api/snapshot" => Ok(ok(snapshot_json(shared, request)?)),
        "/api/view" => Ok(ok(view_json(shared, request)?)),
        "/api/node" => Ok(ok(describe_json(shared, request)?)),
        "/api/node/suggest" => Ok(ok(suggest_json(shared, request)?)),
        "/api/screenshot" => screenshot(shared),
        "/api/wait" => Ok(ok(wait_json(shared, request)?)),
        "/api/locator/resolve" => {
            let locator = required_param(request, "locator")?;
            Ok(ok(resolve_json(shared, &locator)?))
        }
        "/api/locator/verify" => {
            let locator = required_param(request, "locator")?;
            Ok(ok(verify_json(shared, &locator)?))
        }
        "/api/scenes" => Ok(ok(scene::list(shared)?)),
        "/api/scenes/open" => {
            let path = required_param(request, "path")?;
            Ok(ok(scene::document(shared, &path)?))
        }
        "/api/audit" => Ok(ok(audit_json(shared))),
        "/api/config" => Ok(ok(config_json(shared))),
        other => Err(ApiError::not_found(format!("unknown route '{other}'"))),
    }
}

fn route_post(shared: &Shared, request: &Request) -> Result<Response, ApiError> {
    let body = json_body(request)?;
    match request.path.as_str() {
        "/api/attach" => {
            let device = body
                .get("device")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| request.param("device").map(str::to_string))
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| ApiError::bad_request("'device' is required"))?;
            shared
                .command(|reply| DeviceCommand::Attach {
                    name: device,
                    reply,
                })
                .map_err(device_error)?;
            Ok(ok(status_json(shared)))
        }
        "/api/detach" => {
            shared
                .command(|reply| DeviceCommand::Detach { reply })
                .map_err(device_error)?;
            Ok(ok(status_json(shared)))
        }
        "/api/input" => Ok(ok(input_json(shared, &body)?)),
        "/api/freeze" => {
            let frozen = body
                .get("frozen")
                .and_then(Value::as_bool)
                .ok_or_else(|| ApiError::bad_request("'frozen' must be true or false"))?;
            // Freeze only stops the client from applying live updates; the
            // device loop keeps acquiring so unfreeze is instant
            // (TASK-INSPECTOR 10.4). The flag is reported in status.
            shared.frozen.store(frozen, Ordering::Relaxed);
            Ok(ok(status_json(shared)))
        }
        "/api/locator/resolve" => {
            let locator = body
                .get("locator")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::bad_request("'locator' is required"))?;
            Ok(ok(resolve_json(shared, locator)?))
        }
        "/api/locator/verify" => {
            let locator = body
                .get("locator")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::bad_request("'locator' is required"))?;
            Ok(ok(verify_json(shared, locator)?))
        }
        "/api/scenes/create" => Ok(ok(scene::create(shared, &body)?)),
        "/api/scenes/preview" | "/api/scenes/validate" => Ok(ok(scene::preview(shared, &body)?)),
        "/api/scenes/commit" => Ok(ok(scene::commit(shared, &body)?)),
        "/api/scenes/undo" => Ok(ok(scene::undo(shared, &body)?)),
        other => Err(ApiError::not_found(format!("unknown route '{other}'"))),
    }
}

fn ok(data: Value) -> Response {
    Response::json(200, json!({ "ok": true, "data": data }))
}

fn error_response(error: ApiError) -> Response {
    Response::json(error.status, error.to_value())
}

fn is_public_asset(path: &str) -> bool {
    matches!(
        path,
        "/" | "/index.html" | "/assets/app.css" | "/assets/app.js" | "/favicon.svg"
    )
}

fn asset_response(path: &str) -> Response {
    match path {
        "/assets/app.css" => Response::text(200, "text/css; charset=utf-8", assets::APP_CSS),
        "/assets/app.js" => Response::text(200, "text/javascript; charset=utf-8", assets::APP_JS),
        "/favicon.svg" => Response::text(200, "image/svg+xml", assets::FAVICON_SVG),
        _ => Response::text(200, "text/html; charset=utf-8", assets::INDEX_HTML),
    }
}

fn health_json(shared: &Shared) -> Value {
    let status = shared.status();
    json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "url": &shared.url,
        "workspace": shared.base.display().to_string(),
        "device": status.device,
        "platform": status.platform,
        "attached": status.attached,
        "rev": status.rev,
        "nodes": status.nodes,
        "frozen": status.frozen,
        "error": status.error,
    })
}

fn status_json(shared: &Shared) -> Value {
    json!({
        "url": &shared.url,
        "bind": &shared.bind,
        "port": shared.port,
        "loopback": shared.loopback,
        "workspace": shared.base.display().to_string(),
        "version": env!("CARGO_PKG_VERSION"),
        "started_at": &shared.started_at,
        "uptime_ms": shared.started.elapsed().as_millis() as u64,
        "status": shared.status(),
        "frozen": shared.frozen.load(Ordering::Relaxed),
        "poll_interval_ms": shared.config.inspector.poll_interval().as_millis() as u64,
        "include_system_ui": shared.config.inspector.include_system_ui,
        "min_visible_percent": shared.config.inspector.min_visible_percent,
        "screenshot_scale": shared.config.inspector.screenshot_scale,
        "fps": shared.config.inspector.fps,
        "token_required": !shared.loopback,
        "locator_grammar": "role= text= testid= within(...) >> focused",
        "keys": RemoteKey::ALL.iter().map(|key| key.as_str()).collect::<Vec<_>>(),
        "directions": ["up", "down", "left", "right"],
    })
}

fn devices_json(shared: &Shared) -> Value {
    let status = shared.status();
    let devices: Vec<Value> = shared
        .devices
        .iter()
        .map(|device| {
            let attached =
                status.attached && status.device.as_deref() == Some(device.name.as_str());
            json!({
                "name": &device.name,
                "platform": device.platform.as_str(),
                "type": device.device_type.as_str(),
                "ip": &device.ip,
                "attached": attached,
                "lease": if attached { "interactive" } else { "none" },
            })
        })
        .collect();
    json!({
        "devices": devices,
        "attached": status.device,
        "app": &shared.app_name,
    })
}

fn snapshot_json(shared: &Shared, request: &Request) -> Result<Value, ApiError> {
    let snapshot = latest(shared)?;
    let include_raw =
        request.param("include_raw") == Some("1") || request.param("include_raw") == Some("true");
    if request.param("format") == Some("compact") {
        return Ok(view_value(
            &snapshot,
            query_usize(request, "max_nodes", 400),
            query_usize(request, "max_depth", 32),
        ));
    }
    let mut value = serde_json::to_value(&*snapshot)
        .map_err(|error| ApiError::internal(format!("cannot serialize the snapshot: {error}")))?;
    if let Value::Object(map) = &mut value {
        if let Some(Value::Array(nodes)) = map.get_mut("nodes") {
            for node in nodes {
                if let Value::Object(node) = node {
                    if !include_raw {
                        node.remove("raw");
                    }
                    if node.get("secure").and_then(Value::as_bool) == Some(true) {
                        node.insert("value".to_string(), json!("***"));
                    }
                }
            }
        }
        map.insert("status".to_string(), status_json(shared));
    }
    Ok(value)
}

fn view_json(shared: &Shared, request: &Request) -> Result<Value, ApiError> {
    let snapshot = latest(shared)?;
    Ok(view_value(
        &snapshot,
        query_usize(request, "max_nodes", 400),
        query_usize(request, "max_depth", 32),
    ))
}

fn view_value(snapshot: &Snapshot, max_nodes: usize, max_depth: usize) -> Value {
    let options = CompactOptions {
        max_nodes: max_nodes.clamp(1, COMPACT_MAX_NODES),
        max_depth: max_depth.clamp(1, 80),
        include_test_id: true,
    };
    let text = compact(snapshot, &options);
    json!({
        "text": text,
        "rev": snapshot.rev,
        "settled": snapshot.settled,
        "nodes": snapshot.nodes.len(),
        "degraded": &snapshot.degraded,
        "truncated": snapshot.degraded.iter().any(|entry| entry.partial),
    })
}

fn describe_json(shared: &Shared, request: &Request) -> Result<Value, ApiError> {
    let snapshot = latest(shared)?;
    let id = query_id(request)?;
    let node = find_node(&snapshot, id)?;
    let children = snapshot.children_of(id).len();
    let mut value = serde_json::to_value(node)
        .map_err(|error| ApiError::internal(format!("cannot serialize the node: {error}")))?;
    if let Value::Object(map) = &mut value {
        map.insert("ref".to_string(), json!(format!("e{id}")));
        map.insert("children".to_string(), json!(children));
        if node.secure {
            map.insert("value".to_string(), json!("***"));
        }
    }
    Ok(value)
}

fn suggest_json(shared: &Shared, request: &Request) -> Result<Value, ApiError> {
    let snapshot = latest(shared)?;
    let id = query_id(request)?;
    let node = find_node(&snapshot, id)?;
    let options = SynthesisOptions {
        max_candidates: 12,
        screen: (snapshot.screen.logical_w, snapshot.screen.logical_h),
        allow_css: true,
        allow_xpath: false,
        allow_region: true,
    };
    let suggestions = snapshot.suggest(id, &options);
    Ok(json!({
        "ref": format!("e{id}"),
        "role": node.role.as_str(),
        "name": node.name,
        "suggestions": suggestions,
    }))
}

fn resolve_json(shared: &Shared, locator: &str) -> Result<Value, ApiError> {
    let snapshot = latest(shared)?;
    let parsed = parse_locator(locator).map_err(locator_error)?;
    let matches = snapshot.resolve(&parsed).map_err(locator_error)?;
    let count = matches.len();
    Ok(json!({
        "locator": locator,
        "rev": snapshot.rev,
        "count": count,
        "unique": count == 1,
        "matches": matches,
    }))
}

fn verify_json(shared: &Shared, locator: &str) -> Result<Value, ApiError> {
    let snapshot = latest(shared)?;
    let parsed = parse_locator(locator).map_err(locator_error)?;
    let matches = snapshot.resolve(&parsed).map_err(locator_error)?;
    let count = matches.len();
    let unique = count == 1;
    let visible = matches.first().is_some_and(|entry| entry.visible);
    Ok(json!({
        "locator": locator,
        "rev": snapshot.rev,
        "count": count,
        "unique": unique,
        "visible": visible,
        "stable": unique && visible,
        "matches": matches,
    }))
}

fn wait_json(shared: &Shared, request: &Request) -> Result<Value, ApiError> {
    let since = query_u64(request, "since", shared.status().rev);
    let timeout = query_u64(request, "timeout_ms", WAIT_DEFAULT_MS).min(WAIT_MAX_MS);
    let rev = shared.wait_for_rev(since, Duration::from_millis(timeout));
    let changed = rev > since;
    let view = if changed {
        shared
            .latest_snapshot()
            .map(|snapshot| view_value(&snapshot, 120, 24))
    } else {
        None
    };
    Ok(json!({
        "changed": changed,
        "since": since,
        "rev": rev,
        "status": shared.status(),
        "view": view,
    }))
}

fn input_json(shared: &Shared, body: &Value) -> Result<Value, ApiError> {
    let kind = match body.get("kind").and_then(Value::as_str) {
        Some("tap") => pb::InspectorInputKind::Tap,
        Some("scroll") => pb::InspectorInputKind::Scroll,
        Some("key") => pb::InspectorInputKind::Key,
        Some("type") => pb::InspectorInputKind::Type,
        Some("swipe") => pb::InspectorInputKind::Swipe,
        Some(other) => {
            return Err(ApiError::bad_request(format!(
                "unknown input kind '{other}'"
            )))
        }
        None => return Err(ApiError::bad_request("'kind' is required")),
    };
    let node_id = body
        .get("ref")
        .and_then(parse_ref)
        .or_else(|| body.get("node_id").and_then(Value::as_u64))
        .unwrap_or(0);
    let key = if kind == pb::InspectorInputKind::Key {
        let name = body
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::bad_request("'key' is required for key input"))?;
        impresario_clients::convert::remote_key_to_proto(parse_key(name)?)
    } else {
        pb::RemoteKey::Unspecified
    };
    let direction = if kind == pb::InspectorInputKind::Scroll {
        body.get("direction")
            .and_then(Value::as_str)
            .map(parse_direction)
            .transpose()?
            .map_or_else(
                || pb::Direction::Unspecified,
                impresario_clients::convert::direction_to_proto,
            )
    } else {
        pb::Direction::Unspecified
    };
    let request = pb::InspectorInputRequest {
        kind: kind as i32,
        node_id,
        x: number(body, "x"),
        y: number(body, "y"),
        end_x: number(body, "end_x"),
        end_y: number(body, "end_y"),
        text: body
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        key: key as i32,
        direction: direction as i32,
        count: body
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .min(u64::from(u32::MAX)) as u32,
        duration_ms: body
            .get("duration_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32,
    };
    let response = shared
        .command(|reply| DeviceCommand::Input { request, reply })
        .map_err(device_error)?;
    if !response.ok {
        let error = response.error.as_ref().map_or_else(
            || ApiError::new(422, "INPUT_FAILED", "the input action failed", None),
            |error| {
                let wire = match impresario_clients::convert::error_info_from_proto(error) {
                    Ok(wire) | Err(wire) => wire,
                };
                ApiError::new(422, wire.code.as_str(), wire.message, None)
            },
        );
        return Err(error);
    }
    let view = shared
        .latest_snapshot()
        .map(|snapshot| view_value(&snapshot, 120, 24));
    Ok(json!({
        "ok": true,
        "rev": response.rev,
        "status": shared.status(),
        "view": view,
    }))
}

fn screenshot(shared: &Shared) -> Result<Response, ApiError> {
    let response = shared
        .command(|reply| DeviceCommand::Screenshot { reply })
        .map_err(device_error)?;
    if response.screenshot_png.is_empty() {
        return Err(ApiError::new(
            404,
            "NO_SCREENSHOT",
            "the executor did not return a screenshot",
            None,
        ));
    }
    let rev = response.rev;
    let width = response.screenshot_width;
    let height = response.screenshot_height;
    Ok(Response {
        status: 200,
        content_type: "image/png".to_string(),
        body: response.screenshot_png,
        extra_headers: vec![
            ("X-Inspector-Rev".to_string(), rev.to_string()),
            ("X-Inspector-Width".to_string(), width.to_string()),
            ("X-Inspector-Height".to_string(), height.to_string()),
        ],
    })
}

fn audit_json(shared: &Shared) -> Value {
    let audit = shared
        .audit
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let entries: Vec<&scene::AuditEntry> = audit.iter().rev().take(200).collect();
    json!({ "entries": entries })
}

fn config_json(shared: &Shared) -> Value {
    let apps: Vec<&str> = shared.apps.iter().map(|app| app.name.as_str()).collect();
    json!({
        "inspector": &shared.config.inspector,
        "apps": apps,
        "app": &shared.app_name,
        "scenes_dir": &shared.config.scenes_dir,
        "workspace": shared.base.display().to_string(),
        "limits": {
            "max_body_bytes": 2 * 1024 * 1024,
            "wait_max_ms": WAIT_MAX_MS,
            "compact_max_nodes": COMPACT_MAX_NODES,
        },
    })
}

fn latest(shared: &Shared) -> Result<std::sync::Arc<Snapshot>, ApiError> {
    shared.latest_snapshot().ok_or_else(|| {
        ApiError::new(
            404,
            "NO_SNAPSHOT",
            "no snapshot has been acquired yet; attach a device first",
            None,
        )
    })
}

fn find_node(snapshot: &Snapshot, id: u64) -> Result<&SnapshotNode, ApiError> {
    snapshot.find(id).ok_or_else(|| {
        ApiError::new(
            404,
            "STALE_REF",
            format!("node e{id} is not in the current snapshot"),
            Some(json!({ "ref": format!("e{id}"), "rev": snapshot.rev })),
        )
    })
}

fn query_id(request: &Request) -> Result<u64, ApiError> {
    let raw = request
        .param("id")
        .or_else(|| request.param("ref"))
        .ok_or_else(|| ApiError::bad_request("'id' or 'ref' is required"))?;
    parse_ref_str(raw).ok_or_else(|| ApiError::bad_request(format!("'{raw}' is not a node ref")))
}

fn required_param(request: &Request, name: &str) -> Result<String, ApiError> {
    request
        .param(name)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request(format!("'{name}' is required")))
}

fn query_u64(request: &Request, name: &str, default: u64) -> u64 {
    request
        .param(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn query_usize(request: &Request, name: &str, default: usize) -> usize {
    request
        .param(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn parse_ref(value: &Value) -> Option<u64> {
    match value {
        Value::String(text) => parse_ref_str(text),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

fn parse_ref_str(text: &str) -> Option<u64> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix('e').unwrap_or(trimmed);
    digits.parse().ok()
}

fn number(body: &Value, name: &str) -> f32 {
    body.get(name)
        .and_then(Value::as_f64)
        .map_or(0.0, |value| value as f32)
}

fn parse_key(name: &str) -> Result<RemoteKey, ApiError> {
    RemoteKey::ALL
        .iter()
        .copied()
        .find(|key| key.as_str().eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| {
            ApiError::new(
                422,
                "UNKNOWN_KEY",
                format!("unknown remote key '{name}'"),
                Some(json!({ "keys": RemoteKey::ALL.iter().map(|key| key.as_str()).collect::<Vec<_>>() })),
            )
        })
}

fn parse_direction(name: &str) -> Result<Direction, ApiError> {
    match name.trim().to_ascii_lowercase().as_str() {
        "up" => Ok(Direction::Up),
        "down" => Ok(Direction::Down),
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        other => Err(ApiError::new(
            422,
            "UNKNOWN_DIRECTION",
            format!("unknown direction '{other}'"),
            Some(json!({ "directions": ["up", "down", "left", "right"] })),
        )),
    }
}

fn locator_error(error: impresario_locator::LocatorError) -> ApiError {
    ApiError::new(
        422,
        "LOCATOR_INVALID",
        error.to_string(),
        Some(json!({ "locator_error": error.to_string() })),
    )
}

fn device_error(message: String) -> ApiError {
    let status = if message.starts_with("NOT_SUPPORTED") {
        422
    } else if message.starts_with("unknown device") {
        404
    } else {
        503
    };
    let code = if message.starts_with("NOT_SUPPORTED") {
        "NOT_SUPPORTED"
    } else {
        "DEVICE_ERROR"
    };
    ApiError::new(status, code, message, None)
}

fn json_body(request: &Request) -> Result<Value, ApiError> {
    if request.body.is_empty() {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_slice(&request.body)
        .map_err(|error| ApiError::bad_request(format!("invalid JSON body: {error}")))?;
    if !value.is_object() {
        return Err(ApiError::bad_request(
            "the request body must be a JSON object",
        ));
    }
    Ok(value)
}

fn content_type_json(request: &Request) -> bool {
    request
        .header("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
}

fn host_allowed(shared: &Shared, host: Option<&str>) -> bool {
    if !shared.loopback {
        return host.is_some_and(|value| !value.trim().is_empty());
    }
    let Some(host) = host else {
        return false;
    };
    let name = host_name(host);
    name.eq_ignore_ascii_case("localhost")
        || name == "127.0.0.1"
        || name == "::1"
        || name == "[::1]"
}

fn host_name(host: &str) -> &str {
    let trimmed = host.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some((name, _)) = rest.split_once(']') {
            return name;
        }
        return trimmed;
    }
    match trimmed.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|value| value.is_ascii_digit()) => name,
        _ => trimmed,
    }
}

fn authorized(shared: &Shared, request: &Request) -> bool {
    if shared.loopback {
        return true;
    }
    let Some(expected) = shared.token.as_deref() else {
        return false;
    };
    let header = request.header("authorization").and_then(|value| {
        value
            .strip_prefix("Bearer ")
            .or_else(|| value.strip_prefix("bearer "))
    });
    let provided = header
        .or_else(|| request.header("x-impresario-token"))
        .or_else(|| request.param("token"));
    provided.is_some_and(|value| constant_time_eq(value, expected))
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

fn same_origin(request: &Request) -> bool {
    let Some(origin) = request.header("origin") else {
        return true;
    };
    let Some(host) = request.header("host") else {
        return false;
    };
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    if authority.eq_ignore_ascii_case(host) {
        return true;
    }
    let default_port = if scheme.eq_ignore_ascii_case("https") {
        "443"
    } else {
        "80"
    };
    normalize_authority(authority, default_port)
        .eq_ignore_ascii_case(&normalize_authority(host, default_port))
}

fn normalize_authority(authority: &str, default_port: &str) -> String {
    match authority.rsplit_once(':') {
        Some((name, port)) if !name.contains(']') || name.ends_with(']') => {
            if port == default_port {
                name.to_string()
            } else {
                format!("{name}:{port}")
            }
        }
        _ => authority.to_string(),
    }
}

/// Stream live updates as Server-Sent Events until the client disconnects.
pub(crate) fn serve_events(stream: &mut TcpStream, shared: &Shared, request: &Request) {
    if !host_allowed(shared, request.header("host")) {
        let _ = http::write_response(
            stream,
            &error_response(ApiError::new(
                403,
                "HOST_NOT_ALLOWED",
                "this inspector only answers its configured host name",
                None,
            )),
            false,
        );
        return;
    }
    if !authorized(shared, request) {
        let _ = http::write_response(stream, &error_response(ApiError::unauthorized()), false);
        return;
    }
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream; charset=utf-8\r\n\
                Cache-Control: no-store\r\n\
                Connection: close\r\n\
                X-Accel-Buffering: no\r\n\
                X-Content-Type-Options: nosniff\r\n\
                X-Frame-Options: DENY\r\n\
                Referrer-Policy: no-referrer\r\n\
                Content-Security-Policy: default-src 'none'\r\n\r\n";
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let receiver = shared.subscribe();
    let hello = json!({
        "kind": "hello",
        "url": &shared.url,
        "status": shared.status(),
        "token_required": !shared.loopback,
    });
    if send_event(stream, "hello", &hello).is_err() {
        return;
    }
    if let Some(snapshot) = shared.latest_snapshot() {
        let initial = json!({ "snapshot": &*snapshot, "status": shared.status() });
        if send_event(stream, "snapshot", &initial).is_err() {
            return;
        }
    }
    loop {
        match receiver.recv_timeout(Duration::from_secs(15)) {
            Ok(text) => {
                if write!(stream, "event: update\ndata: {text}\n\n")
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if stream
                    .write_all(b": keepalive\n\n")
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn send_event(stream: &mut TcpStream, name: &str, data: &Value) -> std::io::Result<()> {
    write!(stream, "event: {name}\ndata: {data}\n\n")?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_parse_with_or_without_the_e_prefix() {
        assert_eq!(parse_ref_str("e12"), Some(12));
        assert_eq!(parse_ref_str("12"), Some(12));
        assert_eq!(parse_ref_str("nope"), None);
    }

    #[test]
    fn host_names_strip_ports_and_brackets() {
        assert_eq!(host_name("127.0.0.1:7311"), "127.0.0.1");
        assert_eq!(host_name("[::1]:7311"), "::1");
        assert_eq!(host_name("localhost"), "localhost");
    }

    #[test]
    fn tokens_compare_exactly() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secreT"));
        assert!(!constant_time_eq("secret", "secret "));
    }

    #[test]
    fn directions_and_keys_are_total_for_known_names() {
        assert!(matches!(parse_direction("down"), Ok(Direction::Down)));
        assert!(parse_direction("sideways").is_err());
        assert!(matches!(parse_key("back"), Ok(RemoteKey::Back)));
        assert!(matches!(parse_key("OK"), Ok(RemoteKey::Ok)));
        assert!(parse_key("teleport").is_err());
    }
}
