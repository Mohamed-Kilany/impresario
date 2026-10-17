//! Connector integration suite (TASK.md phase 4 accept): the same fixture
//! scenarios against headless Chromium for the `raw` backend (always) and the
//! `chromiumoxide` backend (feature `chromiumoxide-backend`).
//!
//! The suite drives `impresario-proto`'s generated client directly (the
//! connector keeps "no other edges"): an in-process tonic server hosts
//! [`ConnectorService`], the fixtures live in `fixtures/web/*.html`, and the
//! tests skip cleanly when no Chromium is installed (set `CHROME_BIN`).
//!
//! `cargo test -p impresario-connector --features chromiumoxide-backend`

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use impresario_connector::launch::{find_chromium, launch_chromium_at, LaunchedBrowser};
use impresario_connector::png::png_dimensions;
use impresario_connector::server::{ConnectorService, SessionManager};
use impresario_proto::v1 as pb;
use impresario_proto::v1::agent_client::AgentClient;
use impresario_proto::v1::agent_server::AgentServer;
use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::transport::Channel;
use tonic::{Request, Status};

// ---------------------------------------------------------------------------
// small assertion helpers
// ---------------------------------------------------------------------------

fn expect_ok(result: &pb::CommandResult) {
    assert!(
        result.ok,
        "expected ok for '{}', got {:?}",
        result.command_id, result.error
    );
}

fn expect_code(result: &pb::CommandResult, code: pb::ErrorCode) {
    assert!(
        !result.ok,
        "expected failure ({code:?}) for '{}'",
        result.command_id
    );
    let error = result.error.as_ref().expect("error payload");
    assert_eq!(
        error.code, code as i32,
        "unexpected error for '{}': {error:?}",
        result.command_id
    );
}

fn error_of(result: &pb::CommandResult) -> &pb::ErrorInfo {
    result.error.as_ref().expect("error payload")
}

// ---------------------------------------------------------------------------
// proto builders
// ---------------------------------------------------------------------------

fn selector(strategy: pb::SelectorStrategy, value: &str) -> pb::Selector {
    pb::Selector {
        strategy: strategy as i32,
        value: value.to_string(),
        index: 0,
    }
}

fn css(value: &str) -> pb::Selector {
    selector(pb::SelectorStrategy::Css, value)
}

fn xpath(value: &str) -> pb::Selector {
    selector(pb::SelectorStrategy::Xpath, value)
}

fn text(value: &str) -> pb::Selector {
    selector(pb::SelectorStrategy::Text, value)
}

fn action(kind: pb::ActionType, selector: Option<pb::Selector>) -> pb::RunAction {
    pb::RunAction {
        r#type: kind as i32,
        selector,
        text: String::new(),
        direction: 0,
        count: 0,
        duration_ms: 0,
    }
}

fn body(action: pb::RunAction) -> pb::command::Body {
    pb::command::Body::RunAction(action)
}

fn check_body(
    kind: pb::CheckKind,
    selector: pb::Selector,
    expected_text: &str,
    expected_count: u32,
) -> pb::command::Body {
    pb::command::Body::ValidateCheck(pb::ValidateCheck {
        selector: Some(selector),
        kind: kind as i32,
        expected_text: expected_text.to_string(),
        expected_count,
    })
}

fn command(id: &str, body: pb::command::Body, timeout_ms: u32) -> pb::Command {
    pb::Command {
        command_id: id.to_string(),
        timeout_ms,
        body: Some(body),
    }
}

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct Handshake {
    platform: i32,
    endpoint: String,
    app_url: String,
    options: Vec<(String, String)>,
    token: Option<String>,
    protocol: String,
}

fn suite_options(backend: &str, page_match: &str) -> Vec<(String, String)> {
    vec![
        ("cdp_backend".to_string(), backend.to_string()),
        ("page_match".to_string(), page_match.to_string()),
        ("poll_interval_ms".to_string(), "50".to_string()),
    ]
}

struct Harness {
    client: AgentClient<Channel>,
    manager: Arc<SessionManager>,
    token: Option<String>,
}

impl Harness {
    async fn start(token: Option<&str>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server address");
        let service = ConnectorService::new(token.map(str::to_string));
        let manager = service.manager().clone();
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(AgentServer::new(service))
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .expect("test server");
        });
        let client = AgentClient::connect(format!("http://{addr}"))
            .await
            .expect("connect test client");
        Self {
            client,
            manager,
            token: token.map(str::to_string),
        }
    }

    async fn handshake(&mut self, spec: Handshake) -> Result<pb::HandshakeResponse, Status> {
        let mut request = Request::new(pb::HandshakeRequest {
            protocol_version: spec.protocol,
            master_version: "test".to_string(),
            device_name: "fixture".to_string(),
            platform: spec.platform,
            target_endpoint: spec.endpoint,
            app_id: String::new(),
            app_url: spec.app_url,
            options: spec.options.into_iter().collect(),
            auth_token: spec.token.clone().unwrap_or_default(),
        });
        if let Some(token) = spec.token.as_deref() {
            if let Ok(value) = token.parse() {
                request.metadata_mut().insert("x-impresario-token", value);
            }
        }
        self.client
            .handshake(request)
            .await
            .map(|response| response.into_inner())
    }

    fn request_with_session<T>(&self, session_id: &str, message: T) -> Request<T> {
        let mut request = Request::new(message);
        request.metadata_mut().insert(
            "x-impresario-session-id",
            session_id.parse().expect("session id"),
        );
        if let Some(token) = self.token.as_deref() {
            if let Ok(value) = token.parse() {
                request.metadata_mut().insert("x-impresario-token", value);
            }
        }
        request
    }

    async fn shutdown(&mut self, session_id: &str) {
        let request = self.request_with_session(
            session_id,
            pb::ShutdownRequest {
                terminate_app: false,
            },
        );
        self.client.shutdown(request).await.expect("shutdown rpc");
    }

    async fn capture_screenshot(&mut self, session_id: &str) -> pb::ScreenshotResponse {
        let request = self.request_with_session(session_id, pb::ScreenshotRequest {});
        self.client
            .capture_screenshot(request)
            .await
            .expect("capture rpc")
            .into_inner()
    }
}

/// One open `Session` stream plus its command channel.
struct Driver {
    session_id: String,
    events: tonic::Streaming<pb::Event>,
    commands: mpsc::Sender<pb::Command>,
    next: u32,
    /// Results read while waiting for another command (a cancel's own result
    /// can arrive after the cancelled command's result).
    seen: std::collections::HashMap<String, pb::CommandResult>,
}

impl Driver {
    async fn open(client: &mut AgentClient<Channel>, session_id: &str) -> Self {
        let (commands, receiver) = mpsc::channel::<pb::Command>(64);
        let mut request = Request::new(ReceiverStream::new(receiver));
        request.metadata_mut().insert(
            "x-impresario-session-id",
            session_id.parse().expect("session id"),
        );
        let events = client
            .session(request)
            .await
            .expect("session rpc")
            .into_inner();
        Self {
            session_id: session_id.to_string(),
            events,
            commands,
            next: 0,
            seen: std::collections::HashMap::new(),
        }
    }

    /// Send one command, return its id.
    async fn send(&mut self, body: pb::command::Body, timeout_ms: u32) -> String {
        self.next += 1;
        let id = format!("cmd-{}", self.next);
        let command = command(&id, body, timeout_ms);
        self.commands.send(command).await.expect("send command");
        id
    }

    /// Send one command and wait for its result.
    async fn run(&mut self, body: pb::command::Body, timeout_ms: u32) -> pb::CommandResult {
        let id = self.send(body, timeout_ms).await;
        self.await_result(&id).await
    }

    /// Send one command, expect `ok`.
    async fn run_ok(&mut self, body: pb::command::Body, timeout_ms: u32) -> pb::CommandResult {
        let result = self.run(body, timeout_ms).await;
        expect_ok(&result);
        result
    }

    /// Send one command, expect a specific error code.
    async fn run_expect(
        &mut self,
        body: pb::command::Body,
        timeout_ms: u32,
        code: pb::ErrorCode,
    ) -> pb::CommandResult {
        let result = self.run(body, timeout_ms).await;
        expect_code(&result, code);
        result
    }

    async fn await_result(&mut self, id: &str) -> pb::CommandResult {
        if let Some(result) = self.seen.remove(id) {
            return result;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let message = tokio::time::timeout(remaining, self.events.message())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for the result of '{id}'"))
                .expect("session stream");
            let Some(event) = message else {
                panic!("session stream ended while waiting for '{id}'");
            };
            match event.body {
                Some(pb::event::Body::Result(result)) => {
                    if result.command_id == id {
                        return result;
                    }
                    self.seen.insert(result.command_id.clone(), result);
                }
                Some(pb::event::Body::Heartbeat(_)) | None => {}
                Some(pb::event::Body::Closed(closed)) => {
                    panic!("session closed while waiting for '{id}': {closed:?}");
                }
            }
        }
    }

    /// Read events until `SessionClosed`; panics on timeout.
    async fn await_closed(&mut self) -> pb::SessionClosed {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let message = tokio::time::timeout(remaining, self.events.message())
                .await
                .expect("timed out waiting for SessionClosed")
                .expect("session stream");
            match message {
                Some(event) => match event.body {
                    Some(pb::event::Body::Closed(closed)) => return closed,
                    Some(pb::event::Body::Heartbeat(_))
                    | Some(pb::event::Body::Result(_))
                    | None => {}
                },
                None => panic!("session stream ended without SessionClosed"),
            }
        }
    }

    async fn text_is(&mut self, selector: pb::Selector, expected: &str) {
        let result = self
            .run_ok(
                check_body(pb::CheckKind::TextEquals, selector, expected, 0),
                2_000,
            )
            .await;
        assert_eq!(result.observed_text, expected, "observed text");
    }

    async fn text_contains(&mut self, selector: pb::Selector, needle: &str) {
        self.run_ok(
            check_body(pb::CheckKind::TextContainsCheck, selector, needle, 0),
            2_000,
        )
        .await;
    }

    async fn check_visible(&mut self, selector: pb::Selector, timeout_ms: u32) {
        self.run_ok(
            check_body(pb::CheckKind::Visible, selector, "", 0),
            timeout_ms,
        )
        .await;
    }

    async fn check_fails(
        &mut self,
        kind: pb::CheckKind,
        selector: pb::Selector,
        expected_text: &str,
        timeout_ms: u32,
    ) -> pb::CommandResult {
        self.run_expect(
            check_body(kind, selector, expected_text, 0),
            timeout_ms,
            pb::ErrorCode::CheckFailed,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Chromium fixtures
// ---------------------------------------------------------------------------

fn fixture_url(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("web")
        .join(name);
    let resolved = path.canonicalize().expect("fixture file exists");
    let text = resolved.to_string_lossy().replace('\\', "/");
    // `canonicalize` yields Windows verbatim paths (`\\?\D:\...`); Chromium's
    // file-URL parser rejects them and silently opens `chrome://newtab`.
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

/// Launch a test-managed headless Chromium with one fixture page open
/// (headless Chromium rejects several startup URLs).
async fn start_chrome(page: &str) -> Option<LaunchedBrowser> {
    find_chromium()?;
    let args = vec![
        "--disable-gpu".to_string(),
        "--mute-audio".to_string(),
        "--window-size=1100,800".to_string(),
        // I-37: the renderer must keep producing frames (scroll events).
        "--disable-background-timer-throttling".to_string(),
        "--disable-backgrounding-occluded-windows".to_string(),
        "--disable-renderer-backgrounding".to_string(),
    ];
    Some(
        launch_chromium_at(true, &fixture_url(page), &args)
            .await
            .expect("launch chromium"),
    )
}

// ---------------------------------------------------------------------------
// the fixture suite (both backends)
// ---------------------------------------------------------------------------

async fn run_fixture_suite(backend: &str) {
    let Some(mut chrome) = start_chrome("index.html").await else {
        eprintln!("skipping fixture suite ({backend}): no Chromium found (set CHROME_BIN)");
        return;
    };
    let endpoint = format!("ws://{}:{}", chrome.host, chrome.port);
    let mut harness = Harness::start(None).await;

    // --- handshake, capabilities, device info ------------------------------
    let response = harness
        .handshake(Handshake {
            platform: pb::Platform::Web as i32,
            endpoint: endpoint.clone(),
            app_url: String::new(),
            options: suite_options(backend, "index.html"),
            token: None,
            protocol: "1.0.0".to_string(),
        })
        .await
        .expect("handshake rpc");
    assert!(
        response.accepted,
        "[{backend}] handshake rejected: {:?}",
        response.error
    );
    let capabilities = &response.capabilities;
    for expected in [
        pb::Capability::MultiSession,
        pb::Capability::Logs,
        pb::Capability::Screenshot,
        pb::Capability::ProgrammaticFocus,
        pb::Capability::KeyEvents,
        pb::Capability::AppControl,
    ] {
        assert!(
            capabilities.contains(&(expected as i32)),
            "[{backend}] missing capability {expected:?}"
        );
    }
    assert!(
        !capabilities.contains(&(pb::Capability::DpadFocus as i32)),
        "[{backend}] web sessions must not advertise DpadFocus"
    );
    let device = response.device.as_ref().expect("device info");
    assert!(device.screen_width_px > 0, "[{backend}] device: {device:?}");
    let session_id = response.session_id.clone();
    assert_eq!(harness.manager.session_count(), 1);

    let mut web = Driver::open(&mut harness.client, &session_id).await;

    // --- basic actions -----------------------------------------------------
    web.check_visible(css("#fixture-title"), 2_000).await;
    web.run_ok(
        body(action(pb::ActionType::Click, Some(css("#click-button")))),
        2_000,
    )
    .await;
    web.text_is(css("#click-result"), "clicked").await;

    // --- text entry through framework-style listeners ----------------------
    let mut replace = action(pb::ActionType::Type, Some(css("#replace-input")));
    replace.text = "new".to_string();
    let result = web.run_ok(body(replace), 2_000).await;
    assert_eq!(result.observed_text, "new");
    web.text_is(css("#replace-state"), "input=new events=input,change")
        .await;

    let mut append = action(pb::ActionType::Append, Some(css("#append-input")));
    append.text = "x".to_string();
    let result = web.run_ok(body(append), 2_000).await;
    assert_eq!(result.observed_text, "oldx");
    web.text_is(css("#append-state"), "input=oldx events=input,change")
        .await;

    let clear = action(pb::ActionType::Clear, Some(css("#clear-input")));
    let result = web.run_ok(body(clear), 2_000).await;
    assert_eq!(result.observed_text, "");
    web.text_is(css("#clear-state"), "input= events=input,change")
        .await;

    let mut readonly = action(pb::ActionType::Type, Some(css("#readonly-input")));
    readonly.text = "x".to_string();
    web.run_expect(body(readonly), 600, pb::ErrorCode::NotInteractable)
        .await;

    // --- long press --------------------------------------------------------
    let mut hold = action(pb::ActionType::LongPress, Some(css("#long-press-button")));
    hold.duration_ms = 700;
    web.run_ok(body(hold), 2_000).await;
    web.text_is(css("#long-press-result"), "long").await;

    // --- states: hidden, disabled, obscured --------------------------------
    web.run_ok(
        check_body(pb::CheckKind::Hidden, css("#hidden-panel"), "", 0),
        500,
    )
    .await;
    web.check_fails(pb::CheckKind::Visible, css("#hidden-panel"), "", 400)
        .await;
    web.run_ok(
        check_body(pb::CheckKind::Disabled, css("#disabled-button"), "", 0),
        500,
    )
    .await;
    web.check_fails(pb::CheckKind::Enabled, css("#disabled-button"), "", 400)
        .await;

    let result = web
        .run_expect(
            body(action(pb::ActionType::Click, Some(css("#disabled-button")))),
            400,
            pb::ErrorCode::NotInteractable,
        )
        .await;
    assert_eq!(
        error_of(&result).details.get("reason").map(String::as_str),
        Some("disabled")
    );

    let result = web
        .run_expect(
            body(action(pb::ActionType::Click, Some(css("#obscured-button")))),
            400,
            pb::ErrorCode::NotInteractable,
        )
        .await;
    let error = error_of(&result);
    assert_eq!(
        error.details.get("reason").map(String::as_str),
        Some("obscured")
    );
    assert_eq!(
        error.details.get("obscured_by").map(String::as_str),
        Some("div#obscurer")
    );

    // --- delayed element (auto-wait) ---------------------------------------
    web.check_visible(css("#delayed-box"), 3_000).await;
    web.text_is(css("#delayed-box"), "delayed-ready").await;

    // --- select ------------------------------------------------------------
    let mut pick = action(pb::ActionType::Select, Some(css("#fruit")));
    pick.text = "Banana".to_string();
    let result = web.run_ok(body(pick), 2_000).await;
    assert_eq!(result.observed_text, "banana");
    web.text_is(css("#fruit-state"), "value=banana").await;

    // --- selector strategies and counts ------------------------------------
    web.check_visible(text("fixture-marker-text"), 2_000).await;
    web.check_visible(xpath("//*[@id=\"click-button\"]"), 2_000)
        .await;
    let result = web
        .run_ok(
            check_body(pb::CheckKind::Count, css(".bullet"), "", 3),
            2_000,
        )
        .await;
    assert_eq!(result.observed_count, 3);
    let result = web
        .run_expect(
            check_body(pb::CheckKind::Count, css(".bullet"), "", 2),
            2_000,
            pb::ErrorCode::CheckFailed,
        )
        .await;
    let error = error_of(&result);
    assert_eq!(error.details.get("expected").map(String::as_str), Some("2"));
    assert_eq!(error.details.get("observed").map(String::as_str), Some("3"));
    assert_eq!(
        error.details.get("matched_count").map(String::as_str),
        Some("3")
    );

    // --- focus -------------------------------------------------------------
    web.run_ok(
        body(action(pb::ActionType::Focus, Some(css("#grid-5")))),
        2_000,
    )
    .await;
    web.run_ok(
        check_body(pb::CheckKind::Focused, css("#grid-5"), "", 0),
        500,
    )
    .await;
    web.text_is(css("#focus-state"), "grid-5").await;

    // --- key press (CDP key events on the focused field) -------------------
    let mut keys = action(pb::ActionType::Type, Some(css("#keys-input")));
    keys.text = "xy".to_string();
    web.run_ok(body(keys), 2_000).await;
    web.run_ok(
        pb::command::Body::KeyPress(pb::KeyPress {
            key: pb::RemoteKey::KeyBackspace as i32,
            repeat: 1,
            interval_ms: 0,
        }),
        2_000,
    )
    .await;
    web.text_is(css("#keys-input"), "x").await;

    // --- history back ------------------------------------------------------
    web.run_ok(
        body(action(pb::ActionType::Click, Some(css("#anchor-link")))),
        2_000,
    )
    .await;
    web.text_is(css("#hash-state"), "#two").await;
    web.run_ok(body(action(pb::ActionType::Back, None)), 2_000)
        .await;
    web.text_is(css("#hash-state"), "").await;

    // --- scrolling ---------------------------------------------------------
    let mut scroll = action(pb::ActionType::Scroll, None);
    scroll.direction = pb::Direction::Down as i32;
    scroll.count = 2;
    web.run_ok(body(scroll), 2_000).await;
    web.text_is(css("#window-scroll-state"), "scrolled").await;

    web.run_ok(
        body(action(
            pb::ActionType::ScrollTo,
            Some(css("#scroll-target")),
        )),
        5_000,
    )
    .await;
    web.check_visible(css("#scroll-target"), 500).await;

    // --- shadow contenteditable -------------------------------------------
    let mut shadow = action(pb::ActionType::Type, Some(css("#shadow-input")));
    shadow.text = "fresh".to_string();
    web.run_ok(body(shadow), 2_000).await;
    web.text_is(css("#shadow-input"), "fresh").await;
    web.run_ok(
        check_body(pb::CheckKind::Focused, css("#shadow-input"), "", 0),
        500,
    )
    .await;
    web.text_contains(css("#shadow-state"), "fresh").await;

    // --- app control (reload) ----------------------------------------------
    web.run_ok(
        pb::command::Body::AppControl(pb::AppControl {
            op: pb::AppOp::Restart as i32,
            clear_data: false,
        }),
        5_000,
    )
    .await;
    web.text_is(css("#click-result"), "-").await;
    web.text_is(css("#replace-state"), "-").await;

    // --- screenshots (in-band + out-of-band) -------------------------------
    let shot = web
        .run_ok(
            pb::command::Body::Screenshot(pb::Screenshot {
                label: "fixture".to_string(),
            }),
            5_000,
        )
        .await;
    assert!(!shot.screenshot_png.is_empty(), "in-band screenshot bytes");
    let (width, height) = png_dimensions(&shot.screenshot_png).expect("PNG header");
    assert!(width > 0 && height > 0, "PNG dimensions {width}x{height}");

    let out_of_band = harness.capture_screenshot(&session_id).await;
    assert!(
        out_of_band.error.is_none(),
        "capture: {:?}",
        out_of_band.error
    );
    assert!(!out_of_band.png.is_empty(), "out-of-band screenshot bytes");
    assert!(out_of_band.width > 0 && out_of_band.height > 0);

    // --- console logs ------------------------------------------------------
    let mut log_client = harness.client.clone();
    let mut logs = log_client
        .stream_logs(harness.request_with_session(
            &session_id,
            pb::LogRequest {
                min_level: pb::LogLevel::Debug as i32,
            },
        ))
        .await
        .expect("stream logs rpc")
        .into_inner();
    web.run_ok(
        body(action(pb::ActionType::Click, Some(css("#log-button")))),
        2_000,
    )
    .await;
    let entry = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(message) = logs.message().await.expect("log stream") {
            if message.message.contains("fixture-log-message") {
                return message;
            }
        }
        panic!("log stream ended before the fixture message");
    })
    .await
    .expect("fixture log entry");
    assert_eq!(
        entry.level,
        pb::LogLevel::Warn as i32,
        "log entry: {entry:?}"
    );
    assert_eq!(entry.source, "console", "log entry: {entry:?}");
    drop(logs);

    // --- cancellation ------------------------------------------------------
    let pending = web
        .send(
            body(action(pb::ActionType::Click, Some(css("#never-appears")))),
            30_000,
        )
        .await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let cancel = web
        .run_ok(
            pb::command::Body::Cancel(pb::Cancel {
                target_command_id: pending.clone(),
            }),
            0,
        )
        .await;
    assert!(cancel.ok);
    let cancelled = web.await_result(&pending).await;
    expect_code(&cancelled, pb::ErrorCode::Cancelled);

    // cancelling an unknown id is a PROTOCOL error on the cancel itself
    web.run_expect(
        pb::command::Body::Cancel(pb::Cancel {
            target_command_id: "no-such-command".to_string(),
        }),
        0,
        pb::ErrorCode::Protocol,
    )
    .await;

    // cancelling a finished command is PROTOCOL too
    let finished_id = web
        .send(
            body(action(pb::ActionType::Click, Some(css("#click-button")))),
            2_000,
        )
        .await;
    let finished = web.await_result(&finished_id).await;
    expect_ok(&finished);
    web.run_expect(
        pb::command::Body::Cancel(pb::Cancel {
            target_command_id: finished_id,
        }),
        0,
        pb::ErrorCode::Protocol,
    )
    .await;

    // --- multi-session + shutdown ------------------------------------------
    let second = harness
        .handshake(Handshake {
            platform: pb::Platform::Web as i32,
            endpoint: endpoint.clone(),
            app_url: String::new(),
            options: suite_options(backend, "index.html"),
            token: None,
            protocol: "1.0.0".to_string(),
        })
        .await
        .expect("second handshake");
    assert!(second.accepted, "second handshake: {:?}", second.error);
    assert_ne!(second.session_id, session_id);
    assert_eq!(harness.manager.session_count(), 2);
    // `devices check` flow: shutdown without ever opening the stream
    harness.shutdown(&second.session_id).await;
    assert_eq!(harness.manager.session_count(), 1);
    web.check_visible(css("#fixture-title"), 2_000).await;

    harness.shutdown(&web.session_id).await;
    let closed = web.await_closed().await;
    assert_eq!(closed.reason, "shutdown");
    assert_eq!(harness.manager.session_count(), 0);

    // --- TV session on the same fixture (D-pad focus) ----------------------
    let tv_session = harness
        .handshake(Handshake {
            platform: pb::Platform::Webos as i32,
            endpoint: endpoint.clone(),
            app_url: String::new(),
            options: suite_options(backend, "index.html"),
            token: None,
            protocol: "1.0.0".to_string(),
        })
        .await
        .expect("tv handshake");
    assert!(tv_session.accepted, "tv handshake: {:?}", tv_session.error);
    assert!(
        tv_session
            .capabilities
            .contains(&(pb::Capability::DpadFocus as i32)),
        "webos sessions advertise DpadFocus"
    );
    let mut tv = Driver::open(&mut harness.client, &tv_session.session_id).await;
    tv.run_ok(
        body(action(pb::ActionType::Click, Some(css("#grid-5")))),
        3_000,
    )
    .await;
    tv.text_is(css("#clicked-state"), "grid-5").await;
    tv.text_is(css("#focus-state"), "grid-5").await;
    tv.run_ok(
        body(action(pb::ActionType::Focus, Some(css("#grid-9")))),
        3_000,
    )
    .await;
    tv.run_ok(
        check_body(pb::CheckKind::Focused, css("#grid-9"), "", 0),
        500,
    )
    .await;
    let mut tv_scroll = action(pb::ActionType::Scroll, None);
    tv_scroll.direction = pb::Direction::Down as i32;
    tv_scroll.count = 2;
    tv.run_ok(body(tv_scroll), 3_000).await;
    harness.shutdown(&tv_session.session_id).await;
    let closed = tv.await_closed().await;
    assert_eq!(closed.reason, "shutdown");

    // --- element-container scrolling (second fixture, one page per Chrome) --
    let Some(mut scroll_chrome) = start_chrome("scroll.html").await else {
        panic!("Chromium disappeared mid-suite");
    };
    let scroll_endpoint = format!("ws://{}:{}", scroll_chrome.host, scroll_chrome.port);
    let scroll_session = harness
        .handshake(Handshake {
            platform: pb::Platform::Web as i32,
            endpoint: scroll_endpoint,
            app_url: String::new(),
            options: suite_options(backend, "scroll.html"),
            token: None,
            protocol: "1.0.0".to_string(),
        })
        .await
        .expect("scroll handshake");
    assert!(
        scroll_session.accepted,
        "scroll handshake: {:?}",
        scroll_session.error
    );
    let mut scrolling = Driver::open(&mut harness.client, &scroll_session.session_id).await;
    let mut container_scroll = action(pb::ActionType::Scroll, Some(css("#scroller")));
    container_scroll.direction = pb::Direction::Down as i32;
    container_scroll.count = 1;
    scrolling.run_ok(body(container_scroll), 2_000).await;
    scrolling.text_is(css("#scroller-state"), "scrolled").await;
    // `scrollto` inside the list resolves the container as the primary
    // scrollable and walks it to the end.
    scrolling
        .run_ok(
            body(action(pb::ActionType::ScrollTo, Some(css("#inner-target")))),
            5_000,
        )
        .await;
    scrolling.check_visible(css("#inner-target"), 500).await;
    let mut missing = action(pb::ActionType::Scroll, Some(css("#no-such-scroller")));
    missing.direction = pb::Direction::Down as i32;
    scrolling
        .run_expect(body(missing), 600, pb::ErrorCode::ElementNotFound)
        .await;
    harness.shutdown(&scroll_session.session_id).await;
    let closed = scrolling.await_closed().await;
    assert_eq!(closed.reason, "shutdown");
    scroll_chrome.kill().await;

    // --- launch mode (connector spawns Chromium) ---------------------------
    let launch_session = harness
        .handshake(Handshake {
            platform: pb::Platform::Web as i32,
            endpoint: "launch:headless".to_string(),
            app_url: fixture_url("index.html"),
            options: suite_options(backend, "").into_iter().take(1).collect(),
            token: None,
            protocol: "1.0.0".to_string(),
        })
        .await
        .expect("launch handshake");
    assert!(
        launch_session.accepted,
        "launch handshake: {:?}",
        launch_session.error
    );
    assert_eq!(harness.manager.session_count(), 1);
    let mut launch = Driver::open(&mut harness.client, &launch_session.session_id).await;
    // The freshly launched page is about:blank, not the fixture.
    launch
        .check_fails(pb::CheckKind::Visible, css("#fixture-title"), "", 600)
        .await;
    launch
        .run_ok(
            pb::command::Body::AppControl(pb::AppControl {
                op: pb::AppOp::Launch as i32,
                clear_data: false,
            }),
            10_000,
        )
        .await;
    launch
        .text_is(css("#fixture-title"), "Impresario web fixture")
        .await;
    launch
        .run_ok(
            body(action(pb::ActionType::Click, Some(css("#click-button")))),
            3_000,
        )
        .await;
    launch.text_is(css("#click-result"), "clicked").await;
    launch
        .run_ok(
            pb::command::Body::AppControl(pb::AppControl {
                op: pb::AppOp::Restart as i32,
                clear_data: false,
            }),
            10_000,
        )
        .await;
    launch.text_is(css("#click-result"), "-").await;
    harness.shutdown(&launch_session.session_id).await;
    let closed = launch.await_closed().await;
    assert_eq!(closed.reason, "shutdown");
    assert_eq!(harness.manager.session_count(), 0);

    chrome.kill().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixture_suite_raw_backend() {
    run_fixture_suite("raw").await;
}

#[cfg(feature = "chromiumoxide-backend")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixture_suite_chromiumoxide_backend() {
    run_fixture_suite("chromiumoxide").await;
}

// ---------------------------------------------------------------------------
// auth + protocol error mapping (no Chromium required)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_and_rpc_error_mapping() {
    let mut harness = Harness::start(Some("s3cret")).await;

    let spec = |endpoint: &str,
                platform: i32,
                protocol: &str,
                token: Option<&str>,
                options: Vec<(String, String)>| Handshake {
        platform,
        endpoint: endpoint.to_string(),
        app_url: String::new(),
        options,
        token: token.map(str::to_string),
        protocol: protocol.to_string(),
    };

    // Missing token.
    let status = harness
        .handshake(spec(
            "ws://127.0.0.1:1",
            pb::Platform::Web as i32,
            "1.0.0",
            None,
            vec![],
        ))
        .await
        .expect_err("missing token");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);

    // Wrong token.
    let status = harness
        .handshake(spec(
            "ws://127.0.0.1:1",
            pb::Platform::Web as i32,
            "1.0.0",
            Some("wrong"),
            vec![],
        ))
        .await
        .expect_err("wrong token");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);

    // Right token, unreachable endpoint: accepted=false + DEVICE_UNAVAILABLE.
    let response = harness
        .handshake(spec(
            "ws://127.0.0.1:1",
            pb::Platform::Web as i32,
            "1.0.0",
            Some("s3cret"),
            vec![],
        ))
        .await
        .expect("handshake rpc");
    assert!(!response.accepted);
    assert_eq!(
        response.error.as_ref().expect("error").code,
        pb::ErrorCode::DeviceUnavailable as i32
    );

    // Incompatible protocol major.
    let response = harness
        .handshake(spec(
            "ws://127.0.0.1:1",
            pb::Platform::Web as i32,
            "2.0.0",
            Some("s3cret"),
            vec![],
        ))
        .await
        .expect("handshake rpc");
    assert!(!response.accepted);
    assert_eq!(
        response.error.as_ref().expect("error").code,
        pb::ErrorCode::Protocol as i32
    );

    // Unsupported platform.
    let response = harness
        .handshake(spec(
            "ws://127.0.0.1:1",
            pb::Platform::Android as i32,
            "1.0.0",
            Some("s3cret"),
            vec![],
        ))
        .await
        .expect("handshake rpc");
    assert!(!response.accepted);
    assert_eq!(
        response.error.as_ref().expect("error").code,
        pb::ErrorCode::Protocol as i32
    );

    // Malformed documented option.
    let response = harness
        .handshake(spec(
            "ws://127.0.0.1:1",
            pb::Platform::Web as i32,
            "1.0.0",
            Some("s3cret"),
            vec![("cdp_backend".to_string(), "bogus".to_string())],
        ))
        .await
        .expect("handshake rpc");
    assert!(!response.accepted);
    assert_eq!(
        response.error.as_ref().expect("error").code,
        pb::ErrorCode::Protocol as i32
    );

    // Session without session-id metadata (auth passes first: the token is
    // attached, mirroring the master).
    {
        let mut client = harness.client.clone();
        let (_commands, receiver) = mpsc::channel::<pb::Command>(1);
        let mut request = Request::new(ReceiverStream::new(receiver));
        request
            .metadata_mut()
            .insert("x-impresario-token", "s3cret".parse().expect("token"));
        let status = client
            .session(request)
            .await
            .expect_err("missing session id");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    // Session with an unknown id (token attached).
    {
        let mut client = harness.client.clone();
        let (_commands, receiver) = mpsc::channel::<pb::Command>(1);
        let mut request = Request::new(ReceiverStream::new(receiver));
        request
            .metadata_mut()
            .insert("x-impresario-session-id", "session-99".parse().expect("id"));
        request
            .metadata_mut()
            .insert("x-impresario-token", "s3cret".parse().expect("token"));
        let status = client.session(request).await.expect_err("unknown session");
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    // StreamLogs with an unknown id.
    {
        let mut client = harness.client.clone();
        let mut request = Request::new(pb::LogRequest {
            min_level: pb::LogLevel::Debug as i32,
        });
        request
            .metadata_mut()
            .insert("x-impresario-session-id", "session-99".parse().expect("id"));
        request
            .metadata_mut()
            .insert("x-impresario-token", "s3cret".parse().expect("token"));
        let status = client
            .stream_logs(request)
            .await
            .expect_err("unknown session");
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    // CaptureScreenshot without metadata.
    {
        let mut client = harness.client.clone();
        let mut request = Request::new(pb::ScreenshotRequest {});
        request
            .metadata_mut()
            .insert("x-impresario-token", "s3cret".parse().expect("token"));
        let status = client
            .capture_screenshot(request)
            .await
            .expect_err("missing session id");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    // Shutdown of an unknown session is idempotent (decision I-36).
    harness.shutdown("session-42").await;
    assert_eq!(harness.manager.session_count(), 0);
}
