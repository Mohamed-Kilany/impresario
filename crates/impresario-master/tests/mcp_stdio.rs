//! The MCP server, end to end: a spawned `impresario mcp` child speaking
//! newline-delimited JSON-RPC on stdio, a `MockAgent` standing in for the
//! device, and the full author → validate → check → run → debug loop
//! (decision I-80).
//!
//! Every stdout line of the child must parse as one JSON-RPC message: the
//! stdout channel is protocol-only, and a stray `println!` anywhere in the
//! stack fails the test.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use impresario_clients::testing::MockAgentBuilder;
use impresario_core::command::WireErrorCode;
use serde_json::{json, Value};

use common::{write_workspace, TestAgent, PLAY_BASIC};

const TIMEOUT: Duration = Duration::from_secs(120);

/// A spawned MCP server plus a line reader; raw (non-JSON) stdout lines are
/// kept so the test can prove the channel stayed clean.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<Value>,
    raw: Arc<Mutex<Vec<String>>>,
    strays: Vec<Value>,
}

impl Mcp {
    fn spawn(workspace: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_impresario"))
            .arg("--workspace")
            .arg(workspace)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn impresario mcp");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (tx, responses) = mpsc::channel();
        let raw = Arc::new(Mutex::new(Vec::new()));
        let raw_reader = Arc::clone(&raw);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Value>(&line) {
                    Ok(message) => {
                        if tx.send(message).is_err() {
                            break;
                        }
                    }
                    Err(_) => raw_reader.lock().expect("raw lock").push(line),
                }
            }
        });
        Self {
            child,
            stdin,
            responses,
            raw,
            strays: Vec::new(),
        }
    }

    fn send(&mut self, message: Value) {
        self.send_raw(&serde_json::to_string(&message).expect("serialize message"));
    }

    fn send_raw(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).expect("write line");
        self.stdin.write_all(b"\n").expect("write newline");
        self.stdin.flush().expect("flush");
    }

    fn notify(&mut self, method: &str) {
        self.send(json!({ "jsonrpc": "2.0", "method": method }));
    }

    fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        self.await_response(id)
    }

    fn tools_call(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }

    /// Like [`Self::tools_call`], but attaches `_meta` (modern requests).
    fn tools_call_meta(&mut self, id: i64, name: &str, arguments: Value, meta: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments, "_meta": meta }),
        )
    }

    fn await_response(&mut self, id: i64) -> Value {
        loop {
            if let Some(index) = self
                .strays
                .iter()
                .position(|value| value.get("id").and_then(Value::as_i64) == Some(id))
            {
                return self.strays.remove(index);
            }
            let value = self
                .responses
                .recv_timeout(TIMEOUT)
                .expect("a JSON-RPC response");
            if value.get("id").and_then(Value::as_i64) == Some(id) {
                return value;
            }
            self.strays.push(value);
        }
    }

    fn recv_next(&mut self) -> Value {
        self.responses
            .recv_timeout(TIMEOUT)
            .expect("a JSON-RPC response")
    }

    /// The protocol channel carried nothing but JSON-RPC.
    fn assert_clean(&self) {
        let raw = self.raw.lock().expect("raw lock");
        assert!(raw.is_empty(), "non-JSON lines on stdout: {raw:?}");
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn result_of(response: &Value) -> &Value {
    assert!(
        response.get("error").is_none(),
        "unexpected JSON-RPC error: {response}"
    );
    response
        .get("result")
        .unwrap_or_else(|| panic!("missing result: {response}"))
}

fn structured(response: &Value) -> Value {
    result_of(response)
        .get("structuredContent")
        .unwrap_or_else(|| panic!("missing structuredContent: {response}"))
        .clone()
}

fn content_of(response: &Value) -> Vec<Value> {
    result_of(response)
        .get("content")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("missing content: {response}"))
        .clone()
}

#[test]
fn legacy_handshake_runs_and_reads_artifacts() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("d1".to_string(), agent.address.clone())],
        &[("basic.play", PLAY_BASIC)],
    );
    let mut mcp = Mcp::spawn(temp.path());

    // Handshake era: `initialize` negotiates the client's revision.
    let initialize = mcp.request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "impresario-tests", "version": "0" },
        }),
    );
    let result = result_of(&initialize);
    assert_eq!(
        result.get("protocolVersion").and_then(Value::as_str),
        Some("2025-06-18")
    );
    assert_eq!(
        result
            .get("serverInfo")
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str),
        Some("impresario")
    );
    assert!(result
        .get("capabilities")
        .and_then(|caps| caps.get("tools"))
        .is_some());
    assert!(result.get("instructions").and_then(Value::as_str).is_some());
    mcp.notify("notifications/initialized");

    // Legacy ping exists; legacy results carry no `resultType`.
    let ping = mcp.request(2, "ping", json!({}));
    assert_eq!(result_of(&ping), &json!({}));
    assert!(result.get("resultType").is_none());

    // The tool surface is complete.
    let tools = result_of(&mcp.request(3, "tools/list", json!({}))).clone();
    let names: Vec<&str> = tools
        .get("tools")
        .and_then(Value::as_array)
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    for expected in [
        "impresario_workspace_info",
        "impresario_validate",
        "impresario_devices_list",
        "impresario_devices_check",
        "impresario_run",
        "impresario_report_get",
        "impresario_artifacts_list",
        "impresario_artifact_get",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }

    // workspace_info reports the device, its support matrix and the files.
    let info = structured(&mcp.tools_call(4, "impresario_workspace_info", json!({})));
    assert_eq!(info["devices"][0]["name"], json!("d1"));
    assert_eq!(info["devices"][0]["platform"], json!("web"));
    assert_eq!(
        info["devices"][0]["support"]["actions"]["click"],
        json!("✅")
    );
    assert_eq!(info["scenes"][0]["name"], json!("TestScreen"));
    assert_eq!(info["plays"][0]["name"], json!("Basic"));
    assert_eq!(info["plays"][0]["acts"][0]["name"], json!("Pass"));

    // validate
    let validate = structured(&mcp.tools_call(5, "impresario_validate", json!({})));
    assert_eq!(validate["ok"], json!(true));
    assert_eq!(validate["errors"], json!(0));

    // devices_check reaches the mock through the real transport stack.
    let check =
        structured(&mcp.tools_call(6, "impresario_devices_check", json!({ "devices": ["d1"] })));
    assert_eq!(check["ok"], json!(true));
    assert_eq!(
        check["devices"][0]["device_info"]["model"],
        json!("Mock Device")
    );

    // run — the play goes by the bare name `workspace_info` reports.
    let run = structured(&mcp.tools_call(
        7,
        "impresario_run",
        json!({ "devices": ["d1"], "plays": ["basic.play"] }),
    ));
    assert_eq!(run["exit_code"], json!(0));
    assert_eq!(run["status"], json!("passed"));
    assert_eq!(run["summary"]["acts_passed"], json!(1));
    let run_dir = PathBuf::from(run["run_dir"].as_str().expect("run_dir string"));
    assert!(run_dir.join("report.json").is_file());
    assert!(run_dir.join("junit.xml").is_file());

    // report_get returns the full Appendix-B document.
    let report = structured(&mcp.tools_call(8, "impresario_report_get", json!({})));
    assert_eq!(report["report"]["status"], json!("passed"));
    assert_eq!(report["report"]["run_id"], run["run_id"]);

    // artifacts_list / artifact_get (text)
    let listed = structured(&mcp.tools_call(9, "impresario_artifacts_list", json!({})));
    let paths: Vec<&str> = listed["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .filter_map(|artifact| artifact["path"].as_str())
        .collect();
    assert!(paths.contains(&"report.json"), "paths: {paths:?}");
    let artifact = mcp.tools_call(
        10,
        "impresario_artifact_get",
        json!({ "path": "report.json" }),
    );
    let content = content_of(&artifact);
    assert_eq!(content[0]["type"], json!("text"));
    let parsed: Value =
        serde_json::from_str(content[0]["text"].as_str().expect("text")).expect("report parses");
    assert_eq!(parsed["run_id"], run["run_id"]);

    // Escaping the run directory is a tool error, not a protocol error.
    let escape = mcp.tools_call(
        11,
        "impresario_artifact_get",
        json!({ "path": "../impresario.config" }),
    );
    assert_eq!(result_of(&escape)["isError"], json!(true));

    // An unknown tool is invalid params at the protocol level.
    let unknown = mcp.tools_call(12, "impresario_nope", json!({}));
    assert_eq!(unknown["error"]["code"], json!(-32602));

    mcp.assert_clean();
}

#[test]
fn modern_discover_protocol_errors_and_diagnostics() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("d1".to_string(), "127.0.0.1:1".to_string())],
        &[],
    );
    let mut mcp = Mcp::spawn(temp.path());
    let meta = json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" } });

    // 2026-07-28 clients discover the server without a handshake.
    let discover = mcp.request(1, "server/discover", meta.clone());
    let result = result_of(&discover);
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["supportedVersions"], json!(["2026-07-28"]));
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        json!("impresario")
    );

    // Modern requests are served statelessly and results are tagged.
    let info = result_of(&mcp.tools_call_meta(
        2,
        "impresario_workspace_info",
        json!({}),
        meta["_meta"].clone(),
    ))
    .clone();
    assert_eq!(info["resultType"], json!("complete"));
    assert!(info.get("structuredContent").is_some());

    // `ping` and `initialize` were removed in 2026-07-28.
    let ping = mcp.request(3, "ping", meta.clone());
    assert_eq!(ping["error"]["code"], json!(-32601));
    let initialize = mcp.request(4, "initialize", meta.clone());
    assert_eq!(initialize["error"]["code"], json!(-32601));

    // A declared version the server does not speak -> -32022.
    let bad_version = mcp.request(
        5,
        "tools/list",
        json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "1900-01-01" } }),
    );
    assert_eq!(bad_version["error"]["code"], json!(-32022));
    assert_eq!(
        bad_version["error"]["data"]["supported"],
        json!(["2026-07-28"])
    );

    // Unknown method.
    let unknown = mcp.request(6, "tools/nope", json!({}));
    assert_eq!(unknown["error"]["code"], json!(-32601));

    // Malformed line -> parse error with a null id.
    mcp.send_raw("{not json");
    let parse = mcp.recv_next();
    assert_eq!(parse["error"]["code"], json!(-32700));
    assert_eq!(parse["id"], json!(null));

    // Broken plays come back as diagnostics with stable codes.
    std::fs::write(
        temp.path().join("plays/broken.play"),
        "Play: Broken\n\nAct: Broken\n\nStage: S\n    Scene: Ghost\n        Check: A\n",
    )
    .expect("write broken play");
    let broken = structured(&mcp.tools_call_meta(
        7,
        "impresario_validate",
        json!({ "paths": ["plays/broken.play"] }),
        meta["_meta"].clone(),
    ));
    assert_eq!(broken["ok"], json!(false));
    assert!(broken["errors"].as_u64().unwrap_or(0) >= 1);
    let codes: Vec<&str> = broken["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str())
        .collect();
    assert!(
        codes.iter().any(|code| code.starts_with("IMP-DSL")),
        "codes: {codes:?}"
    );

    // A run refusal carries its diagnostics, not just a count: a bare play
    // name that does not exist is reported by name.
    let refused = mcp.tools_call_meta(
        99,
        "impresario_run",
        json!({ "plays": ["nope.play"], "devices": ["d1"] }),
        meta["_meta"].clone(),
    );
    assert_eq!(result_of(&refused)["isError"], json!(true));
    let refusal = content_of(&refused);
    let refusal_text = refusal[0]["text"].as_str().expect("refusal text");
    assert!(
        refusal_text.contains("nope.play") && refusal_text.contains("does not exist"),
        "refusal text: {refusal_text}"
    );

    // Resources: the DSL reference is embedded in the binary.
    let resources = result_of(&mcp.request(8, "resources/list", json!({}))).clone();
    let uris: Vec<&str> = resources["resources"]
        .as_array()
        .expect("resources array")
        .iter()
        .filter_map(|resource| resource["uri"].as_str())
        .collect();
    assert!(uris.contains(&"impresario://docs/dsl"), "uris: {uris:?}");
    assert!(uris.contains(&"impresario://workspace/info"));
    let read = result_of(&mcp.request(
        9,
        "resources/read",
        json!({ "uri": "impresario://docs/dsl" }),
    ))
    .clone();
    let text = read["contents"][0]["text"].as_str().expect("doc text");
    assert!(text.len() > 1000, "dsl doc looks wrong");
    assert!(text.contains("Scene:"), "dsl doc looks wrong");

    let missing = mcp.request(10, "resources/read", json!({ "uri": "impresario://nope" }));
    assert_eq!(missing["error"]["code"], json!(-32602));

    mcp.assert_clean();
}

#[test]
fn failed_run_returns_failure_screenshot_as_image() {
    const PLAY_FAILS: &str =
        "Play: Fails\n\nAct: Boom\n\nStage: S\n    Scene: TestScreen\n        Check: A\n";
    let png = b"\x89PNG\r\n\x1a\nmock-failure".to_vec();
    let agent = TestAgent::start(
        MockAgentBuilder::new()
            .fail_command(1, WireErrorCode::CheckFailed, "scripted check failure")
            .screenshot(png.clone()),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("d1".to_string(), agent.address.clone())],
        &[("fails.play", PLAY_FAILS)],
    );
    let mut mcp = Mcp::spawn(temp.path());

    let run = structured(&mcp.tools_call(1, "impresario_run", json!({ "devices": ["d1"] })));
    assert_eq!(run["exit_code"], json!(1));
    assert_eq!(run["status"], json!("failed"));
    assert_eq!(run["summary"]["acts_failed"], json!(1));
    let failure = &run["devices"][0]["plays"][0]["acts"][0]["failures"][0];
    assert_eq!(failure["message"], json!("scripted check failure"));
    let screenshot = failure["screenshot"]
        .as_str()
        .expect("failure screenshot path")
        .replace('\\', "/");
    assert!(screenshot.starts_with("screenshots/"));
    assert!(screenshot.ends_with(".png"));

    let listed = structured(&mcp.tools_call(2, "impresario_artifacts_list", json!({})));
    let images: Vec<&str> = listed["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .filter(|artifact| artifact["kind"] == json!("image"))
        .filter_map(|artifact| artifact["path"].as_str())
        .collect();
    assert!(
        images.contains(&screenshot.as_str()),
        "images: {images:?}, screenshot: {screenshot}"
    );

    // The screenshot comes back as MCP image content, byte for byte.
    let image = mcp.tools_call(3, "impresario_artifact_get", json!({ "path": screenshot }));
    let content = content_of(&image);
    assert_eq!(content[0]["type"], json!("image"));
    assert_eq!(content[0]["mimeType"], json!("image/png"));
    assert_eq!(
        content[0]["data"],
        json!(base64::engine::general_purpose::STANDARD.encode(&png))
    );

    mcp.assert_clean();
}
