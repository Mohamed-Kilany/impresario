//! Master-side gRPC client for the `Agent` service (spec 7.2 rules, 7.3
//! lifecycle, section 9).
//!
//! [`AgentClient`] connects to one executor and performs the handshake;
//! [`AgentSession`] owns the bidirectional `Session` stream: it sends
//! commands, demultiplexes `Event`s to the waiting caller, watches
//! heartbeats and enforces the master-side deadline of spec 7.2.7
//! (`timeout_ms + command_grace_ms`, then `Cancel`, 2 s, `TIMEOUT`).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::{select, Either};
use impresario_core::command::{Command, CommandOutcome, WireError, WireErrorCode};
use impresario_core::platforms::Platform;
use impresario_proto::v1 as pb;
use pb::agent_client::AgentClient as ProtoAgent;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::{MetadataKey, MetadataMap, MetadataValue};
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Streaming};

use crate::convert;
use crate::log::{LogLevel, LogStream};

/// Wire protocol version this client speaks (spec 7.2.2).
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Major component of [`PROTOCOL_VERSION`]; majors must match.
pub const PROTOCOL_MAJOR: u32 = 1;

/// gRPC max message size, both directions (spec 7.2.11).
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// HTTP/2 keepalive interval (spec 7.2.11).
pub const HTTP2_KEEPALIVE: Duration = Duration::from_secs(10);

/// Per-attempt TCP connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Outgoing command queue depth; commands are rarely queued (one at a time).
const COMMAND_QUEUE: usize = 32;

/// Number of consecutive missed 5 s heartbeats after which the session is
/// declared dead (`TRANSPORT`; D-12, no resume).
const MISSED_HEARTBEATS: u32 = 3;

const SESSION_ID_KEY: &str = "x-impresario-session-id";
const TOKEN_KEY: &str = "x-impresario-token";

type WaiterSender = oneshot::Sender<Result<CommandOutcome, ClientError>>;

/// Client-side failures. Command *results* (including `ok=false` outcomes)
/// are values, not errors; this type is for transport, protocol, deadline
/// and session-lifecycle failures.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ClientError {
    #[error("transport failure: {0}")]
    Transport(String),
    #[error("protocol violation: {0}")]
    Protocol(String),
    #[error("session closed: {reason}")]
    SessionClosed {
        reason: String,
        error: Option<WireError>,
    },
    #[error(
        "command '{command_id}' exceeded the master deadline ({timeout_ms} ms timeout + {grace_ms} ms grace); sent Cancel"
    )]
    Timeout {
        command_id: String,
        timeout_ms: u32,
        grace_ms: u64,
    },
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),
    #[error("handshake rejected: {}", .0.message)]
    HandshakeRejected(WireError),
}

impl ClientError {
    /// The wire error code this failure maps to.
    #[must_use]
    pub fn code(&self) -> WireErrorCode {
        match self {
            ClientError::Transport(_) => WireErrorCode::Transport,
            ClientError::Protocol(_) => WireErrorCode::Protocol,
            ClientError::SessionClosed { error, .. } => error
                .as_ref()
                .map_or(WireErrorCode::Transport, |error| error.code),
            ClientError::Timeout { .. } => WireErrorCode::Timeout,
            ClientError::Unauthenticated(_) => WireErrorCode::Unauthenticated,
            ClientError::HandshakeRejected(error) => error.code,
        }
    }

    /// The equivalent wire error (details preserved when present).
    #[must_use]
    pub fn to_wire(&self) -> WireError {
        match self {
            ClientError::SessionClosed {
                error: Some(error), ..
            } => error.clone(),
            ClientError::HandshakeRejected(error) => error.clone(),
            other => WireError::new(other.code(), other.to_string()),
        }
    }

    pub(crate) fn from_status(status: tonic::Status) -> Self {
        let code = status.code();
        if code == tonic::Code::Unauthenticated {
            ClientError::Unauthenticated(status.message().to_string())
        } else {
            ClientError::Transport(format!("gRPC {code}: {}", status.message()))
        }
    }
}

impl From<WireError> for ClientError {
    fn from(error: WireError) -> Self {
        match error.code {
            WireErrorCode::Unauthenticated => ClientError::Unauthenticated(error.message),
            _ => ClientError::Protocol(error.message),
        }
    }
}

/// Tunables for one session. Defaults are the spec values; tests shrink the
/// timings so transport failure handling runs in milliseconds.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// No heartbeat for this long -> `TRANSPORT` (defaults to 15 s = three
    /// missed 5 s heartbeats).
    pub heartbeat_timeout: Duration,
    /// Grace given to a cancelled command before teardown (default 2 s).
    pub cancel_wait: Duration,
    /// How often the heartbeat watchdog wakes (default 250 ms).
    pub watchdog_poll: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            heartbeat_timeout: Duration::from_secs(5 * u64::from(MISSED_HEARTBEATS)),
            cancel_wait: Duration::from_secs(2),
            watchdog_poll: Duration::from_millis(250),
        }
    }
}

/// One `Capability` of the handshake, without the proto sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    MultiSession,
    Logs,
    Screenshot,
    TouchGestures,
    DpadFocus,
    ProgrammaticFocus,
    KeyEvents,
    AppControl,
}

impl Capability {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::MultiSession => "MULTI_SESSION",
            Capability::Logs => "LOGS",
            Capability::Screenshot => "SCREENSHOT",
            Capability::TouchGestures => "TOUCH_GESTURES",
            Capability::DpadFocus => "DPAD_FOCUS",
            Capability::ProgrammaticFocus => "PROGRAMMATIC_FOCUS",
            Capability::KeyEvents => "KEY_EVENTS",
            Capability::AppControl => "APP_CONTROL",
        }
    }

    fn from_proto(value: i32) -> Option<Self> {
        match pb::Capability::try_from(value) {
            Ok(pb::Capability::MultiSession) => Some(Capability::MultiSession),
            Ok(pb::Capability::Logs) => Some(Capability::Logs),
            Ok(pb::Capability::Screenshot) => Some(Capability::Screenshot),
            Ok(pb::Capability::TouchGestures) => Some(Capability::TouchGestures),
            Ok(pb::Capability::DpadFocus) => Some(Capability::DpadFocus),
            Ok(pb::Capability::ProgrammaticFocus) => Some(Capability::ProgrammaticFocus),
            Ok(pb::Capability::KeyEvents) => Some(Capability::KeyEvents),
            Ok(pb::Capability::AppControl) => Some(Capability::AppControl),
            // Unknown capabilities are ignored for forward compatibility
            // (7.2.2: a newer executor may advertise more than we know).
            Ok(pb::Capability::Unspecified) | Err(_) => None,
        }
    }
}

/// Handshake device information (spec 7.1 `DeviceInfo`, printed by
/// `devices check`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub model: String,
    pub os_name: String,
    pub os_version: String,
    pub screen_width_px: u32,
    pub screen_height_px: u32,
    pub pixel_ratio: f32,
    /// e.g. `"Chromium/79"`, `"UiAutomator2"`, `"XCUITest"`.
    pub runtime: String,
}

impl DeviceInfo {
    fn from_proto(info: &pb::DeviceInfo) -> Self {
        Self {
            model: info.model.clone(),
            os_name: info.os_name.clone(),
            os_version: info.os_version.clone(),
            screen_width_px: info.screen_width_px,
            screen_height_px: info.screen_height_px,
            pixel_ratio: info.pixel_ratio,
            runtime: info.runtime.clone(),
        }
    }
}

/// What the master sends in `Handshake` (spec 7.1).
#[derive(Debug, Clone)]
pub struct HandshakeParams {
    pub protocol_version: String,
    pub master_version: String,
    pub device_name: String,
    pub platform: Platform,
    pub target_endpoint: String,
    pub app_id: Option<String>,
    pub app_url: Option<String>,
    pub options: BTreeMap<String, String>,
    /// Required when the executor is configured with a token.
    pub auth_token: Option<String>,
}

impl HandshakeParams {
    #[must_use]
    pub fn new(
        device_name: impl Into<String>,
        platform: Platform,
        target_endpoint: impl Into<String>,
        master_version: impl Into<String>,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_string(),
            master_version: master_version.into(),
            device_name: device_name.into(),
            platform,
            target_endpoint: target_endpoint.into(),
            app_id: None,
            app_url: None,
            options: BTreeMap::new(),
            auth_token: None,
        }
    }
}

/// The accepted handshake.
#[derive(Debug, Clone)]
pub struct HandshakeOutcome {
    pub accepted: bool,
    pub session_id: String,
    pub agent_version: String,
    pub protocol_version: String,
    pub capabilities: Vec<Capability>,
    pub device: Option<DeviceInfo>,
    pub error: Option<WireError>,
}

impl HandshakeOutcome {
    #[must_use]
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// `CaptureScreenshot` reply (spec 7.1: out-of-band, may overlap a hung
/// session).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureOutcome {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub error: Option<WireError>,
}

/// The major component of a `major.minor.patch` version string.
#[must_use]
pub fn protocol_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

/// A connected executor before a session is opened.
#[derive(Debug, Clone)]
pub struct AgentClient {
    inner: ProtoAgent<Channel>,
    token: Option<String>,
    address: String,
}

impl AgentClient {
    /// Connect to `addr` (`host:port`; a `scheme://` prefix is accepted and
    /// passed through). Message limits and HTTP/2 keepalive follow
    /// spec 7.2.11.
    pub async fn connect(addr: &str) -> Result<Self, ClientError> {
        let uri = if addr.contains("://") {
            addr.to_string()
        } else {
            format!("http://{addr}")
        };
        let endpoint = Endpoint::from_shared(uri)
            .map_err(|e| ClientError::Transport(format!("invalid agent address '{addr}': {e}")))?
            .tcp_nodelay(true)
            .connect_timeout(CONNECT_TIMEOUT)
            .http2_keep_alive_interval(HTTP2_KEEPALIVE)
            .keep_alive_timeout(HTTP2_KEEPALIVE * 2)
            .keep_alive_while_idle(true);
        let channel = endpoint.connect().await.map_err(|e| {
            ClientError::Transport(format!("cannot reach the agent at {addr}: {e}"))
        })?;
        let inner = ProtoAgent::new(channel)
            .max_decoding_message_size(MAX_MESSAGE_SIZE)
            .max_encoding_message_size(MAX_MESSAGE_SIZE);
        Ok(Self {
            inner,
            token: None,
            address: addr.to_string(),
        })
    }

    /// Connect with the spec 7.3 retry: exponential backoff 200 ms -> 2 s
    /// until `agent_start_timeout` elapses.
    pub async fn connect_retry(
        addr: &str,
        agent_start_timeout: Duration,
    ) -> Result<Self, ClientError> {
        let deadline = Instant::now() + agent_start_timeout;
        let mut backoff = Duration::from_millis(200);
        loop {
            match Self::connect(addr).await {
                Ok(client) => return Ok(client),
                Err(error) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(error);
                    }
                    let sleep_for = backoff.min(deadline - now);
                    tokio::time::sleep(sleep_for).await;
                    backoff = (backoff * 2).min(Duration::from_secs(2));
                }
            }
        }
    }

    /// Send `x-impresario-token` on every RPC (spec 7.2.1).
    #[must_use]
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }

    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Perform the handshake and verify acceptance and protocol major
    /// compatibility (spec 7.2.2). A rejected handshake is an error carrying
    /// the executor's `ErrorInfo` (e.g. `ALREADY_IN_USE`).
    pub async fn handshake(
        &self,
        params: &HandshakeParams,
    ) -> Result<HandshakeOutcome, ClientError> {
        let request = signed_request(handshake_request(params), self.token.as_deref(), None)?;
        let response = self
            .inner
            .clone()
            .handshake(request)
            .await
            .map_err(ClientError::from_status)?
            .into_inner();
        let outcome = handshake_outcome(&response)?;
        if !outcome.accepted {
            let error = outcome.error.clone().unwrap_or_else(|| {
                WireError::new(
                    WireErrorCode::Protocol,
                    "handshake rejected without ErrorInfo",
                )
            });
            return Err(ClientError::HandshakeRejected(error));
        }
        if outcome.session_id.is_empty() {
            return Err(ClientError::Protocol(
                "handshake accepted without a session_id".to_string(),
            ));
        }
        match protocol_major(&outcome.protocol_version) {
            Some(major) if major == PROTOCOL_MAJOR => {}
            _ => {
                return Err(ClientError::Protocol(format!(
                "protocol major mismatch: master speaks {PROTOCOL_VERSION}, agent answered '{}'",
                outcome.protocol_version
            )))
            }
        }
        Ok(outcome)
    }

    /// Open the `Session` stream (spec 7.2: one stream per session; commands
    /// are executed strictly in order by the executor).
    pub async fn open_session(
        &self,
        session_id: &str,
        options: SessionOptions,
    ) -> Result<AgentSession, ClientError> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<pb::Command>(COMMAND_QUEUE);
        let request = signed_request(
            ReceiverStream::new(cmd_rx),
            self.token.as_deref(),
            Some(session_id),
        )?;
        let response = self
            .inner
            .clone()
            .session(request)
            .await
            .map_err(ClientError::from_status)?;
        let state = Arc::new(SessionState::new());
        let demux = tokio::spawn(demux_events(response.into_inner(), state.clone()));
        let watchdog = spawn_watchdog(state.clone(), options.clone());
        Ok(AgentSession {
            client: self.inner.clone(),
            session_id: session_id.to_string(),
            token: self.token.clone(),
            cmd_tx,
            state,
            demux,
            watchdog,
            options,
            prefix: command_prefix(),
            seq: AtomicU64::new(0),
            in_flight: Mutex::new(None),
        })
    }

    /// Ask the executor to end the session identified by `session_id`
    /// (spec 7.3 step 6) without an open `Session` stream. Used by
    /// `impresario devices check`, which handshakes and then tears down.
    pub async fn shutdown_session(
        &self,
        session_id: &str,
        terminate_app: bool,
    ) -> Result<(), ClientError> {
        let request = signed_request(
            pb::ShutdownRequest { terminate_app },
            self.token.as_deref(),
            Some(session_id),
        )?;
        self.inner
            .clone()
            .shutdown(request)
            .await
            .map_err(ClientError::from_status)?;
        Ok(())
    }
}

/// An open session: `send` one command at a time, watch logs, capture
/// out-of-band screenshots, shut down.
pub struct AgentSession {
    client: ProtoAgent<Channel>,
    session_id: String,
    token: Option<String>,
    cmd_tx: mpsc::Sender<pb::Command>,
    state: Arc<SessionState>,
    demux: JoinHandle<()>,
    watchdog: JoinHandle<()>,
    options: SessionOptions,
    prefix: String,
    seq: AtomicU64,
    /// Id of the command currently awaiting its result (shutdown cancel).
    in_flight: Mutex<Option<String>>,
}

impl AgentSession {
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Whether the session has failed and further commands will be refused.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state.closed_error().is_some()
    }

    /// The failure that closed the session, if any.
    #[must_use]
    pub fn close_reason(&self) -> Option<ClientError> {
        self.state.closed_error()
    }

    /// Send one command and wait for its `CommandResult` (spec 7.2.4).
    ///
    /// The master deadline is `timeout_ms + grace_ms` (spec 7.2.7). On
    /// overrun the client sends `Cancel`, gives the executor `cancel_wait`
    /// (2 s by default), declares `TIMEOUT` and fails the session.
    pub async fn send(
        &self,
        command: &Command,
        timeout_ms: u32,
        grace_ms: u64,
    ) -> Result<CommandOutcome, ClientError> {
        if let Some(error) = self.state.closed_error() {
            return Err(error);
        }
        let id = self.next_id();
        let proto = convert::command_to_proto(command, &id, timeout_ms);
        let (waiter, rx) = oneshot::channel();
        self.state.register(id.clone(), waiter);
        if self.cmd_tx.send(proto).await.is_err() {
            self.state.take_waiter(&id);
            return Err(ClientError::Transport(
                "the session command stream is closed".to_string(),
            ));
        }
        *lock(&self.in_flight) = Some(id.clone());
        let total = Duration::from_millis(u64::from(timeout_ms).saturating_add(grace_ms));
        let deadline = Instant::now() + total;
        let result = match select(Box::pin(rx), Box::pin(tokio::time::sleep_until(deadline))).await
        {
            Either::Left((result, _)) => match result {
                Ok(Ok(outcome)) => Ok(outcome),
                Ok(Err(error)) => Err(error),
                Err(_recv) => Err(self.state.closed_error().unwrap_or_else(|| {
                    ClientError::Transport(
                        "the session ended while awaiting a command result".to_string(),
                    )
                })),
            },
            Either::Right((_elapsed, rx)) => Err(self
                .handle_overrun(id.clone(), timeout_ms, grace_ms, rx)
                .await),
        };
        let mut in_flight = lock(&self.in_flight);
        if in_flight.as_deref() == Some(id.as_str()) {
            *in_flight = None;
        }
        drop(in_flight);
        result
    }

    /// The id of the command whose result is currently awaited, if any
    /// (used by the executor to `Cancel` on shutdown).
    #[must_use]
    pub fn in_flight_command_id(&self) -> Option<String> {
        lock(&self.in_flight).clone()
    }

    /// Cancel one command (spec 7.2.5). Unknown ids yield a `PROTOCOL`
    /// result for the cancel command itself.
    pub async fn cancel(&self, target_command_id: &str) -> Result<CommandOutcome, ClientError> {
        let cancel_ms = u32::try_from(self.options.cancel_wait.as_millis()).unwrap_or(u32::MAX);
        self.send(
            &Command::Cancel {
                target_command_id: target_command_id.to_string(),
            },
            cancel_ms,
            0,
        )
        .await
    }

    /// Out-of-band screenshot RPC (spec 7.2.9): may overlap an in-flight
    /// command and must not disturb it.
    pub async fn capture_screenshot(&self) -> Result<CaptureOutcome, ClientError> {
        let request = signed_request(
            pb::ScreenshotRequest {},
            self.token.as_deref(),
            Some(&self.session_id),
        )?;
        let response = self
            .client
            .clone()
            .capture_screenshot(request)
            .await
            .map_err(ClientError::from_status)?
            .into_inner();
        let error = match &response.error {
            Some(info) => Some(convert::error_info_from_proto(info)?),
            None => None,
        };
        Ok(CaptureOutcome {
            png: response.png,
            width: response.width,
            height: response.height,
            error,
        })
    }

    /// Open `StreamLogs` at `min_level` (spec 7.2.10).
    pub async fn stream_logs(&self, min_level: LogLevel) -> Result<LogStream, ClientError> {
        let request = signed_request(
            pb::LogRequest {
                min_level: convert::log_level_to_proto(min_level) as i32,
            },
            self.token.as_deref(),
            Some(&self.session_id),
        )?;
        let response = self
            .client
            .clone()
            .stream_logs(request)
            .await
            .map_err(ClientError::from_status)?;
        Ok(LogStream::new(response.into_inner()))
    }

    /// Ask the executor to shut down (end of life, spec 7.3 step 6).
    pub async fn shutdown(&self, terminate_app: bool) -> Result<(), ClientError> {
        let request = signed_request(
            pb::ShutdownRequest { terminate_app },
            self.token.as_deref(),
            Some(&self.session_id),
        )?;
        self.client
            .clone()
            .shutdown(request)
            .await
            .map_err(ClientError::from_status)?;
        self.state.fail_session(ClientError::SessionClosed {
            reason: "shutdown requested".to_string(),
            error: None,
        });
        Ok(())
    }

    fn next_id(&self) -> String {
        let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        format!("{}-{n}", self.prefix)
    }

    /// Spec 7.2.7: on overrun send `Cancel`, wait `cancel_wait`, declare
    /// `TIMEOUT`, tear down the session.
    async fn handle_overrun(
        &self,
        command_id: String,
        timeout_ms: u32,
        grace_ms: u64,
        rx: Pin<Box<oneshot::Receiver<Result<CommandOutcome, ClientError>>>>,
    ) -> ClientError {
        tracing::warn!(
            command_id = %command_id,
            "master deadline exceeded; sending Cancel"
        );
        if let Err(error) = self.push_cancel(&command_id).await {
            tracing::warn!(command_id = %command_id, %error, "Cancel could not be sent");
        }
        if let Either::Left((result, _)) =
            select(rx, Box::pin(tokio::time::sleep(self.options.cancel_wait))).await
        {
            match result {
                Ok(Ok(outcome)) => tracing::warn!(
                    command_id = %command_id,
                    ok = outcome.ok,
                    "cancelled command settled within the grace period"
                ),
                Ok(Err(error)) => tracing::warn!(command_id = %command_id, %error),
                Err(_) => {}
            }
        }
        let error = ClientError::Timeout {
            command_id,
            timeout_ms,
            grace_ms,
        };
        self.state.fail_session(error.clone());
        error
    }

    /// Fire-and-forget `Cancel` (used on the overrun path, where awaiting the
    /// cancel's own result would spend the grace budget).
    async fn push_cancel(&self, target: &str) -> Result<(), ClientError> {
        let id = self.next_id();
        let proto = pb::Command {
            command_id: id,
            timeout_ms: 0,
            body: Some(pb::command::Body::Cancel(pb::Cancel {
                target_command_id: target.to_string(),
            })),
        };
        self.cmd_tx
            .send(proto)
            .await
            .map_err(|_| ClientError::Transport("the session command stream is closed".to_string()))
    }
}

impl Drop for AgentSession {
    fn drop(&mut self) {
        self.demux.abort();
        self.watchdog.abort();
    }
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

struct Heartbeat {
    seq: u64,
    at: Instant,
}

struct SessionState {
    waiters: Mutex<HashMap<String, WaiterSender>>,
    last_heartbeat: Mutex<Heartbeat>,
    closed: Mutex<Option<ClientError>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl SessionState {
    fn new() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
            last_heartbeat: Mutex::new(Heartbeat {
                seq: 0,
                at: Instant::now(),
            }),
            closed: Mutex::new(None),
        }
    }

    fn register(&self, id: String, sender: WaiterSender) {
        lock(&self.waiters).insert(id, sender);
    }

    fn take_waiter(&self, id: &str) -> Option<WaiterSender> {
        lock(&self.waiters).remove(id)
    }

    /// Fail every waiter with `error` and remember the first close reason.
    fn fail_session(&self, error: ClientError) {
        {
            let mut closed = lock(&self.closed);
            if closed.is_some() {
                return;
            }
            *closed = Some(error.clone());
        }
        let waiters = {
            let mut waiters = lock(&self.waiters);
            std::mem::take(&mut *waiters)
        };
        for (_, sender) in waiters {
            let _ = sender.send(Err(error.clone()));
        }
    }

    fn closed_error(&self) -> Option<ClientError> {
        lock(&self.closed).clone()
    }

    fn record_heartbeat(&self, seq: u64) {
        let mut heartbeat = lock(&self.last_heartbeat);
        heartbeat.seq = seq;
        heartbeat.at = Instant::now();
    }

    fn heartbeat_age(&self) -> Duration {
        lock(&self.last_heartbeat).at.elapsed()
    }
}

fn spawn_watchdog(state: Arc<SessionState>, options: SessionOptions) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(options.watchdog_poll).await;
            if state.closed_error().is_some() {
                break;
            }
            let age = state.heartbeat_age();
            if age >= options.heartbeat_timeout {
                state.fail_session(ClientError::Transport(format!(
                    "no heartbeat for {} ms (three missed 5 s beats); giving up, no resume (D-12)",
                    age.as_millis()
                )));
                break;
            }
        }
    })
}

async fn demux_events(mut events: Streaming<pb::Event>, state: Arc<SessionState>) {
    loop {
        match events.message().await {
            Ok(Some(event)) => match event.body {
                Some(pb::event::Body::Result(result)) => {
                    match convert::outcome_from_proto(&result) {
                        Ok(outcome) => {
                            if let Some(sender) = state.take_waiter(&outcome.command_id) {
                                let _ = sender.send(Ok(outcome));
                            } else {
                                tracing::debug!(
                                    command_id = %result.command_id,
                                    "result for an unknown command id ignored"
                                );
                            }
                        }
                        Err(error) => {
                            let client_error = ClientError::Protocol(error.message.clone());
                            if let Some(sender) = state.take_waiter(&result.command_id) {
                                let _ = sender.send(Err(client_error));
                            }
                        }
                    }
                }
                Some(pb::event::Body::Heartbeat(heartbeat)) => {
                    state.record_heartbeat(heartbeat.seq);
                }
                Some(pb::event::Body::Closed(closed)) => {
                    let error = match &closed.error {
                        Some(info) => convert::error_info_from_proto(info).ok(),
                        None => None,
                    };
                    state.fail_session(ClientError::SessionClosed {
                        reason: closed.reason.clone(),
                        error,
                    });
                    break;
                }
                None => {
                    tracing::warn!("event without a body ignored (unknown oneof)");
                }
            },
            Ok(None) => {
                state.fail_session(ClientError::Transport(
                    "the session event stream ended".to_string(),
                ));
                break;
            }
            Err(status) => {
                state.fail_session(ClientError::from_status(status));
                break;
            }
        }
    }
}

fn handshake_request(params: &HandshakeParams) -> pb::HandshakeRequest {
    pb::HandshakeRequest {
        protocol_version: params.protocol_version.clone(),
        master_version: params.master_version.clone(),
        device_name: params.device_name.clone(),
        platform: convert::platform_to_proto(params.platform) as i32,
        target_endpoint: params.target_endpoint.clone(),
        app_id: params.app_id.clone().unwrap_or_default(),
        app_url: params.app_url.clone().unwrap_or_default(),
        options: params
            .options
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        auth_token: params.auth_token.clone().unwrap_or_default(),
    }
}

fn handshake_outcome(response: &pb::HandshakeResponse) -> Result<HandshakeOutcome, ClientError> {
    let error = match &response.error {
        Some(info) => Some(convert::error_info_from_proto(info)?),
        None => None,
    };
    let capabilities = response
        .capabilities
        .iter()
        .filter_map(|value| Capability::from_proto(*value))
        .collect();
    Ok(HandshakeOutcome {
        accepted: response.accepted,
        session_id: response.session_id.clone(),
        agent_version: response.agent_version.clone(),
        protocol_version: response.protocol_version.clone(),
        capabilities,
        device: response.device.as_ref().map(DeviceInfo::from_proto),
        error,
    })
}

fn signed_request<T>(
    message: T,
    token: Option<&str>,
    session_id: Option<&str>,
) -> Result<Request<T>, ClientError> {
    let mut request = Request::new(message);
    if let Some(session_id) = session_id {
        insert_metadata(request.metadata_mut(), SESSION_ID_KEY, session_id)?;
    }
    if let Some(token) = token {
        insert_metadata(request.metadata_mut(), TOKEN_KEY, token)?;
    }
    Ok(request)
}

fn insert_metadata(
    map: &mut MetadataMap,
    key: &'static str,
    value: &str,
) -> Result<(), ClientError> {
    let value = MetadataValue::try_from(value)
        .map_err(|_| ClientError::Protocol(format!("metadata '{key}' must be printable ASCII")))?;
    map.insert(MetadataKey::from_static(key), value);
    Ok(())
}

/// Unique command-id prefix for one session (timestamp-based, like run ids).
fn command_prefix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    format!("c{nanos:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_major_parsing() {
        assert_eq!(protocol_major("1.0.0"), Some(1));
        assert_eq!(protocol_major("12.3"), Some(12));
        assert_eq!(protocol_major("nope"), None);
        assert_eq!(protocol_major(""), None);
    }

    #[test]
    fn client_error_codes() {
        assert_eq!(
            ClientError::Transport("x".to_string()).code(),
            WireErrorCode::Transport
        );
        assert_eq!(
            ClientError::Timeout {
                command_id: "c".to_string(),
                timeout_ms: 10,
                grace_ms: 5
            }
            .code(),
            WireErrorCode::Timeout
        );
        let closed = ClientError::SessionClosed {
            reason: "bye".to_string(),
            error: Some(WireError::new(WireErrorCode::AppCrashed, "boom")),
        };
        assert_eq!(closed.code(), WireErrorCode::AppCrashed);
        let rejected =
            ClientError::HandshakeRejected(WireError::new(WireErrorCode::AlreadyInUse, "busy"));
        assert_eq!(rejected.code(), WireErrorCode::AlreadyInUse);
        assert!(rejected.to_wire().message.contains("busy"));
    }

    #[test]
    fn capabilities_from_proto_skips_unknown() {
        assert_eq!(
            Capability::from_proto(pb::Capability::Screenshot as i32),
            Some(Capability::Screenshot)
        );
        assert_eq!(Capability::from_proto(0), None);
        assert_eq!(Capability::from_proto(999), None);
    }

    #[test]
    fn command_prefix_is_unique_per_call() {
        let first = command_prefix();
        let second = command_prefix();
        assert!(first.starts_with('c'));
        assert_ne!(first, second);
    }

    #[test]
    fn session_options_defaults_match_spec() {
        let options = SessionOptions::default();
        assert_eq!(options.heartbeat_timeout, Duration::from_secs(15));
        assert_eq!(options.cancel_wait, Duration::from_secs(2));
    }
}
