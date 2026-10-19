//! Tool definitions (`tools/list`) and the `tools/call` entry point.
//!
//! Every tool result carries a text block (pretty JSON, or a human summary
//! plus the JSON); tools that return a table also carry
//! `structuredContent`, and artifact reads may add an `image` block so an
//! agent can see failure screenshots.

use serde_json::{json, Map, Value};

use crate::mcp::handlers;
use crate::mcp::protocol::{RpcError, RpcResult, Server, INVALID_PARAMS};

/// The MCP tool list, in a fixed order (clients page `tools/list`, so the
/// order must not vary between calls).
pub(crate) fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "impresario_workspace_info",
            "title": "Workspace info",
            "description": "Summarize the workspace this server was started in: config \
        defaults (timeouts, retries, reporters, report output), configured devices with each \
        platform's action/check support matrix, scenes, plays with their acts and tags, and any \
        parse/validation diagnostics. Call this first to learn what can be run here.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "impresario_validate",
            "title": "Validate workspace",
            "description": "Parse and validate the workspace (or the given files/directories) \
        without touching any device. Returns core diagnostics with their codes (IMP-DSL-*, \
        IMP-CFG-*) and workspace-relative positions. Use after writing or editing .scene/.play \
        files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files or directories to validate, relative to the \
        workspace. Default: the configured scenes and plays directories.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_devices_list",
            "title": "List devices",
            "description": "List the devices from impresario.devices.config (name, platform, \
        target endpoint, app id/url, agent address, connector). No device is contacted; use \
        impresario_devices_check to actually reach them.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "impresario_devices_check",
            "title": "Check devices",
            "description": "For each named device (default: all): prepare the transport, \
        connect through the on-device agent, handshake, read DeviceInfo and tear down. Takes \
        seconds per device and contacts the hardware. Serialized with runs (one device-side \
        operation at a time).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "devices": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Device names from the devices config. Default: all.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_run",
            "title": "Run plays",
            "description": "Plan and execute plays on the selected devices through their \
        agents, then write reports to <report_output>/<run_id>. Returns a compact summary \
        (statuses, counts, failures with log tails, run_dir); fetch details with \
        impresario_report_get and impresario_artifact_get — failure screenshots come back as \
        images. Only file reporters are available over MCP (console output belongs to stdout, \
        which carries the protocol), default json+junit. One run at a time.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "plays": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Play file names, workspace-relative paths or glob \
        patterns (like --play). Default: every play in the plays directory.",
                    },
                    "devices": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Device names to run on (like --device). Default: all \
        configured devices.",
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only acts carrying one of these tags (like --tag).",
                    },
                    "exclude_tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Skip acts carrying one of these tags (like --exclude-tag).",
                    },
                    "acts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only acts with one of these names (like --act).",
                    },
                    "vars": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Variables that override scene pieces (name to value, \
        like --var name=value).",
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 100,
                        "maximum": 600000,
                        "description": "Per-step timeout override (like --timeout).",
                    },
                    "retries": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 5,
                        "description": "Act retries (like --retries). Default: config.",
                    },
                    "fail_fast": {
                        "type": "boolean",
                        "description": "Cancel the rest of the run on the first failure (like \
        --fail-fast). Default: false.",
                    },
                    "reporters": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["json", "junit", "html"] },
                        "description": "File reporters to write. Default: json and junit. \
        Cheap step: the console reporter is not available over MCP.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_report_get",
            "title": "Get run report",
            "description": "Read a run's report.json (the complete Appendix-B run document: \
        statuses, steps, failures, screenshots, log tails). Without run_id the newest run under \
        the report directory is used.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": {
                        "type": "string",
                        "description": "Run id (a directory name under report_output). \
        Default: the newest run.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_artifacts_list",
            "title": "List run artifacts",
            "description": "List the files of a run (report.json, junit.xml, report.html, \
        screenshots/<device>/*.png) with sizes and kinds. Without run_id the newest run is used.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": {
                        "type": "string",
                        "description": "Run id. Default: the newest run.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_artifact_get",
            "title": "Get run artifact",
            "description": "Read one artifact of a run. PNG/JPEG/WebP artifacts (failure \
        screenshots) come back as images so you can look at them; text artifacts come back as \
        text. Paths are relative to the run directory and may not escape it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Artifact path relative to the run directory, e.g. \
        report.json or screenshots/tv/failure-1.png.",
                    },
                    "run_id": {
                        "type": "string",
                        "description": "Run id. Default: the newest run.",
                    },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
    ]
}

/// `tools/call`: validate the envelope, run the tool, shape the result.
pub(crate) fn call(server: &Server, params: &Value) -> RpcResult {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "tools/call requires a string 'name'",
        ));
    };
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "tools/call 'arguments' must be an object",
            ));
        }
    };
    let output = match name {
        "impresario_workspace_info" => handlers::workspace_info(server, &arguments),
        "impresario_validate" => handlers::validate(server, &arguments),
        "impresario_devices_list" => handlers::devices_list(server, &arguments),
        "impresario_devices_check" => handlers::devices_check(server, &arguments),
        "impresario_run" => handlers::run(server, &arguments),
        "impresario_report_get" => handlers::report_get(server, &arguments),
        "impresario_artifacts_list" => handlers::artifacts_list(server, &arguments),
        "impresario_artifact_get" => handlers::artifact_get(server, &arguments),
        other => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown tool '{other}'"),
            ));
        }
    };
    Ok(output.into_result())
}

/// What a tool produced. Failures of the *operation* (a device did not
/// answer, a path does not exist, an argument is out of range) are tool
/// execution errors (`isError: true` in the result), not JSON-RPC errors.
pub(crate) struct ToolOutput {
    pub text: String,
    pub structured: Option<Value>,
    /// `(base64, mime type)` for image results.
    pub image: Option<(String, String)>,
    pub is_error: bool,
}

impl ToolOutput {
    pub(crate) fn json(value: Value) -> Self {
        let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
        Self {
            text,
            structured: Some(value),
            image: None,
            is_error: false,
        }
    }

    pub(crate) fn json_with_prefix(value: Value, prefix: impl Into<String>) -> Self {
        let mut output = Self::json(value);
        output.text = format!("{}\n{}", prefix.into(), output.text);
        output
    }

    pub(crate) fn message(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            structured: None,
            image: None,
            is_error: false,
        }
    }

    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self {
            text: message.into(),
            structured: None,
            image: None,
            is_error: true,
        }
    }

    pub(crate) fn image(
        base64: String,
        mime: impl Into<String>,
        caption: impl Into<String>,
    ) -> Self {
        Self {
            text: caption.into(),
            structured: None,
            image: Some((base64, mime.into())),
            is_error: false,
        }
    }

    pub(crate) fn into_result(self) -> Value {
        let mut result = Map::new();
        let mut content = Vec::new();
        if let Some((data, mime_type)) = self.image {
            content.push(json!({ "type": "image", "data": data, "mimeType": mime_type }));
        }
        content.push(json!({ "type": "text", "text": self.text }));
        result.insert("content".to_string(), Value::Array(content));
        if let Some(structured) = self.structured {
            result.insert("structuredContent".to_string(), structured);
        }
        if self.is_error {
            result.insert("isError".to_string(), Value::Bool(true));
        }
        Value::Object(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_unique_and_schemaful() {
        let tools = definitions();
        assert_eq!(tools.len(), 8);
        let mut names = std::collections::BTreeSet::new();
        for tool in &tools {
            let name = tool["name"].as_str().expect("name").to_string();
            assert!(names.insert(name.clone()), "duplicate {name}");
            assert_eq!(
                tool["inputSchema"]["type"],
                json!("object"),
                "{name} schema"
            );
            assert!(
                tool["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{name} description"
            );
        }
    }

    #[test]
    fn tool_output_shapes_text_structured_and_image() {
        let json_output = ToolOutput::json(json!({ "a": 1 })).into_result();
        assert_eq!(json_output["structuredContent"]["a"], json!(1));
        assert_eq!(json_output["content"][0]["type"], json!("text"));
        assert!(json_output.get("isError").is_none());

        let error_output = ToolOutput::error("boom").into_result();
        assert_eq!(error_output["isError"], json!(true));

        let image_output = ToolOutput::image("AAA".to_string(), "image/png", "cap").into_result();
        assert_eq!(image_output["content"][0]["type"], json!("image"));
        assert_eq!(image_output["content"][0]["mimeType"], json!("image/png"));
        assert_eq!(image_output["content"][1]["type"], json!("text"));
    }
}
