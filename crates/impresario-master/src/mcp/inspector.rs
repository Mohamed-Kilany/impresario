//! MCP bridge to a running `impresario inspector` process.
//!
//! The inspector owns the device lease and the live snapshot. MCP tools are
//! thin clients of its loopback JSON API: they discover the URL and token from
//! `~/.impresario/inspector.json`, call one endpoint and return its `data`
//! envelope. No tool here opens a device or a gRPC session directly, so the
//! UI and an agent always see the same revision.

use std::io::Read as _;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};

use super::protocol::Server;
use super::tools::ToolOutput;

/// One resolved inspector endpoint.
struct Session {
    url: String,
    token: Option<String>,
}

impl Session {
    fn discover(server: &Server) -> Result<Self, String> {
        let path = server.home.inspector_state_path();
        let source = std::fs::read_to_string(&path).map_err(|error| {
            format!("no inspector is running ({error}); start one with `impresario inspector`")
        })?;
        let state: Value = serde_json::from_str(&source)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
        let url = state
            .get("url")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("{} has no inspector url", path.display()))?
            .to_string();
        let token = state
            .get("token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        Ok(Self { url, token })
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>, String), String> {
        let url = format!("{}{}", self.url.trim_end_matches('/'), path);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(150))
            .build();
        let mut request = agent.request(method, &url);
        if let Some(token) = &self.token {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        let response = match body {
            Some(body) => {
                let encoded = serde_json::to_string(body)
                    .map_err(|error| format!("cannot encode the inspector request: {error}"))?;
                request
                    .set("Content-Type", "application/json")
                    .send_string(&encoded)
            }
            None => request.call(),
        };
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(error) => {
                return Err(format!(
                    "cannot reach the inspector at {}: {error}",
                    self.url
                ))
            }
        };
        let status = response.status();
        let content_type = response
            .header("content-type")
            .unwrap_or_default()
            .to_string();
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read the inspector response: {error}"))?;
        Ok((status, bytes, content_type))
    }

    fn json(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let (status, bytes, _) = self.request(method, path, body)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            format!("the inspector returned invalid JSON (HTTP {status}): {error}")
        })?;
        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(value.get("data").cloned().unwrap_or(Value::Null));
        }
        let error = value.get("error").unwrap_or(&Value::Null);
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("INSPECTOR_ERROR");
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the inspector rejected the request");
        Err(format!("{code}: {message}"))
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        self.json("GET", path, None)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.json("POST", path, Some(body))
    }
}

fn output(result: Result<Value, String>) -> ToolOutput {
    match result {
        Ok(value) => ToolOutput::json(value),
        Err(error) => ToolOutput::error(error),
    }
}

fn required_str<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("'{name}' is required"))
}

fn optional_str<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn optional_u64(arguments: &Value, name: &str) -> Option<u64> {
    arguments.get(name).and_then(Value::as_u64)
}

fn optional_bool(arguments: &Value, name: &str) -> Option<bool> {
    arguments.get(name).and_then(Value::as_bool)
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn query(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `inspector_list_devices`
pub(crate) fn list_devices(server: &Server, _arguments: &Value) -> ToolOutput {
    let result = Session::discover(server).and_then(|session| session.get("/api/devices"));
    output(result)
}

/// `inspector_attach`
pub(crate) fn attach(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let device = required_str(arguments, "device")?;
        let session = Session::discover(server)?;
        session.post("/api/attach", &json!({ "device": device }))
    })();
    output(result)
}

/// `inspector_detach`
pub(crate) fn detach(server: &Server, _arguments: &Value) -> ToolOutput {
    let result =
        Session::discover(server).and_then(|session| session.post("/api/detach", &json!({})));
    output(result)
}

/// `inspector_get_view`
pub(crate) fn get_view(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let session = Session::discover(server)?;
        let mut pairs = Vec::new();
        if let Some(max_nodes) = optional_u64(arguments, "max_nodes") {
            pairs.push(("max_nodes", max_nodes.to_string()));
        }
        if let Some(max_depth) = optional_u64(arguments, "max_depth") {
            pairs.push(("max_depth", max_depth.to_string()));
        }
        let suffix = if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", query(&pairs))
        };
        session.get(&format!("/api/view{suffix}"))
    })();
    output(result)
}

/// `inspector_wait_for_change`
pub(crate) fn wait_for_change(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let session = Session::discover(server)?;
        let mut pairs = Vec::new();
        if let Some(since) = optional_u64(arguments, "since") {
            pairs.push(("since", since.to_string()));
        }
        if let Some(timeout_ms) = optional_u64(arguments, "timeout_ms") {
            pairs.push(("timeout_ms", timeout_ms.to_string()));
        }
        let suffix = if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", query(&pairs))
        };
        session.get(&format!("/api/wait{suffix}"))
    })();
    output(result)
}

/// `inspector_get_screenshot`
pub(crate) fn get_screenshot(server: &Server, _arguments: &Value) -> ToolOutput {
    let result = (|| {
        let session = Session::discover(server)?;
        let (status, bytes, content_type) = session.request("GET", "/api/screenshot", None)?;
        if status != 200 {
            let message = String::from_utf8_lossy(&bytes);
            return Err(format!("screenshot failed (HTTP {status}): {message}"));
        }
        let mime = if content_type.trim().is_empty() {
            "image/png".to_string()
        } else {
            content_type
        };
        Ok((
            base64::engine::general_purpose::STANDARD.encode(&bytes),
            mime,
        ))
    })();
    match result {
        Ok((data, mime)) => ToolOutput::image(data, mime, "Current inspector frame"),
        Err(error) => ToolOutput::error(error),
    }
}

/// `inspector_find`
pub(crate) fn find(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let locator = required_str(arguments, "locator")?;
        let session = Session::discover(server)?;
        session.get(&format!("/api/locator/resolve?locator={}", encode(locator)))
    })();
    output(result)
}

/// `inspector_describe`
pub(crate) fn describe(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let reference = node_query(arguments)?;
        let session = Session::discover(server)?;
        session.get(&format!("/api/node?{reference}"))
    })();
    output(result)
}

/// `inspector_suggest_locators`
pub(crate) fn suggest_locators(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let reference = node_query(arguments)?;
        let session = Session::discover(server)?;
        session.get(&format!("/api/node/suggest?{reference}"))
    })();
    output(result)
}

/// `inspector_verify_locator`
pub(crate) fn verify_locator(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let locator = required_str(arguments, "locator")?;
        let session = Session::discover(server)?;
        session.get(&format!("/api/locator/verify?locator={}", encode(locator)))
    })();
    output(result)
}

fn node_query(arguments: &Value) -> Result<String, String> {
    if let Some(reference) = optional_str(arguments, "ref") {
        return Ok(format!("ref={}", encode(reference)));
    }
    if let Some(id) = optional_u64(arguments, "id") {
        return Ok(format!("id={id}"));
    }
    Err("'ref' or 'id' is required".to_string())
}

fn node_body(arguments: &Value, kind: &str) -> Result<Value, String> {
    let mut body = json!({ "kind": kind });
    let map = body
        .as_object_mut()
        .ok_or_else(|| "cannot build the input body".to_string())?;
    if let Some(reference) = optional_str(arguments, "ref") {
        map.insert("ref".to_string(), json!(reference));
    } else if let Some(id) = optional_u64(arguments, "id") {
        map.insert("node_id".to_string(), json!(id));
    }
    for name in ["x", "y", "end_x", "end_y"] {
        if let Some(value) = arguments.get(name).and_then(Value::as_f64) {
            map.insert(name.to_string(), json!(value));
        }
    }
    Ok(body)
}

/// `inspector_tap`
pub(crate) fn tap(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let body = node_body(arguments, "tap")?;
        let session = Session::discover(server)?;
        session.post("/api/input", &body)
    })();
    output(result)
}

/// `inspector_scroll`
pub(crate) fn scroll(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let mut body = node_body(arguments, "scroll")?;
        let map = body
            .as_object_mut()
            .ok_or_else(|| "cannot build the scroll body".to_string())?;
        if let Some(direction) = optional_str(arguments, "direction") {
            map.insert("direction".to_string(), json!(direction));
        }
        if let Some(count) = optional_u64(arguments, "count") {
            map.insert("count".to_string(), json!(count));
        }
        let session = Session::discover(server)?;
        session.post("/api/input", &body)
    })();
    output(result)
}

/// `inspector_key`
pub(crate) fn key(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let key = required_str(arguments, "key")?;
        let mut body = node_body(arguments, "key")?;
        let map = body
            .as_object_mut()
            .ok_or_else(|| "cannot build the key body".to_string())?;
        map.insert("key".to_string(), json!(key));
        if let Some(count) = optional_u64(arguments, "count") {
            map.insert("count".to_string(), json!(count));
        }
        let session = Session::discover(server)?;
        session.post("/api/input", &body)
    })();
    output(result)
}

/// `inspector_type`
pub(crate) fn type_text(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let text = required_str(arguments, "text")?;
        let mut body = node_body(arguments, "type")?;
        let map = body
            .as_object_mut()
            .ok_or_else(|| "cannot build the type body".to_string())?;
        map.insert("text".to_string(), json!(text));
        let session = Session::discover(server)?;
        session.post("/api/input", &body)
    })();
    output(result)
}

/// `inspector_scene_list`
pub(crate) fn scene_list(server: &Server, _arguments: &Value) -> ToolOutput {
    let result = Session::discover(server).and_then(|session| session.get("/api/scenes"));
    output(result)
}

/// `inspector_scene_create`
pub(crate) fn scene_create(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let path = required_str(arguments, "path")?;
        let name = required_str(arguments, "name")?;
        let platforms = arguments
            .get("platforms")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        let session = Session::discover(server)?;
        session.post(
            "/api/scenes/create",
            &json!({ "path": path, "name": name, "platforms": platforms }),
        )
    })();
    output(result)
}

fn preview_scene(server: &Server, arguments: &Value, op: Value) -> Result<Value, String> {
    let path = required_str(arguments, "path")?;
    let session = Session::discover(server)?;
    let mut body = json!({ "path": path, "op": op });
    if let Some(base_revision) = optional_str(arguments, "base_revision") {
        if let Some(map) = body.as_object_mut() {
            map.insert("base_revision".to_string(), json!(base_revision));
        }
    }
    let preview = session.post("/api/scenes/preview", &body)?;
    if optional_bool(arguments, "commit") != Some(true) {
        return Ok(preview);
    }
    let edit_id = preview
        .get("edit_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "the inspector preview has no edit id".to_string())?;
    let commit = session.post("/api/scenes/commit", &json!({ "edit_id": edit_id }))?;
    Ok(json!({ "preview": preview, "commit": commit }))
}

fn scene_op(arguments: &Value, kind: &str, fields: &[&str]) -> Result<Value, String> {
    let mut op = json!({ "kind": kind });
    let map = op
        .as_object_mut()
        .ok_or_else(|| "cannot build the scene operation".to_string())?;
    for name in fields {
        match *name {
            "confirm" => {
                if let Some(value) = optional_bool(arguments, name) {
                    map.insert(name.to_string(), json!(value));
                }
            }
            _ => {
                if let Some(value) = optional_str(arguments, name) {
                    map.insert(name.to_string(), json!(value));
                }
            }
        }
    }
    Ok(op)
}

/// `inspector_scene_add_element`
pub(crate) fn scene_add_element(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let locator = required_str(arguments, "locator")?;
        let mut op = scene_op(
            arguments,
            "add_element",
            &["row", "column", "platform", "seed"],
        )?;
        if let Some(map) = op.as_object_mut() {
            map.insert("locator".to_string(), json!(locator));
        }
        preview_scene(server, arguments, op)
    })();
    output(result)
}

/// `inspector_scene_fill_cell`
pub(crate) fn scene_fill_cell(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let locator = required_str(arguments, "locator")?;
        let mut op = scene_op(arguments, "fill_cell", &["row", "column", "platform"])?;
        if let Some(map) = op.as_object_mut() {
            map.insert("locator".to_string(), json!(locator));
        }
        preview_scene(server, arguments, op)
    })();
    output(result)
}

/// `inspector_scene_replace_cell`
pub(crate) fn scene_replace_cell(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let locator = required_str(arguments, "locator")?;
        let mut op = scene_op(
            arguments,
            "replace_cell",
            &["row", "column", "platform", "confirm"],
        )?;
        if let Some(map) = op.as_object_mut() {
            map.insert("locator".to_string(), json!(locator));
        }
        preview_scene(server, arguments, op)
    })();
    output(result)
}

/// `inspector_scene_validate`
pub(crate) fn scene_validate(server: &Server, arguments: &Value) -> ToolOutput {
    let result = (|| {
        let path = required_str(arguments, "path")?;
        let session = Session::discover(server)?;
        let value = session.post(
            "/api/scenes/preview",
            &json!({ "path": path, "op": { "kind": "validate" } }),
        )?;
        Ok(value
            .get("document")
            .cloned()
            .unwrap_or_else(|| json!({ "path": path, "valid": true })))
    })();
    output(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(
            encode("role=button[name=\"Play\"]"),
            "role%3Dbutton%5Bname%3D%22Play%22%5D"
        );
        assert_eq!(
            query(&[("locator", "text=Hi there".to_string())]),
            "locator=text%3DHi%20there"
        );
    }

    #[test]
    fn required_fields_are_validated() {
        assert_eq!(required_str(&json!({ "device": "tv" }), "device"), Ok("tv"));
        assert!(required_str(&json!({ "device": " " }), "device").is_err());
    }

    #[test]
    fn node_queries_accept_refs_and_ids() {
        assert_eq!(
            node_query(&json!({ "ref": "e7" })),
            Ok("ref=e7".to_string())
        );
        assert_eq!(node_query(&json!({ "id": 7 })), Ok("id=7".to_string()));
        assert!(node_query(&json!({})).is_err());
    }
}
