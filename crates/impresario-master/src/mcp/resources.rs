//! MCP resources: the reference documents an agent authors tests from,
//! plus the live workspace summary. The docs are compiled into the binary
//! (`include_str!`), so a connected agent always reads the documentation
//! that matches its server build.

use serde_json::{json, Value};

use crate::mcp::handlers;
use crate::mcp::protocol::{RpcError, RpcResult, Server, INTERNAL_ERROR, INVALID_PARAMS};

struct Doc {
    uri: &'static str,
    name: &'static str,
    description: &'static str,
    text: &'static str,
}

const WORKSPACE_INFO_URI: &str = "impresario://workspace/info";

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
];

pub(crate) fn list(_server: &Server) -> Value {
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
    Err(RpcError::new(
        INVALID_PARAMS,
        format!("unknown resource '{uri}'"),
    ))
}
