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
        "impresario_tags_list",
        "impresario_validate",
        "impresario_devices_list",
        "impresario_devices_check",
        "impresario_probe_inspect",
        "impresario_run",
        "impresario_report_get",
        "impresario_artifacts_list",
        "impresario_artifact_get",
        "impresario_jobs_list",
        "impresario_jobs_get",
        "impresario_service_status",
        "impresario_schedules_list",
        "impresario_schedule_add",
        "impresario_schedule_remove",
        "impresario_schedule_run_now",
        "impresario_watchers_list",
        "impresario_watcher_start",
        "impresario_watcher_stop",
        "impresario_watcher_dumps",
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

    // probe_inspect executes one live probe through the same executor.
    let probe = structured(&mcp.tools_call(
        60,
        "impresario_probe_inspect",
        json!({
            "kind": "showing",
            "device": "d1",
            "piece": "TestScreen.A",
        }),
    ));
    assert_eq!(probe["kind"], json!("showing"));
    assert_eq!(probe["verdict"], json!("pass"));
    assert_eq!(probe["exit_code"], json!(0));
    assert_eq!(probe["verdicts"][0]["claim"], json!("showing=present"));
    let probe_dir = PathBuf::from(probe["run_dir"].as_str().expect("probe run_dir string"));
    assert!(probe_dir.join("report.json").is_file());
    assert!(probe_dir.join("evidence.json").is_file());

    let probe_artifacts = structured(&mcp.tools_call(
        61,
        "impresario_artifacts_list",
        json!({ "run_id": probe["run_id"] }),
    ));
    let evidence = probe_artifacts["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .find(|artifact| artifact["path"] == json!("evidence.json"))
        .expect("evidence artifact");
    assert_eq!(evidence["kind"], json!("evidence"));
    let bundle = mcp.tools_call(
        62,
        "impresario_artifact_get",
        json!({ "run_id": probe["run_id"], "path": "evidence.json" }),
    );
    let bundle_text = content_of(&bundle);
    let bundle: Value = serde_json::from_str(bundle_text[0]["text"].as_str().expect("text"))
        .expect("evidence.json parses");
    assert_eq!(bundle["kind"], json!("showing"));
    assert_eq!(bundle["result"]["verdicts"][0]["verdict"], json!("pass"));

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
        "Play: Broken\n\nAct: Broken\n    Perform: Missing\n",
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
        refusal_text.contains("nope.play") && refusal_text.contains("is not a file"),
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
    assert!(uris.contains(&"impresario://docs/scheduling"));
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

    let scheduling = result_of(&mcp.request(
        10,
        "resources/read",
        json!({ "uri": "impresario://docs/scheduling" }),
    ))
    .clone();
    let scheduling = scheduling["contents"][0]["text"]
        .as_str()
        .expect("scheduling doc text");
    assert!(scheduling.contains("notifications:"));
    assert!(scheduling.contains("impresario_watcher_dumps"));

    let missing = mcp.request(11, "resources/read", json!({ "uri": "impresario://nope" }));
    assert_eq!(missing["error"]["code"], json!(-32602));

    mcp.assert_clean();
}

#[test]
fn resident_tools_join_state_and_edit_config_layers() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "schedules:\n  - name: nightly\n    when: { kind: daily, at: \"02:30\" }\n    run: { plays: [basic.play] }\nwatchers:\n  - name: soak\n    run: { plays: [basic.play] }\n",
        &[("d1".to_string(), agent.address.clone())],
        &[("basic.play", PLAY_BASIC)],
    );
    std::fs::create_dir_all(temp.path().join("reports/watch/soak/20261106-120000"))
        .expect("dump directory");

    let mut mcp = Mcp::spawn(temp.path());

    let status = structured(&mcp.tools_call(1, "impresario_service_status", json!({})));
    assert_eq!(status["running"], json!(false));
    assert_eq!(status["client_version"], json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(status["restart_required"], json!(false));
    assert_eq!(status["heartbeat_stale"], json!(false));
    assert_eq!(status["schedules"][0]["name"], json!("nightly"));
    assert_eq!(status["watchers"][0]["name"], json!("soak"));
    assert_eq!(status["watchers"][0]["lease"], json!("gap"));
    let next = status["schedules"][0]["next_fire"]
        .as_str()
        .expect("next fire");
    assert!(next.contains("02:30"), "next fire: {next}");

    let schedules = structured(&mcp.tools_call(2, "impresario_schedules_list", json!({})));
    assert_eq!(schedules["schedules"].as_array().expect("array").len(), 1);
    let watchers = structured(&mcp.tools_call(3, "impresario_watchers_list", json!({})));
    assert_eq!(watchers["watchers"][0]["name"], json!("soak"));
    assert_eq!(watchers["watchers"][0]["lease"], json!("gap"));

    let added = structured(&mcp.tools_call(
        4,
        "impresario_schedule_add",
        json!({
            "name": "morning",
            "schedule": {
                "when": { "kind": "daily", "at": "09:00" },
                "run": { "plays": ["basic.play"] }
            }
        }),
    ));
    assert_eq!(added["schedule"]["name"], json!("morning"));
    let duplicate = mcp.tools_call(
        5,
        "impresario_schedule_add",
        json!({
            "name": "morning",
            "schedule": { "when": { "kind": "daily", "at": "10:00" } }
        }),
    );
    assert_eq!(result_of(&duplicate)["isError"], json!(true));
    let mismatched = mcp.tools_call(
        6,
        "impresario_schedule_add",
        json!({ "name": "bad", "schedule": { "when": { "kind": "daily", "at": "10:00" }, "name": "other" } }),
    );
    assert_eq!(result_of(&mismatched)["isError"], json!(true));

    let listed = structured(&mcp.tools_call(7, "impresario_schedules_list", json!({})));
    assert_eq!(listed["schedules"].as_array().expect("array").len(), 2);
    let removed = structured(&mcp.tools_call(
        8,
        "impresario_schedule_remove",
        json!({ "name": "morning" }),
    ));
    assert_eq!(removed["removed"], json!("morning"));
    let missing = mcp.tools_call(
        9,
        "impresario_schedule_remove",
        json!({ "name": "morning", "layer": "global" }),
    );
    assert_eq!(result_of(&missing)["isError"], json!(true));

    let stopped =
        structured(&mcp.tools_call(10, "impresario_watcher_stop", json!({ "name": "soak" })));
    assert_eq!(stopped["enabled"], json!(false));
    let started =
        structured(&mcp.tools_call(11, "impresario_watcher_start", json!({ "name": "soak" })));
    assert_eq!(started["enabled"], json!(true));

    let dumps =
        structured(&mcp.tools_call(12, "impresario_watcher_dumps", json!({ "name": "soak" })));
    assert_eq!(dumps["dumps"][0]["name"], json!("20261106-120000"));
    assert_eq!(
        dumps["dumps"][0]["path"],
        json!("reports/watch/soak/20261106-120000")
    );

    let run = structured(&mcp.tools_call(
        13,
        "impresario_schedule_run_now",
        json!({ "name": "nightly" }),
    ));
    assert_eq!(run["exit_code"], json!(0));
    assert_eq!(run["status"], json!("passed"));

    mcp.assert_clean();
}

#[test]
fn tags_jobs_and_probes_resource_are_available() {
    const TAGGED_PLAY: &str = r#"Play: Tagged
Tags: smoke, auth

Act: Pass
Tags: smoke
    Perform: Basic
"#;
    const TAGGED_THEATRE: &str = r#"Theatre: Tagged
Tags: nightly

Stage: Curtain
    Tags: smoke
    Scene: TestScreen
        Check: A
"#;
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(temp.path(), "", &[], &[("tagged.play", TAGGED_PLAY)]);
    std::fs::write(temp.path().join("theatres/tagged.theatre"), TAGGED_THEATRE)
        .expect("write tagged theatre");

    let home = common::test_home();
    let passed_id = "20261109-120000-mcp-tags-passed";
    let running_id = "20261109-120000-mcp-tags-running";
    for (id, status, pid) in [
        (passed_id, "passed", Value::Null),
        (running_id, "running", json!(std::process::id())),
    ] {
        let dir = home.join("jobs").join(id);
        std::fs::create_dir_all(&dir).expect("job dir");
        let finished = status == "passed";
        let record = json!({
            "schema_version": 1,
            "id": id,
            "status": status,
            "detached": true,
            "created_at": "2026-11-09T12:00:00Z",
            "started_at": "2026-11-09T12:00:01Z",
            "finished_at": if finished { json!("2026-11-09T12:00:02Z") } else { Value::Null },
            "duration_ms": if finished { json!(1000) } else { Value::Null },
            "exit_code": if finished { json!(0) } else { Value::Null },
            "workspace": "mcp-tags",
            "devices": ["d1"],
            "plays": ["tagged.play"],
            "label": "1 play(s) x 1 device(s)",
            "pid": pid,
            "queue_position": Value::Null,
            "run_dir": "reports/20261109-120000-mcp-tags",
            "message": Value::Null,
        });
        std::fs::write(
            dir.join("job.json"),
            serde_json::to_string(&record).expect("serialize job"),
        )
        .expect("write job");
    }

    let mut mcp = Mcp::spawn(temp.path());

    // Tags across plays, acts, theatres and stages, with workspace-relative files.
    let tags = structured(&mcp.tools_call(1, "impresario_tags_list", json!({})));
    let by_tag = |name: &str| {
        tags["tags"]
            .as_array()
            .expect("tags array")
            .iter()
            .find(|tag| tag["tag"] == json!(name))
            .unwrap_or_else(|| panic!("missing tag {name}: {tags}"))
            .clone()
    };
    let smoke = by_tag("smoke");
    assert_eq!(smoke["count"], json!(3));
    let files: Vec<&str> = smoke["files"]
        .as_array()
        .expect("files array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(files.contains(&"plays/tagged.play"), "files: {files:?}");
    assert!(
        files.contains(&"theatres/tagged.theatre"),
        "files: {files:?}"
    );
    let usages: Vec<&str> = smoke["usages"]
        .as_array()
        .expect("usages array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(usages.contains(&"play Tagged"), "usages: {usages:?}");
    assert!(usages.contains(&"act Tagged / Pass"), "usages: {usages:?}");
    assert!(
        usages.contains(&"stage Tagged.Curtain"),
        "usages: {usages:?}"
    );
    assert_eq!(by_tag("auth")["count"], json!(1));
    assert_eq!(by_tag("nightly")["count"], json!(1));
    assert_eq!(tags["diagnostics"], json!([]));

    // Active-only is the default; all: true includes the finished job.
    let active = structured(&mcp.tools_call(2, "impresario_jobs_list", json!({})));
    let active_ids: Vec<&str> = active["jobs"]
        .as_array()
        .expect("jobs array")
        .iter()
        .filter_map(|job| job["id"].as_str())
        .collect();
    assert!(
        active_ids.contains(&running_id),
        "active jobs: {active_ids:?}"
    );
    assert!(
        !active_ids.contains(&passed_id),
        "finished job leaked into the active list: {active_ids:?}"
    );

    let all = structured(&mcp.tools_call(
        3,
        "impresario_jobs_list",
        json!({ "all": true, "limit": 1 }),
    ));
    assert_eq!(all["returned"], json!(1));
    assert!(
        all["total"].as_u64().unwrap_or(0) >= 2,
        "total: {}",
        all["total"]
    );

    let job = structured(&mcp.tools_call(4, "impresario_jobs_get", json!({ "id": passed_id })));
    assert_eq!(job["job"]["id"], json!(passed_id));
    assert_eq!(job["job"]["status"], json!("passed"));
    assert_eq!(job["report_copy"], json!(false));

    let missing = mcp.tools_call(
        5,
        "impresario_jobs_get",
        json!({ "id": "20261109-120000-no-such-job" }),
    );
    assert_eq!(result_of(&missing)["isError"], json!(true));
    let text = content_of(&missing)[0]["text"]
        .as_str()
        .expect("error text")
        .to_string();
    assert!(text.contains("unknown job"), "error text: {text}");

    // The probe reference is a first-class resource.
    let resources = result_of(&mcp.request(6, "resources/list", json!({}))).clone();
    let uris: Vec<&str> = resources["resources"]
        .as_array()
        .expect("resources array")
        .iter()
        .filter_map(|resource| resource["uri"].as_str())
        .collect();
    assert!(uris.contains(&"impresario://docs/probes"), "uris: {uris:?}");
    let probes = result_of(&mcp.request(
        7,
        "resources/read",
        json!({ "uri": "impresario://docs/probes" }),
    ))
    .clone();
    let probes = probes["contents"][0]["text"]
        .as_str()
        .expect("probes doc text");
    assert!(probes.contains("unverifiable"));

    mcp.assert_clean();
}

#[test]
fn failed_run_returns_failure_screenshot_as_image() {
    const PLAY_FAILS: &str = "Play: Fails\n\nAct: Boom\n    Perform: Shot\n";
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
