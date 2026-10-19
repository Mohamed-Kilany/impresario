//! MCP wire handling: dual-era protocol support (2026-07-28 stateless
//! requests plus the legacy `initialize` handshake), JSON-RPC shaping and
//! method dispatch for everything except the tool/resource bodies.

use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::mcp::{resources, tools};

/// A dual-era MCP server bound to one workspace.
#[derive(Debug, Clone)]
pub(crate) struct Server {
    pub base: PathBuf,
    pub config_path: PathBuf,
    pub devices_path: PathBuf,
}

/// JSON-RPC error codes this server emits; `UNSUPPORTED_VERSION` is the
/// 2026-07-28 revision's `UnsupportedProtocolVersionError`.
pub(crate) const PARSE_ERROR: i64 = -32700;
pub(crate) const INVALID_REQUEST: i64 = -32600;
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const INTERNAL_ERROR: i64 = -32603;
pub(crate) const UNSUPPORTED_VERSION: i64 = -32022;

/// The stateless revision this server speaks: every request carries
/// `_meta.io.modelcontextprotocol/protocolVersion` and results are tagged
/// `resultType: "complete"`; clients discover the server with
/// `server/discover` instead of an `initialize` handshake.
const MODERN_VERSION: &str = "2026-07-28";
/// Newest handshake revision; the counter-offer for anything older that is
/// not in `LEGACY_VERSIONS`.
const LATEST_LEGACY_VERSION: &str = "2025-11-25";
/// Handshake revisions this server accepts from `initialize`.
const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

const NAME: &str = "impresario";

const INSTRUCTIONS: &str = "\
Impresario drives cross-platform UI tests through an on-device agent \
(Android phones and TVs over adb, web/Chromium, and Apple platforms where a \
Mac is available). Workflow: read impresario://docs/dsl (language), \
impresario://docs/parity (what each platform supports) and \
impresario://docs/devices (hardware setup); author .scene/.play files in \
the workspace; call impresario_validate; call impresario_devices_check; \
call impresario_run; triage failures with impresario_report_get, \
impresario_artifacts_list and impresario_artifact_get — failure \
screenshots come back as images. Paths are workspace-relative unless \
absolute; one run or device check executes at a time.";

/// Which protocol era a request speaks. Legacy clients never send `_meta`,
/// so the choice is per request; an `initialize` request pins legacy
/// semantics for the rest of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Era {
    Modern,
    Legacy,
}

/// A JSON-RPC error produced while handling a request.
#[derive(Debug)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub(crate) fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

pub(crate) type RpcResult = Result<Value, RpcError>;

fn server_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn server_info() -> Value {
    json!({
        "name": NAME,
        "title": "Impresario UI test server",
        "version": server_version(),
    })
}

fn capabilities() -> Value {
    json!({
        "tools": { "listChanged": false },
        "resources": { "subscribe": false, "listChanged": false },
    })
}

/// The era of one request, or the error to answer with when the client
/// declared a version this server does not implement.
fn era_of(params: &Value) -> Result<Era, RpcError> {
    let version = params
        .get("_meta")
        .and_then(|meta| meta.get(META_VERSION))
        .and_then(Value::as_str);
    match version {
        None => Ok(Era::Legacy),
        Some(MODERN_VERSION) => Ok(Era::Modern),
        Some(version) if LEGACY_VERSIONS.contains(&version) => Ok(Era::Legacy),
        Some(version) => Err(RpcError::new(
            UNSUPPORTED_VERSION,
            format!("protocol version '{version}' is not supported"),
        )
        .with_data(json!({ "supported": [MODERN_VERSION] }))),
    }
}

/// Handle one parsed JSON-RPC message. `None` means "no response" (a
/// notification, or a request without an id, which JSON-RPC treats as a
/// notification).
pub(crate) fn dispatch(server: &Server, message: &Value) -> Option<Value> {
    let Some(object) = message.as_object() else {
        return Some(error_response(
            Value::Null,
            INVALID_REQUEST,
            "batch requests are not part of MCP; send one JSON-RPC object per line",
            None,
        ));
    };
    let id = object.get("id").cloned();
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return id.map(|id| error_response(id, INVALID_REQUEST, "missing 'method'", None));
    };
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    if method.starts_with("notifications/") {
        tracing::debug!("mcp: ignoring {method}");
        return None;
    }
    let era = match era_of(&params) {
        Ok(era) => era,
        Err(error) => return id.map(|id| error_value(id, error)),
    };
    // Requests carry an id; anything without one is a notification and is
    // never answered.
    let id = id?;

    let result: RpcResult = match method {
        "initialize" => match era {
            Era::Legacy => Ok(initialize_result(&params)),
            Era::Modern => Err(RpcError::new(
                METHOD_NOT_FOUND,
                "initialize belongs to the handshake era; 2026-07-28 clients call server/discover",
            )),
        },
        "server/discover" => Ok(discover_result()),
        "ping" => match era {
            Era::Legacy => Ok(json!({})),
            Era::Modern => Err(RpcError::new(
                METHOD_NOT_FOUND,
                "ping was removed in protocol revision 2026-07-28",
            )),
        },
        "tools/list" => Ok(json!({ "tools": tools::definitions() })),
        "tools/call" => tools::call(server, &params),
        "resources/list" => Ok(resources::list(server)),
        "resources/read" => resources::read(server, &params),
        other => Err(RpcError::new(
            METHOD_NOT_FOUND,
            format!("unknown method '{other}'"),
        )),
    };
    Some(match result {
        Ok(result) => success_response(id, result, era),
        Err(error) => error_value(id, error),
    })
}

/// Legacy `initialize`: pick a handshake revision both sides know, or make
/// the spec's counter-offer when the client asked for an unknown one.
fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(LATEST_LEGACY_VERSION);
    let negotiated = if LEGACY_VERSIONS.contains(&requested) {
        requested
    } else {
        LATEST_LEGACY_VERSION
    };
    json!({
        "protocolVersion": negotiated,
        "capabilities": capabilities(),
        "serverInfo": server_info(),
        "instructions": INSTRUCTIONS,
    })
}

/// 2026-07-28 `server/discover`: supported versions, capabilities,
/// identity. The response is already modern-shaped (`resultType`), whether
/// or not the probe carried `_meta`.
fn discover_result() -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": [MODERN_VERSION],
        "capabilities": capabilities(),
        "_meta": { META_SERVER_INFO: server_info() },
        "instructions": INSTRUCTIONS,
    })
}

/// Wrap a result; modern requests get the `resultType` tag the 2026-07-28
/// revision requires, legacy requests stay untagged.
fn success_response(id: Value, mut result: Value, era: Era) -> Value {
    if era == Era::Modern {
        if let Some(object) = result.as_object_mut() {
            object
                .entry("resultType".to_string())
                .or_insert_with(|| json!("complete"));
        }
    }
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub(crate) fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = Map::new();
    error.insert("code".to_string(), json!(code));
    error.insert("message".to_string(), json!(message));
    if let Some(data) = data {
        error.insert("data".to_string(), data);
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": Value::Object(error) })
}

fn error_value(id: Value, error: RpcError) -> Value {
    error_response(id, error.code, &error.message, error.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn era_negotiation_reads_per_request_meta() {
        assert_eq!(era_of(&json!({})).expect("no meta"), Era::Legacy);
        assert_eq!(
            era_of(&json!({
                "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
            }))
            .expect("modern"),
            Era::Modern
        );
        assert_eq!(
            era_of(&json!({
                "_meta": { "io.modelcontextprotocol/protocolVersion": "2025-11-25" }
            }))
            .expect("legacy revision in meta"),
            Era::Legacy
        );
        let error = era_of(&json!({
            "_meta": { "io.modelcontextprotocol/protocolVersion": "1900-01-01" }
        }))
        .expect_err("unsupported version");
        assert_eq!(error.code, UNSUPPORTED_VERSION);
        assert_eq!(error.data, Some(json!({ "supported": [MODERN_VERSION] })));
    }

    #[test]
    fn initialize_counter_offers_the_latest_legacy_revision() {
        let known = initialize_result(&json!({ "protocolVersion": "2024-11-05" }));
        assert_eq!(known["protocolVersion"], json!("2024-11-05"));
        for params in [json!({ "protocolVersion": "2099-01-01" }), json!({})] {
            let negotiated = initialize_result(&params);
            assert_eq!(negotiated["protocolVersion"], json!(LATEST_LEGACY_VERSION));
        }
    }

    #[test]
    fn results_are_tagged_only_for_modern_requests() {
        let modern = success_response(json!(1), json!({ "tools": [] }), Era::Modern);
        assert_eq!(modern["result"]["resultType"], json!("complete"));
        let legacy = success_response(json!(1), json!({ "tools": [] }), Era::Legacy);
        assert!(legacy["result"].get("resultType").is_none());
    }

    #[test]
    fn dispatch_shapes_notifications_and_discover() {
        let server = Server {
            base: PathBuf::from("."),
            config_path: PathBuf::from("impresario.config"),
            devices_path: PathBuf::from("impresario.devices.config"),
        };
        let discover = dispatch(
            &server,
            &json!({ "jsonrpc": "2.0", "id": 7, "method": "server/discover", "params": {} }),
        )
        .expect("response");
        assert_eq!(discover["result"]["resultType"], json!("complete"));
        assert!(dispatch(
            &server,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .is_none());
        // A request without an id is a notification: no response.
        assert!(dispatch(&server, &json!({ "jsonrpc": "2.0", "method": "ping" })).is_none());
        // A batch is not MCP: invalid request with a null id.
        let batch = dispatch(&server, &json!([{ "method": "ping" }])).expect("response");
        assert_eq!(batch["error"]["code"], json!(INVALID_REQUEST));
    }
}
