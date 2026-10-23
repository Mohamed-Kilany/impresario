//! An in-process LSP client for the embedded language server (spec 13).
//!
//! The server runs on a duplex pipe inside the test's tokio runtime and the
//! harness speaks real JSON-RPC 2.0 framing (`Content-Length` headers), so
//! the whole transport is exercised.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::{oneshot, Notify};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::{LspService, Server};

use impresario_master::lsp::{Backend, Ctx};

/// Generous so slow machines do not flake; a hung server still fails fast
/// enough for a test run.
const TIMEOUT: Duration = Duration::from_secs(20);

/// A scene both test plays and test scenes can share.
pub const SCENE: &str = r##"Scene: LoginScreen
Platform: cross-platform

Pieces:
    LoginButton: android="id:login_button", web="#login"
    UsernameInput: android="id:username", ios="accessibilityId:username", web="#username"
"##;

/// One act, one stage, two steps.
pub const PLAY: &str = r#"Play: Login
Description: sign in

Act: Successful Login
Tags: smoke

Stage: Navigate
    Scene: LoginScreen
        Action: click => LoginButton
        Check: UsernameInput
"#;

struct Shared {
    seq: AtomicU64,
    publishes: Mutex<HashMap<String, (u64, Value)>>,
    notify: Notify,
}

/// One language-server instance plus the client end of its stdio pipe.
pub struct TestLsp {
    write: WriteHalf<DuplexStream>,
    next_id: i64,
    responses: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    shared: Arc<Shared>,
    server: JoinHandle<()>,
    reader: JoinHandle<()>,
}

impl Drop for TestLsp {
    fn drop(&mut self) {
        self.server.abort();
        self.reader.abort();
    }
}

impl TestLsp {
    /// Start a server for `base` (the workspace root).
    pub async fn start(base: &Path) -> Self {
        super::test_home();
        let ctx = Ctx {
            base: base.to_path_buf(),
            config_path: base.join("impresario.config"),
            devices_path: base.join("impresario.devices.config"),
        };
        let (service, socket) = LspService::new(|client| Backend::new(client, ctx.clone()));
        let (client_side, server_side) = tokio::io::duplex(1 << 16);
        let (server_read, server_write) = tokio::io::split(server_side);
        let server = tokio::spawn(async move {
            Server::new(server_read, server_write, socket)
                .serve(service)
                .await;
        });
        let (client_read, write) = tokio::io::split(client_side);
        let responses: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>> = Arc::default();
        let shared = Arc::new(Shared {
            seq: AtomicU64::new(0),
            publishes: Mutex::new(HashMap::new()),
            notify: Notify::new(),
        });
        let reader = tokio::spawn(read_loop(
            client_read,
            Arc::clone(&responses),
            Arc::clone(&shared),
        ));
        Self {
            write,
            next_id: 1,
            responses,
            shared,
            server,
            reader,
        }
    }

    /// The file URI the server uses for `path`.
    pub fn uri(path: &Path) -> String {
        Uri::from_file_path(path)
            .expect("tests use absolute paths")
            .as_str()
            .to_string()
    }

    /// The current publish counter: pass it to [`Self::wait_diagnostics`] to
    /// wait for a publish that happens *after* this point.
    pub fn mark(&self) -> u64 {
        self.shared.seq.load(Ordering::SeqCst)
    }

    async fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).expect("serialize");
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.write
            .write_all(header.as_bytes())
            .await
            .expect("write header");
        self.write.write_all(&body).await.expect("write body");
        self.write.flush().await.expect("flush");
    }

    /// Send a request and return its `result`.
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let (sender, receiver) = oneshot::channel();
        self.responses.lock().expect("responses").insert(id, sender);
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await;
        let response = timeout(TIMEOUT, receiver)
            .await
            .unwrap_or_else(|_| panic!("no response to {method}"))
            .expect("response channel");
        if let Some(error) = response.get("error") {
            panic!("{method} failed: {error}");
        }
        response.get("result").cloned().unwrap_or(Value::Null)
    }

    /// Send a notification.
    pub async fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await;
    }

    /// `initialize`; `encodings` becomes the client's offered
    /// `general.positionEncodings`.
    pub async fn initialize(&mut self, encodings: Option<&[&str]>) -> Value {
        let mut params = json!({"processId": null, "capabilities": {}});
        if let Some(encodings) = encodings {
            params["capabilities"]["general"] = json!({"positionEncodings": encodings});
        }
        self.request("initialize", params).await
    }

    /// `initialize` followed by `initialized`.
    pub async fn start_session(&mut self, encodings: Option<&[&str]>) -> Value {
        let result = self.initialize(encodings).await;
        self.notify("initialized", json!({})).await;
        result
    }

    pub async fn open(&mut self, path: &Path, text: &str, version: i64) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": Self::uri(path),
                "languageId": "impresario",
                "version": version,
                "text": text,
            }}),
        )
        .await;
    }

    pub async fn change(&mut self, path: &Path, text: &str, version: i64) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": Self::uri(path), "version": version},
                "contentChanges": [{"text": text}],
            }),
        )
        .await;
    }

    pub async fn save(&mut self, path: &Path) {
        self.notify(
            "textDocument/didSave",
            json!({"textDocument": {"uri": Self::uri(path)}}),
        )
        .await;
    }

    pub async fn close(&mut self, path: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": Self::uri(path)}}),
        )
        .await;
    }

    pub async fn did_change_configuration(&mut self, settings: Value) {
        self.notify(
            "workspace/didChangeConfiguration",
            json!({"settings": settings}),
        )
        .await;
    }

    /// The most recent `publishDiagnostics` params for `path`, if any.
    pub fn latest_diagnostics(&self, path: &Path) -> Option<Value> {
        let uri = Self::uri(path);
        self.shared
            .publishes
            .lock()
            .expect("publishes")
            .get(&uri)
            .map(|(_, params)| params.clone())
    }

    /// Wait for a diagnostics publish of `path` that happens after `after`
    /// (a [`Self::mark`] taken before the triggering call).
    pub async fn wait_diagnostics(&self, path: &Path, after: u64) -> Value {
        let uri = Self::uri(path);
        self.wait_until(&uri, after, |_| true).await
    }

    /// Like [`Self::wait_diagnostics`], but for the publish that carries
    /// `version` (used to skip coalesced intermediate publishes).
    pub async fn wait_version(&self, path: &Path, version: i64, after: u64) -> Value {
        let uri = Self::uri(path);
        self.wait_until(&uri, after, |params| {
            params.get("version").and_then(Value::as_i64) == Some(version)
        })
        .await
    }

    async fn wait_until(&self, uri: &str, after: u64, accept: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            // `notified()` does not register until first poll, and
            // `notify_waiters()` stores no permit. Enable it before looking,
            // otherwise a publish between the check and the await is lost and
            // the wait hangs until the (long) timeout - observed under a
            // parallel test run.
            let notified = self.shared.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let publishes = self.shared.publishes.lock().expect("publishes");
                if let Some((seq, params)) = publishes.get(uri) {
                    if *seq > after && accept(params) {
                        return params.clone();
                    }
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for diagnostics of {uri}"
            );
            let _ = timeout(remaining, &mut notified).await;
        }
    }

    /// A request at a position, e.g. `hover` or `completion`.
    pub async fn at(&mut self, method: &str, path: &Path, line: u64, character: u64) -> Value {
        self.request(
            method,
            json!({
                "textDocument": {"uri": Self::uri(path)},
                "position": {"line": line, "character": character},
            }),
        )
        .await
    }
}

async fn read_loop(
    mut read: ReadHalf<DuplexStream>,
    responses: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    shared: Arc<Shared>,
) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let count = match read.read(&mut chunk).await {
            Ok(count) => count,
            Err(_) => return,
        };
        if count == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..count]);
        while let Some(body) = take_message(&mut buffer) {
            let Ok(message) = serde_json::from_slice::<Value>(&body) else {
                continue;
            };
            handle_message(message, &responses, &shared);
        }
    }
}

/// Split one framed message off the front of `buffer`.
fn take_message(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let header_end = buffer.windows(4).position(|window| window == b"\r\n\r\n")? + 4;
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let mut length: Option<usize> = None;
    for line in headers.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-length:") {
            length = line
                .split_once(':')
                .and_then(|(_, value)| value.trim().parse().ok());
        }
    }
    let length = length?;
    if buffer.len() < header_end + length {
        return None;
    }
    let body = buffer[header_end..header_end + length].to_vec();
    buffer.drain(..header_end + length);
    Some(body)
}

fn handle_message(
    message: Value,
    responses: &Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    shared: &Shared,
) {
    let id = message.get("id").and_then(Value::as_i64);
    let method = message.get("method").and_then(Value::as_str);
    match (id, method) {
        // Server -> client requests are not implemented; they are dropped.
        (Some(_), Some(_)) => {}
        (Some(id), None) => {
            if let Some(sender) = responses.lock().expect("responses").remove(&id) {
                let _ = sender.send(message);
            }
        }
        (None, Some("textDocument/publishDiagnostics")) => {
            if let Some(params) = message.get("params") {
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let seq = shared.seq.fetch_add(1, Ordering::SeqCst) + 1;
                shared
                    .publishes
                    .lock()
                    .expect("publishes")
                    .insert(uri, (seq, params.clone()));
                shared.notify.notify_waiters();
            }
        }
        _ => {}
    }
}

/// Write a file, creating parent directories.
pub fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, text).expect("write file");
}

/// A minimal valid `impresario.config`.
pub fn config_text() -> &'static str {
    "project: lsp_tests\n"
}

/// A devices config with the given `(name, platform)` pairs.
pub fn devices_text(devices: &[(&str, &str)]) -> String {
    let mut yaml = String::from("devices:\n");
    for (name, platform) in devices {
        yaml.push_str(&format!("  - name: {name}\n    platform: {platform}\n"));
        match *platform {
            "web" => yaml.push_str("    connection:\n      transport: launch\n"),
            "webos" | "tizen" => yaml.push_str(
                "    connection:\n      host: 127.0.0.1\n      endpoint: ws://127.0.0.1:9998\n",
            ),
            "android" | "androidtv" => {
                yaml.push_str("    connection:\n      serial: emulator-5554\n");
            }
            "ios" => yaml.push_str(
                "    connection:\n      udid: 00008110-001A2345021E001E\n",
            ),
            "tvos" => yaml.push_str(
                "    connection:\n      udid: 00008110-001A2345021E001E\n      endpoint: 127.0.0.1:1\n",
            ),
            _ => {}
        }
    }
    yaml
}

/// A full workspace: config, devices, one scene, one play. Returns the scene
/// and play paths.
pub fn standard_workspace(base: &Path, devices: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    write(&base.join("impresario.config"), config_text());
    write(
        &base.join("impresario.devices.config"),
        &devices_text(devices),
    );
    let scene = base.join("scenes/LoginScreen.scene");
    let play = base.join("plays/login.play");
    write(&scene, SCENE);
    write(&play, PLAY);
    (scene, play)
}

/// Diagnostic codes of a `publishDiagnostics` params object.
pub fn codes(params: &Value) -> Vec<String> {
    diagnostics(params)
        .iter()
        .filter_map(|diagnostic| {
            diagnostic
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// The diagnostics array of a `publishDiagnostics` params object.
pub fn diagnostics(params: &Value) -> Vec<Value> {
    params
        .get("diagnostics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// `(line, character)` of the start of a diagnostic's range.
pub fn range_start(diagnostic: &Value) -> (u64, u64) {
    let start = diagnostic
        .get("range")
        .and_then(|range| range.get("start"))
        .expect("range start");
    (
        start.get("line").and_then(Value::as_u64).expect("line"),
        start
            .get("character")
            .and_then(Value::as_u64)
            .expect("character"),
    )
}

/// `(line, character)` of the end of a diagnostic's range.
pub fn range_end(diagnostic: &Value) -> (u64, u64) {
    let end = diagnostic
        .get("range")
        .and_then(|range| range.get("end"))
        .expect("range end");
    (
        end.get("line").and_then(Value::as_u64).expect("line"),
        end.get("character")
            .and_then(Value::as_u64)
            .expect("character"),
    )
}

/// Completion labels of a completion response.
pub fn completion_labels(result: &Value) -> Vec<String> {
    result
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("label")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}
