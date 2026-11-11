//! `impresario web` end-to-end: the real binary on an ephemeral loopback port
//! serves the embedded console, reads the workspace through the same library
//! functions as the TUI, and executes curated actions by spawning itself.
//! Hardware-free: the probe reaches a `MockAgent`.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::{write_workspace, TestAgent, PLAY_BASIC};
use impresario_clients::testing::MockAgentBuilder;
use serde_json::{json, Value};

/// Kills the console child when the test ends, even on assertion failure.
struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_console(workspace: &std::path::Path) -> (ServerGuard, u16) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_impresario"))
        .arg("--workspace")
        .arg(workspace)
        .args(["web", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn impresario web");
    let stdout = child.stdout.take().expect("stdout pipe");
    let guard = ServerGuard(child);
    let mut lines = BufReader::new(stdout).lines();
    let line = lines
        .next()
        .transpose()
        .expect("read the listening line")
        .expect("the console printed a line");
    let url = line
        .strip_prefix("impresario web: listening on ")
        .unwrap_or_else(|| panic!("unexpected console line: {line}"));
    let port = url
        .rsplit_once(':')
        .and_then(|(_, rest)| rest.trim_end_matches('/').parse::<u16>().ok())
        .unwrap_or_else(|| panic!("unexpected console URL: {url}"));
    // Keep the pipe alive so the child never blocks on stdout.
    std::thread::spawn(move || for _ in lines {});
    (guard, port)
}

struct HttpResponse {
    status: u16,
    body: String,
}

impl HttpResponse {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("body is not JSON ({error}): {}", self.body))
    }
}

fn request(
    port: u16,
    method: &str,
    path: &str,
    host: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the console");
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("read timeout");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    request.push_str(body);
    stream
        .write_all(request.as_bytes())
        .expect("write the request");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    let (head, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed response: {response}"));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("malformed status line: {head}"));
    HttpResponse {
        status,
        body: body.to_string(),
    }
}

fn get(port: u16, path: &str) -> HttpResponse {
    request(port, "GET", path, &format!("127.0.0.1:{port}"), &[], "")
}

fn post(port: u16, path: &str, body: Value) -> HttpResponse {
    let host = format!("127.0.0.1:{port}");
    let origin = format!("http://{host}");
    request(
        port,
        "POST",
        path,
        &host,
        &[
            ("Content-Type", "application/json"),
            ("Origin", origin.as_str()),
        ],
        &body.to_string(),
    )
}

#[test]
fn console_serves_reads_and_runs_curated_actions() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("Pass.play", PLAY_BASIC)],
    );
    let (_server, port) = start_console(temp.path());

    // Static console, compiled into the binary.
    let index = get(port, "/");
    assert_eq!(index.status, 200);
    assert!(index.body.contains("<title>Impresario console</title>"));
    assert!(index.body.contains("data-theme=\"light\""));
    assert!(index.body.contains("id=\"theme-toggle\""));
    let stylesheet = get(port, "/assets/app.css");
    assert_eq!(stylesheet.status, 200);
    assert!(stylesheet.body.contains(":root[data-theme=\"dark\"]"));
    let script = get(port, "/assets/app.js");
    assert_eq!(script.status, 200);
    assert!(script.body.contains("'use strict'"));
    assert!(script.body.contains("impresario.theme"));
    let favicon = get(port, "/favicon.svg");
    assert_eq!(favicon.status, 200);
    assert!(favicon.body.contains("#0f766e"));

    // Read metadata and embedded documentation.
    let meta = get(port, "/api/meta").json();
    assert_eq!(meta["ok"], json!(true));
    assert_eq!(
        meta["data"]["workspace"],
        json!(temp.path().display().to_string())
    );
    assert!(meta["data"]["actions"]
        .as_array()
        .is_some_and(|actions| !actions.is_empty()));
    let schema = get(port, "/api/schema").json();
    assert!(schema["data"]["actions"]
        .as_array()
        .is_some_and(|actions| !actions.is_empty()));
    let guide = get(port, "/api/guide").json();
    assert!(guide["data"]["steps"]
        .as_array()
        .is_some_and(|steps| !steps.is_empty()));
    let overview = get(port, "/api/overview").json();
    assert_eq!(overview["ok"], json!(true));
    assert_eq!(overview["data"]["counts"]["plays"], json!(1));
    let docs = get(port, "/api/docs/web").json();
    assert_eq!(docs["data"]["slug"], json!("web"));
    assert!(docs["data"]["text"]
        .as_str()
        .is_some_and(|text| text.contains("/api/actions/")));
    // Definition reads expose the winning layer so row actions can target it.
    let devices = get(port, "/api/devices").json();
    assert_eq!(devices["data"]["devices"][0]["name"], json!("web1"));
    assert_eq!(devices["data"]["devices"][0]["source"], json!("workspace"));

    // Mutations spawn the CLI child; the action envelope keeps the exit code.
    let validate = post(port, "/api/actions/validate", json!({}));
    assert_eq!(validate.status, 200);
    let validate = validate.json();
    assert_eq!(validate["ok"], json!(true));
    assert_eq!(validate["data"]["exit_code"], json!(0));
    assert_eq!(validate["data"]["passed"], json!(true));

    let dry_run = post(
        port,
        "/api/actions/run",
        json!({ "dry_run": true, "device": ["web1"], "play": ["Pass.play"] }),
    );
    assert_eq!(dry_run.status, 200);
    let dry_run = dry_run.json();
    assert_eq!(dry_run["ok"], json!(true));
    assert_eq!(dry_run["data"]["exit_code"], json!(0));

    // The probe uses the new `--json` flag; the structured result is attached.
    let probe = post(
        port,
        "/api/actions/probe",
        json!({ "kind": "showing", "device": "web1", "piece": "TestScreen.A" }),
    );
    assert_eq!(probe.status, 200);
    let probe = probe.json();
    assert_eq!(probe["ok"], json!(true));
    assert_eq!(probe["data"]["exit_code"], json!(0));
    assert_eq!(probe["data"]["probe"]["ok"], json!(true));
    assert_eq!(probe["data"]["probe"]["device"], json!("web1"));
    assert_eq!(probe["data"]["probe"]["platform"], json!("web"));
    assert_eq!(probe["data"]["probe"]["selector"]["value"], json!("#a"));

    // The history endpoint reports what this console executed.
    let history = get(port, "/api/actions/history").json();
    let names: Vec<&str> = history["data"]["actions"]
        .as_array()
        .expect("history array")
        .iter()
        .filter_map(|action| action["action"].as_str())
        .collect();
    assert!(names.contains(&"validate"));
    assert!(names.contains(&"probe"));
}

#[test]
fn console_rejects_foreign_hosts_origins_and_option_smuggling() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(temp.path(), "", &[], &[("Pass.play", PLAY_BASIC)]);
    let (_server, port) = start_console(temp.path());

    let foreign = request(port, "GET", "/api/meta", "evil.example", &[], "");
    assert_eq!(foreign.status, 403);
    assert_eq!(foreign.json()["error"]["code"], json!("host_not_allowed"));

    let cross = request(
        port,
        "POST",
        "/api/actions/validate",
        &format!("127.0.0.1:{port}"),
        &[
            ("Content-Type", "application/json"),
            ("Origin", "https://evil.example"),
        ],
        "{}",
    );
    assert_eq!(cross.status, 403);
    assert_eq!(cross.json()["error"]["code"], json!("origin_not_allowed"));

    let no_type = request(
        port,
        "POST",
        "/api/actions/validate",
        &format!("127.0.0.1:{port}"),
        &[],
        "{}",
    );
    assert_eq!(no_type.status, 415);
    assert_eq!(
        no_type.json()["error"]["code"],
        json!("unsupported_media_type")
    );

    let unknown = post(port, "/api/actions/definitely_not_an_action", json!({}));
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.json()["error"]["code"], json!("unknown_action"));

    let smuggled = post(
        port,
        "/api/actions/run",
        json!({ "dry_run": true, "device": ["-evil"] }),
    );
    assert_eq!(smuggled.status, 400);
    assert_eq!(smuggled.json()["error"]["code"], json!("invalid_parameter"));
}
