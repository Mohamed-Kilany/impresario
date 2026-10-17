//! `MockAgent`: an in-process tonic server implementing the `Agent` service
//! with scriptable behavior (delays, failures, hung commands, dropped
//! streams, heartbeats, tokens) for orchestrator integration tests
//! (spec section 9).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use impresario_core::command::{WireError, WireErrorCode};
use impresario_proto::v1 as pb;
use pb::agent_server::Agent as AgentService;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::metadata::MetadataMap;
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};

use crate::agent_client::MAX_MESSAGE_SIZE;
use crate::convert;
use crate::log::{unix_ms_now, LogRecord};

/// Scripted executor behavior. All knobs are optional; the defaults are a
/// well-behaved executor.
#[derive(Debug, Clone)]
pub struct MockScript {
    /// When set, `Handshake.auth_token` must equal this and every later RPC
    /// must carry `x-impresario-token`.
    pub token: Option<String>,
    /// Reject a second handshake with `ALREADY_IN_USE` (native agents).
    pub single_session: bool,
    /// Interval between heartbeats (spec: 5 s).
    pub heartbeat_interval: Duration,
    /// Stop emitting heartbeats after this many (for `TRANSPORT` tests).
    pub heartbeat_stop_after: Option<u64>,
    pub agent_version: String,
    pub protocol_version: String,
    /// Advertised `Capability` values (proto enums as `i32`).
    pub capabilities: Vec<i32>,
    pub device: pb::DeviceInfo,
    /// Reject every handshake with this error.
    pub reject: Option<WireError>,
    /// Per-command-kind delay before the result.
    pub delay_kinds: HashMap<String, Duration>,
    /// 1-based command index -> failure.
    pub failures: HashMap<usize, WireError>,
    /// Command kinds that hang until cancelled.
    pub hang_kinds: Vec<String>,
    /// Send `SessionClosed` and end the stream after this many results.
    pub drop_after_results: Option<usize>,
    pub close_reason: String,
    /// Error attached to the scripted `SessionClosed` (defaults to
    /// `TRANSPORT`).
    pub close_error: Option<WireError>,
    pub screenshot: Vec<u8>,
    pub screenshot_width: u32,
    pub screenshot_height: u32,
    /// Entries sent on `StreamLogs`.
    pub logs: Vec<LogRecord>,
    /// Close `StreamLogs` after this many entries (default: keep it open).
    pub log_stream_stops_after: Option<usize>,
}

impl Default for MockScript {
    fn default() -> Self {
        Self {
            token: None,
            single_session: false,
            heartbeat_interval: Duration::from_secs(5),
            heartbeat_stop_after: None,
            agent_version: "0.1.0-mock".to_string(),
            protocol_version: "1.0.0".to_string(),
            capabilities: mock_capabilities(),
            device: pb::DeviceInfo {
                model: "Mock Device".to_string(),
                os_name: "MockOS".to_string(),
                os_version: "1.0".to_string(),
                screen_width_px: 1080,
                screen_height_px: 1920,
                pixel_ratio: 1.0,
                runtime: "MockAgent".to_string(),
            },
            reject: None,
            delay_kinds: HashMap::new(),
            failures: HashMap::new(),
            hang_kinds: Vec::new(),
            drop_after_results: None,
            close_reason: "scripted stream drop".to_string(),
            close_error: None,
            screenshot: b"mock-png".to_vec(),
            screenshot_width: 1080,
            screenshot_height: 1920,
            logs: Vec::new(),
            log_stream_stops_after: None,
        }
    }
}

/// Capabilities of a well-behaved native agent (no `MULTI_SESSION`).
#[must_use]
pub fn mock_capabilities() -> Vec<i32> {
    vec![
        pb::Capability::Logs as i32,
        pb::Capability::Screenshot as i32,
        pb::Capability::TouchGestures as i32,
        pb::Capability::DpadFocus as i32,
        pb::Capability::ProgrammaticFocus as i32,
        pb::Capability::KeyEvents as i32,
        pb::Capability::AppControl as i32,
    ]
}

/// Records everything the mock saw; assertions live on this type.
pub struct MockState {
    script: MockScript,
    commands: Mutex<Vec<pb::Command>>,
    notify: Notify,
    handshakes: AtomicU64,
    sessions: AtomicU64,
    captures: AtomicU64,
    log_streams: AtomicU64,
    shutdowns: AtomicU64,
    log_senders: Mutex<Vec<mpsc::Sender<Result<pb::LogEntry, Status>>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl MockState {
    fn new(script: MockScript) -> Self {
        Self {
            script,
            commands: Mutex::new(Vec::new()),
            notify: Notify::new(),
            handshakes: AtomicU64::new(0),
            sessions: AtomicU64::new(0),
            captures: AtomicU64::new(0),
            log_streams: AtomicU64::new(0),
            shutdowns: AtomicU64::new(0),
            log_senders: Mutex::new(Vec::new()),
        }
    }

    /// The script, for assertions.
    #[must_use]
    pub fn script(&self) -> &MockScript {
        &self.script
    }

    /// Every command received so far, in order.
    #[must_use]
    pub fn commands(&self) -> Vec<pb::Command> {
        lock(&self.commands).clone()
    }

    #[must_use]
    pub fn command_count(&self) -> usize {
        lock(&self.commands).len()
    }

    /// Oneof arm names of the received commands (`"run_action"`, ...).
    #[must_use]
    pub fn command_kinds(&self) -> Vec<String> {
        lock(&self.commands)
            .iter()
            .map(command_kind)
            .map(str::to_string)
            .collect()
    }

    #[must_use]
    pub fn handshakes(&self) -> u64 {
        self.handshakes.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn sessions(&self) -> u64 {
        self.sessions.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn captures(&self) -> u64 {
        self.captures.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn log_streams(&self) -> u64 {
        self.log_streams.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn shutdowns(&self) -> u64 {
        self.shutdowns.load(Ordering::SeqCst)
    }

    /// Wait until at least `count` commands arrived (or `timeout`).
    pub async fn wait_for_commands(&self, count: usize, timeout: Duration) -> Vec<pb::Command> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.command_count() >= count {
                return self.commands();
            }
            if tokio::time::Instant::now() >= deadline {
                return self.commands();
            }
            let _ = tokio::time::timeout(Duration::from_millis(25), self.notify.notified()).await;
        }
    }

    fn record_command(&self, command: &pb::Command) {
        lock(&self.commands).push(command.clone());
        self.notify.notify_waiters();
    }
}

/// The oneof arm name of a command (`"unknown"` for an empty body).
#[must_use]
pub fn command_kind(command: &pb::Command) -> &'static str {
    match &command.body {
        Some(pb::command::Body::RunAction(_)) => "run_action",
        Some(pb::command::Body::ValidateCheck(_)) => "validate_check",
        Some(pb::command::Body::KeyPress(_)) => "key_press",
        Some(pb::command::Body::Screenshot(_)) => "screenshot",
        Some(pb::command::Body::AppControl(_)) => "app_control",
        Some(pb::command::Body::Cancel(_)) => "cancel",
        None => "unknown",
    }
}

/// Builder for a [`MockAgent`].
pub struct MockAgentBuilder {
    script: MockScript,
}

impl Default for MockAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MockAgentBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            script: MockScript::default(),
        }
    }

    #[must_use]
    pub fn script(mut self, script: MockScript) -> Self {
        self.script = script;
        self
    }

    /// Heartbeat interval (default 5 s).
    #[must_use]
    pub fn heartbeat_interval(mut self, interval: Duration) -> Self {
        self.script.heartbeat_interval = interval;
        self
    }

    /// Stop heartbeats after `n` beats (for `TRANSPORT` failure tests).
    #[must_use]
    pub fn stop_heartbeats_after(mut self, n: u64) -> Self {
        self.script.heartbeat_stop_after = Some(n);
        self
    }

    /// Require `token` on the handshake and on every later RPC.
    #[must_use]
    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.script.token = Some(token.into());
        self
    }

    /// Reject a second handshake with `ALREADY_IN_USE`.
    #[must_use]
    pub fn single_session(mut self) -> Self {
        self.script.single_session = true;
        self
    }

    /// Make the `n`-th command (1-based) fail with `code`.
    #[must_use]
    pub fn fail_command(
        mut self,
        n: usize,
        code: WireErrorCode,
        message: impl Into<String>,
    ) -> Self {
        self.script
            .failures
            .insert(n, WireError::new(code, message));
        self
    }

    /// Delay every command of `kind` by `ms` before answering.
    #[must_use]
    pub fn delay_kind(mut self, kind: &str, ms: u64) -> Self {
        self.script
            .delay_kinds
            .insert(kind.to_string(), Duration::from_millis(ms));
        self
    }

    /// Commands of `kind` hang until the master sends `Cancel`.
    #[must_use]
    pub fn hang_kind(mut self, kind: &str) -> Self {
        self.script.hang_kinds.push(kind.to_string());
        self
    }

    /// End the session stream with `SessionClosed` after `n` results.
    #[must_use]
    pub fn drop_stream_after_results(mut self, n: usize) -> Self {
        self.script.drop_after_results = Some(n);
        self
    }

    /// End the session stream with `SessionClosed` after `n` results,
    /// attaching `code` (defaults to `TRANSPORT`).
    #[must_use]
    pub fn drop_stream_after_results_with(
        mut self,
        n: usize,
        code: WireErrorCode,
        reason: impl Into<String>,
    ) -> Self {
        self.script.drop_after_results = Some(n);
        self.script.close_reason = reason.into();
        self.script.close_error = Some(WireError::new(code, self.script.close_reason.clone()));
        self
    }

    /// Reject every handshake with `code`.
    #[must_use]
    pub fn reject_handshake(mut self, code: WireErrorCode, message: impl Into<String>) -> Self {
        self.script.reject = Some(WireError::new(code, message));
        self
    }

    /// PNG bytes returned by `Screenshot` commands and the out-of-band RPC.
    #[must_use]
    pub fn screenshot(mut self, png: Vec<u8>) -> Self {
        self.script.screenshot = png;
        self
    }

    /// Add one advertised capability.
    #[must_use]
    pub fn capability(mut self, capability: pb::Capability) -> Self {
        self.script.capabilities.push(capability as i32);
        self
    }

    /// Entries that `StreamLogs` sends immediately after subscribing.
    #[must_use]
    pub fn logs(mut self, logs: Vec<LogRecord>) -> Self {
        self.script.logs = logs;
        self
    }

    /// Bind `127.0.0.1:0` and serve until the [`MockAgent`] is dropped.
    pub async fn start(self) -> std::io::Result<MockAgent> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let state = Arc::new(MockState::new(self.script));
        let service = pb::agent_server::AgentServer::new(MockService {
            state: state.clone(),
        })
        .max_decoding_message_size(MAX_MESSAGE_SIZE)
        .max_encoding_message_size(MAX_MESSAGE_SIZE);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = Server::builder()
            .http2_keepalive_interval(Some(Duration::from_secs(10)))
            .http2_keepalive_timeout(Some(Duration::from_secs(20)))
            .add_service(service)
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                let _ = shutdown_rx.await;
            });
        let task = tokio::spawn(server);
        Ok(MockAgent {
            address,
            state,
            shutdown: Some(shutdown_tx),
            task,
        })
    }
}

/// A running mock executor.
pub struct MockAgent {
    address: std::net::SocketAddr,
    state: Arc<MockState>,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
}

impl MockAgent {
    #[must_use]
    pub fn builder() -> MockAgentBuilder {
        MockAgentBuilder::new()
    }

    /// `host:port` of the mock server.
    #[must_use]
    pub fn address(&self) -> String {
        self.address.to_string()
    }

    #[must_use]
    pub fn state(&self) -> Arc<MockState> {
        self.state.clone()
    }

    /// Wait until at least `count` commands arrived (or `timeout`).
    pub async fn wait_for_commands(&self, count: usize, timeout: Duration) -> Vec<pb::Command> {
        self.state.wait_for_commands(count, timeout).await
    }
}

impl Drop for MockAgent {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

#[derive(Clone)]
struct MockService {
    state: Arc<MockState>,
}

fn metadata_str(map: &MetadataMap, key: &str) -> Option<String> {
    map.get(key)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn require_token(
    state: &MockState,
    metadata: &MetadataMap,
    session_scoped: bool,
) -> Result<(), Status> {
    if let Some(expected) = &state.script.token {
        match metadata_str(metadata, "x-impresario-token").as_deref() {
            Some(value) if value == expected => {}
            _ => {
                return Err(Status::unauthenticated(
                    "missing or invalid x-impresario-token",
                ))
            }
        }
    }
    if session_scoped && metadata_str(metadata, "x-impresario-session-id").is_none() {
        return Err(Status::unauthenticated("missing x-impresario-session-id"));
    }
    Ok(())
}

#[tonic::async_trait]
impl AgentService for MockService {
    async fn handshake(
        &self,
        request: Request<pb::HandshakeRequest>,
    ) -> Result<Response<pb::HandshakeResponse>, Status> {
        let nth = self.state.handshakes.fetch_add(1, Ordering::SeqCst) + 1;
        let script = self.state.script.clone();
        let request = request.into_inner();
        let response = |accepted: bool, session_id: String, error: Option<pb::ErrorInfo>| {
            pb::HandshakeResponse {
                accepted,
                session_id,
                agent_version: script.agent_version.clone(),
                protocol_version: script.protocol_version.clone(),
                capabilities: if accepted {
                    script.capabilities.clone()
                } else {
                    Vec::new()
                },
                device: if accepted {
                    Some(script.device.clone())
                } else {
                    None
                },
                error,
            }
        };
        let reject = |code: WireErrorCode, message: &str| {
            response(
                false,
                String::new(),
                Some(convert::error_info_to_proto(&WireError::new(code, message))),
            )
        };

        if let Some(expected) = &script.token {
            if request.auth_token != *expected {
                return Ok(Response::new(reject(
                    WireErrorCode::Unauthenticated,
                    "missing or invalid auth_token",
                )));
            }
        }
        if let Some(error) = &script.reject {
            return Ok(Response::new(response(
                false,
                String::new(),
                Some(convert::error_info_to_proto(error)),
            )));
        }
        if script.single_session && nth > 1 {
            return Ok(Response::new(reject(
                WireErrorCode::AlreadyInUse,
                "a session is already active (single-session executor)",
            )));
        }
        Ok(Response::new(response(
            true,
            format!("mock-session-{nth}"),
            None,
        )))
    }

    type SessionStream = ReceiverStream<Result<pb::Event, Status>>;

    async fn session(
        &self,
        request: Request<Streaming<pb::Command>>,
    ) -> Result<Response<Self::SessionStream>, Status> {
        require_token(&self.state, request.metadata(), true)?;
        self.state.sessions.fetch_add(1, Ordering::SeqCst);
        let incoming = request.into_inner();
        let (out_tx, out_rx) = mpsc::channel::<Result<pb::Event, Status>>(128);
        let state = self.state.clone();
        tokio::spawn(run_session(state, incoming, out_tx));
        Ok(Response::new(ReceiverStream::new(out_rx)))
    }

    async fn capture_screenshot(
        &self,
        request: Request<pb::ScreenshotRequest>,
    ) -> Result<Response<pb::ScreenshotResponse>, Status> {
        require_token(&self.state, request.metadata(), true)?;
        self.state.captures.fetch_add(1, Ordering::SeqCst);
        let script = self.state.script.clone();
        Ok(Response::new(pb::ScreenshotResponse {
            png: script.screenshot,
            width: script.screenshot_width,
            height: script.screenshot_height,
            error: None,
        }))
    }

    type StreamLogsStream = ReceiverStream<Result<pb::LogEntry, Status>>;

    async fn stream_logs(
        &self,
        request: Request<pb::LogRequest>,
    ) -> Result<Response<Self::StreamLogsStream>, Status> {
        require_token(&self.state, request.metadata(), true)?;
        self.state.log_streams.fetch_add(1, Ordering::SeqCst);
        let script = self.state.script.clone();
        let (tx, rx) = mpsc::channel::<Result<pb::LogEntry, Status>>(64);
        for record in &script.logs {
            let entry = pb::LogEntry {
                unix_ms: record.unix_ms,
                level: convert::log_level_to_proto(record.level) as i32,
                source: record.source.clone(),
                message: record.message.clone(),
            };
            let _ = tx.try_send(Ok(entry));
        }
        // Keep the stream open like a real executor, unless the script says
        // to stop after `n` entries.
        let keep_open = match script.log_stream_stops_after {
            Some(limit) => script.logs.len() < limit,
            None => true,
        };
        if keep_open {
            lock(&self.state.log_senders).push(tx);
        }
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn shutdown(
        &self,
        request: Request<pb::ShutdownRequest>,
    ) -> Result<Response<pb::ShutdownResponse>, Status> {
        require_token(&self.state, request.metadata(), true)?;
        self.state.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(Response::new(pb::ShutdownResponse {}))
    }
}

/// Tracks the in-flight command and routes `Cancel` to it (spec 7.2.5:
/// cancellation is processed immediately, even while another command runs).
struct Canceller {
    active: Arc<Mutex<Option<String>>>,
    wake: Mutex<Option<Arc<Notify>>>,
    matched: Mutex<HashSet<String>>,
    closed: Arc<Notify>,
}

impl Canceller {
    fn new() -> Self {
        Self {
            active: Arc::new(Mutex::new(None)),
            wake: Mutex::new(None),
            matched: Mutex::new(HashSet::new()),
            closed: Arc::new(Notify::new()),
        }
    }

    fn register(&self, id: &str) -> Arc<Notify> {
        let notify = Arc::new(Notify::new());
        *lock(&self.active) = Some(id.to_string());
        *lock(&self.wake) = Some(notify.clone());
        notify
    }

    fn clear(&self, id: &str) {
        let mut active = lock(&self.active);
        if active.as_deref() == Some(id) {
            *active = None;
            *lock(&self.wake) = None;
        }
    }

    /// Notify the active command when it matches; returns whether it did.
    fn cancel_if_active(&self, id: &str) -> bool {
        let active = lock(&self.active);
        if active.as_deref() == Some(id) {
            if let Some(notify) = lock(&self.wake).as_ref() {
                notify.notify_waiters();
            }
            lock(&self.matched).insert(id.to_string());
            true
        } else {
            false
        }
    }

    fn take_match(&self, id: &str) -> bool {
        lock(&self.matched).remove(id)
    }

    fn close(&self) {
        self.closed.notify_waiters();
    }
}

async fn run_session(
    state: Arc<MockState>,
    mut incoming: Streaming<pb::Command>,
    out: mpsc::Sender<Result<pb::Event, Status>>,
) {
    let canceller = Arc::new(Canceller::new());
    let heartbeat = tokio::spawn(heartbeat_loop(
        state.clone(),
        canceller.active.clone(),
        out.clone(),
    ));
    let (command_tx, mut command_rx) = mpsc::channel::<pb::Command>(64);

    // Reader: forwards commands, resolves `Cancel` immediately.
    let reader_canceller = canceller.clone();
    let reader_state = state.clone();
    let reader = tokio::spawn(async move {
        while let Ok(Some(command)) = incoming.message().await {
            reader_state.record_command(&command);
            if let Some(pb::command::Body::Cancel(cancel)) = &command.body {
                reader_canceller.cancel_if_active(&cancel.target_command_id);
            }
            if command_tx.send(command).await.is_err() {
                break;
            }
        }
        reader_canceller.close();
    });

    // Worker: one command at a time, strictly in order (spec 7.2.4).
    let mut index = 0usize;
    let mut results = 0usize;
    while let Some(command) = command_rx.recv().await {
        index += 1;
        let kind = command_kind(&command);
        let started = std::time::Instant::now();
        let mut cancelled = false;
        let mut stop_after_result = false;

        if kind != "cancel" && state.script.hang_kinds.iter().any(|k| k == kind) {
            let id = command.command_id.clone();
            let wake = canceller.register(&id);
            tokio::select! {
                _ = wake.notified() => cancelled = true,
                _ = canceller.closed.notified() => stop_after_result = true,
                _ = out.closed() => stop_after_result = true,
            }
            canceller.clear(&id);
        } else if kind != "cancel" {
            if let Some(delay) = state.script.delay_kinds.get(kind).copied() {
                let id = command.command_id.clone();
                let wake = canceller.register(&id);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = wake.notified() => cancelled = true,
                    _ = canceller.closed.notified() => stop_after_result = true,
                    _ = out.closed() => stop_after_result = true,
                }
                canceller.clear(&id);
            }
        }

        if stop_after_result {
            break;
        }

        let mut result = if cancelled {
            cancelled_result(&command)
        } else if let Some(pb::command::Body::Cancel(cancel)) = &command.body {
            let matched = canceller.take_match(&cancel.target_command_id);
            if matched {
                ok_result(&command)
            } else {
                failure_result(
                    &command,
                    &WireError::new(
                        WireErrorCode::Protocol,
                        format!(
                            "cancel of unknown or inactive command id '{}'",
                            cancel.target_command_id
                        ),
                    ),
                )
            }
        } else {
            scripted_result(&state.script, index, &command)
        };
        result.duration_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);

        results += 1;
        let event = pb::Event {
            body: Some(pb::event::Body::Result(result)),
        };
        if out.send(Ok(event)).await.is_err() {
            break;
        }
        if let Some(limit) = state.script.drop_after_results {
            if results >= limit {
                let error =
                    state.script.close_error.clone().unwrap_or_else(|| {
                        WireError::new(WireErrorCode::Transport, "stream dropped")
                    });
                let closed = pb::SessionClosed {
                    reason: state.script.close_reason.clone(),
                    error: Some(convert::error_info_to_proto(&error)),
                };
                let _ = out
                    .send(Ok(pb::Event {
                        body: Some(pb::event::Body::Closed(closed)),
                    }))
                    .await;
                break;
            }
        }
    }

    drop(command_rx);
    canceller.close();
    reader.abort();
    heartbeat.abort();
}

async fn heartbeat_loop(
    state: Arc<MockState>,
    active: Arc<Mutex<Option<String>>>,
    out: mpsc::Sender<Result<pb::Event, Status>>,
) {
    let script = state.script.clone();
    if script.heartbeat_interval.is_zero() {
        return;
    }
    let mut seq = 0u64;
    loop {
        tokio::time::sleep(script.heartbeat_interval).await;
        if let Some(limit) = script.heartbeat_stop_after {
            if seq >= limit {
                continue;
            }
        }
        seq += 1;
        let current_command_id = lock(&active).clone().unwrap_or_default();
        let event = pb::Event {
            body: Some(pb::event::Body::Heartbeat(pb::Heartbeat {
                seq,
                unix_ms: u64::try_from(unix_ms_now()).unwrap_or(0),
                current_command_id,
            })),
        };
        if out.send(Ok(event)).await.is_err() {
            break;
        }
    }
}

fn ok_result(command: &pb::Command) -> pb::CommandResult {
    pb::CommandResult {
        command_id: command.command_id.clone(),
        ok: true,
        ..Default::default()
    }
}

fn cancelled_result(command: &pb::Command) -> pb::CommandResult {
    failure_result(
        command,
        &WireError::new(WireErrorCode::Cancelled, "command was cancelled"),
    )
}

fn failure_result(command: &pb::Command, error: &WireError) -> pb::CommandResult {
    pb::CommandResult {
        command_id: command.command_id.clone(),
        ok: false,
        error: Some(convert::error_info_to_proto(error)),
        ..Default::default()
    }
}

fn scripted_result(script: &MockScript, index: usize, command: &pb::Command) -> pb::CommandResult {
    if let Some(error) = script.failures.get(&index) {
        return failure_result(command, error);
    }
    let mut result = pb::CommandResult {
        command_id: command.command_id.clone(),
        ok: true,
        ..Default::default()
    };
    match &command.body {
        Some(pb::command::Body::Screenshot(_)) => {
            result.screenshot_png = script.screenshot.clone();
        }
        Some(pb::command::Body::ValidateCheck(check)) => {
            result.observed_text = check.expected_text.clone();
            result.observed_count = check.expected_count;
        }
        _ => {}
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_core::command::Command;

    #[test]
    fn default_script_is_well_behaved() {
        let script = MockScript::default();
        assert_eq!(script.heartbeat_interval, Duration::from_secs(5));
        assert!(script.token.is_none());
        assert!(script.failures.is_empty());
        assert!(!script.capabilities.is_empty());
        assert!(!script
            .capabilities
            .contains(&(pb::Capability::MultiSession as i32)));
    }

    #[test]
    fn command_kinds_are_stable() {
        let command = convert::command_to_proto(
            &Command::RunAction(impresario_core::command::RunAction::back()),
            "c",
            0,
        );
        assert_eq!(command_kind(&command), "run_action");
        let command = pb::Command {
            command_id: "c".to_string(),
            timeout_ms: 0,
            body: None,
        };
        assert_eq!(command_kind(&command), "unknown");
    }

    #[test]
    fn builder_accumulates_script() {
        let builder = MockAgentBuilder::new()
            .token("t")
            .heartbeat_interval(Duration::from_millis(50))
            .fail_command(2, WireErrorCode::ElementNotFound, "nope")
            .hang_kind("run_action")
            .drop_stream_after_results(1)
            .single_session();
        assert_eq!(builder.script.token.as_deref(), Some("t"));
        assert_eq!(builder.script.heartbeat_interval, Duration::from_millis(50));
        assert!(builder.script.failures.contains_key(&2));
        assert_eq!(builder.script.hang_kinds, vec!["run_action".to_string()]);
        assert_eq!(builder.script.drop_after_results, Some(1));
        assert!(builder.script.single_session);
    }
}
