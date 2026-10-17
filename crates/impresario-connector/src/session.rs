//! One CDP session (spec 10.1): created by a handshake, owns the backend
//! (and the Chromium process for `launch` endpoints), executes commands from
//! the `Session` stream strictly in order with cancellation by command id,
//! and emits heartbeats, results, logs and `SessionClosed`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{broadcast, mpsc, Notify};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use impresario_core::config::CdpBackend as CdpBackendChoice;
use impresario_core::platforms::Platform;
use impresario_proto::v1 as pb;
use tonic::{Status, Streaming};

use crate::backends::raw::RawCdpBackend;
use crate::backends::{CdpBackend, ConsoleEvent, ConsoleLevel};
use crate::errors::ConnectorError;
use crate::exec::{CommandOutput, ExecOptions, Executor, KeyDispatchMode};
use crate::launch::{launch_chromium, LaunchedBrowser};
use crate::pages::{self, HostPort};

/// Heartbeat cadence of spec 7.3.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// How long page discovery retries for a fresh Chromium.
const LAUNCH_PAGE_BUDGET: Duration = Duration::from_secs(15);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Session configuration parsed from the handshake (spec 4.3 options plus
/// the polling knobs of I-34). Unknown option keys are ignored.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub platform: Platform,
    pub target_endpoint: String,
    pub app_url: Option<String>,
    pub cdp_backend: CdpBackendChoice,
    pub key_dispatch: KeyDispatchMode,
    pub page_match: Option<String>,
    pub poll_interval: Duration,
    pub scroll_max_attempts: u32,
    pub focus_max_steps: u32,
}

impl SessionConfig {
    /// Build from handshake fields; malformed documented options are
    /// `PROTOCOL` errors (they come from the master).
    pub fn from_handshake(
        platform: Platform,
        target_endpoint: &str,
        app_url: Option<String>,
        options: &HashMap<String, String>,
    ) -> Result<Self, ConnectorError> {
        let cdp_backend = match options
            .get("cdp_backend")
            .map(String::as_str)
            .unwrap_or("raw")
        {
            "raw" => CdpBackendChoice::Raw,
            "chromiumoxide" => CdpBackendChoice::Chromiumoxide,
            other => {
                return Err(ConnectorError::protocol(format!(
                    "unknown cdp_backend option '{other}'"
                )))
            }
        };
        let key_dispatch = match options
            .get("key_dispatch")
            .map(String::as_str)
            .unwrap_or("cdp")
        {
            "cdp" => KeyDispatchMode::Cdp,
            "js" => KeyDispatchMode::Js,
            other => {
                return Err(ConnectorError::protocol(format!(
                    "unknown key_dispatch option '{other}'"
                )))
            }
        };
        let poll_interval = option_number(options, "poll_interval_ms", 100, 20, 2_000)?;
        let scroll_max_attempts = option_number(options, "scroll_max_attempts", 10, 1, 100)?;
        let focus_max_steps = option_number(options, "focus_max_steps", 50, 1, 500)?;
        let page_match = options
            .get("page_match")
            .filter(|value| !value.is_empty())
            .cloned();
        Ok(Self {
            platform,
            target_endpoint: target_endpoint.to_string(),
            app_url: app_url.filter(|url| !url.is_empty()),
            cdp_backend,
            key_dispatch,
            page_match,
            poll_interval: Duration::from_millis(poll_interval),
            scroll_max_attempts: scroll_max_attempts as u32,
            focus_max_steps: focus_max_steps as u32,
        })
    }
}

fn option_number(
    options: &HashMap<String, String>,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ConnectorError> {
    match options.get(key) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<u64>()
            .map(|value| value.clamp(min, max))
            .map_err(|_| {
                ConnectorError::protocol(format!("option {key} is not a number: '{raw}'"))
            }),
    }
}

/// How a `target_endpoint` is reached (spec 10.3 page resolution).
#[derive(Debug, Clone, PartialEq, Eq)]
enum EndpointPlan {
    /// `launch` / `launch:headless`: spawn Chromium.
    Launch { headless: bool },
    /// A full `ws(s)://.../devtools/page/<id>` URL.
    DirectPage,
    /// `ws(s)://host:port` (usually a TV): resolve the page via `/json/list`.
    Resolve { host: String, port: u16 },
}

fn classify_endpoint(endpoint: &str) -> Result<EndpointPlan, ConnectorError> {
    match endpoint {
        "launch" => return Ok(EndpointPlan::Launch { headless: false }),
        "launch:headless" => return Ok(EndpointPlan::Launch { headless: true }),
        _ => {}
    }
    let url = url::Url::parse(endpoint).map_err(|error| {
        ConnectorError::protocol(format!(
            "target_endpoint '{endpoint}' is not a ws:// URL: {error}"
        ))
    })?;
    if url.scheme() != "ws" && url.scheme() != "wss" {
        return Err(ConnectorError::protocol(format!(
            "target_endpoint '{endpoint}' must be ws:// or wss:// (or launch[:headless])"
        )));
    }
    if pages::has_page_path(endpoint) {
        return Ok(EndpointPlan::DirectPage);
    }
    let host = url
        .host_str()
        .ok_or_else(|| ConnectorError::protocol(format!("'{endpoint}' has no host")))?
        .to_string();
    let port = url.port().ok_or_else(|| {
        ConnectorError::protocol(format!(
            "'{endpoint}' needs an explicit port for page discovery"
        ))
    })?;
    Ok(EndpointPlan::Resolve { host, port })
}

/// The page a session drives, plus the browser-level endpoint the
/// `chromiumoxide` backend needs (spec 10.3 page resolution).
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    /// Page-level websocket (`ws(s)://host:port/devtools/page/<id>`).
    pub page_ws_url: String,
    /// The page's target id, when the endpoint exposed one.
    pub page_id: Option<String>,
    /// Browser-level websocket (`/json/version`), when discoverable.
    pub browser_ws_url: Option<String>,
}

/// Resolve the endpoint into a page websocket URL, launching Chromium when
/// asked to.
async fn resolve_target(
    config: &SessionConfig,
) -> Result<(ResolvedTarget, Option<LaunchedBrowser>), ConnectorError> {
    match classify_endpoint(&config.target_endpoint)? {
        EndpointPlan::Launch { headless } => {
            let browser = launch_chromium(headless, &[]).await?;
            let endpoint = HostPort {
                host: browser.host.clone(),
                port: browser.port,
            };
            let browser_ws_url = browser.ws_url.clone();
            match resolve_page_ws(&endpoint, config.page_match.as_deref(), LAUNCH_PAGE_BUDGET).await
            {
                Ok(page_ws_url) => Ok((
                    ResolvedTarget {
                        page_id: pages::page_id_from_ws(&page_ws_url),
                        page_ws_url,
                        browser_ws_url: Some(browser_ws_url),
                    },
                    Some(browser),
                )),
                Err(error) => {
                    let mut browser = browser;
                    browser.kill().await;
                    Err(error)
                }
            }
        }
        EndpointPlan::DirectPage => {
            let page_ws_url = config.target_endpoint.clone();
            let browser_ws_url = browser_ws_from_page_url(&page_ws_url).await;
            Ok((
                ResolvedTarget {
                    page_id: pages::page_id_from_ws(&page_ws_url),
                    page_ws_url,
                    browser_ws_url,
                },
                None,
            ))
        }
        EndpointPlan::Resolve { host, port } => {
            let endpoint = HostPort { host, port };
            let page_ws_url = resolve_page_ws(
                &endpoint,
                config.page_match.as_deref(),
                Duration::from_secs(5),
            )
            .await?;
            let browser_ws_url = pages::fetch_browser_ws(&endpoint).await.ok();
            Ok((
                ResolvedTarget {
                    page_id: pages::page_id_from_ws(&page_ws_url),
                    page_ws_url,
                    browser_ws_url,
                },
                None,
            ))
        }
    }
}

/// Best-effort browser-level websocket for an explicit page URL (the
/// `chromiumoxide` backend connects at browser level).
async fn browser_ws_from_page_url(page_ws_url: &str) -> Option<String> {
    let url = url::Url::parse(page_ws_url).ok()?;
    let host = url.host_str()?.to_string();
    let port = url.port()?;
    let endpoint = HostPort { host, port };
    pages::fetch_browser_ws(&endpoint).await.ok()
}

async fn resolve_page_ws(
    endpoint: &HostPort,
    page_match: Option<&str>,
    budget: Duration,
) -> Result<String, ConnectorError> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let outcome = match pages::list_pages(endpoint).await {
            Ok(list) => match pages::select_page(&list, page_match) {
                Ok(page) => pages::page_ws_url(&page),
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };
        match outcome {
            Ok(ws) => return Ok(pages::rewrite_ws_host(&ws, endpoint)),
            Err(error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
                tracing::debug!(%error, attempt, "page resolution not ready yet");
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn build_backend(
    config: &SessionConfig,
    target: &ResolvedTarget,
) -> Result<Arc<dyn CdpBackend>, ConnectorError> {
    match config.cdp_backend {
        CdpBackendChoice::Raw => Ok(Arc::new(
            RawCdpBackend::connect(&target.page_ws_url)
                .await
                .map_err(ConnectorError::from)?,
        )),
        CdpBackendChoice::Chromiumoxide => {
            #[cfg(feature = "chromiumoxide-backend")]
            {
                let browser_ws = target.browser_ws_url.as_deref().ok_or_else(|| {
                    ConnectorError::device_unavailable(format!(
                        "the endpoint {} exposes no browser-level websocket (/json/version); \
                         cdp_backend=chromiumoxide needs one — use cdp_backend=raw here",
                        config.target_endpoint
                    ))
                })?;
                let backend = crate::backends::chromiumoxide::ChromiumoxideBackend::connect(
                    browser_ws,
                    target.page_id.as_deref(),
                    config.page_match.as_deref(),
                )
                .await
                .map_err(ConnectorError::from)?;
                Ok(Arc::new(backend))
            }
            #[cfg(not(feature = "chromiumoxide-backend"))]
            {
                let _ = (config, target);
                Err(ConnectorError::unsupported_action(
                    "cdp_backend=chromiumoxide is not compiled in; rebuild with --features chromiumoxide-backend",
                ))
            }
        }
    }
}

/// A live session.
pub struct Session {
    pub id: String,
    pub platform: Platform,
    pub device: pb::DeviceInfo,
    executor: Executor,
    console: broadcast::Sender<ConsoleEvent>,
    browser: Mutex<Option<LaunchedBrowser>>,
    console_pump: Mutex<Option<JoinHandle<()>>>,
    pub shutdown: CancellationToken,
    torn_down: AtomicBool,
}

impl Session {
    /// Resolve the endpoint, connect the backend and verify the page answers.
    pub async fn connect(config: SessionConfig, id: String) -> Result<Self, ConnectorError> {
        let (target, browser) = resolve_target(&config).await?;
        let mut browser = browser;
        let backend = match build_backend(&config, &target).await {
            Ok(backend) => backend,
            Err(error) => {
                if let Some(browser) = browser.as_mut() {
                    browser.kill().await;
                }
                return Err(error);
            }
        };

        let options = ExecOptions {
            poll_interval: config.poll_interval,
            scroll_max_attempts: config.scroll_max_attempts,
            focus_max_steps: config.focus_max_steps,
            key_dispatch: config.key_dispatch,
            app_url: config.app_url.clone(),
        };
        let executor = Executor::new(config.platform, backend.clone(), options);
        let bootstrap = CancellationToken::new();
        if let Err(error) = executor.inject_helper(&bootstrap).await {
            backend.close().await;
            if let Some(browser) = browser.as_mut() {
                browser.kill().await;
            }
            return Err(error);
        }
        let device = match executor.device_info(&bootstrap).await {
            Ok(device) => device,
            Err(error) => {
                let error = ConnectorError::device_unavailable(format!(
                    "the page did not answer the handshake probe on {}: {error}",
                    target.page_ws_url
                ));
                backend.close().await;
                if let Some(browser) = browser.as_mut() {
                    browser.kill().await;
                }
                return Err(error);
            }
        };

        let (console, _) = broadcast::channel(512);
        let console_pump = spawn_console_pump(backend.clone(), console.clone());
        Ok(Self {
            id,
            platform: config.platform,
            device,
            executor,
            console,
            browser: Mutex::new(browser),
            console_pump: Mutex::new(Some(console_pump)),
            shutdown: CancellationToken::new(),
            torn_down: AtomicBool::new(false),
        })
    }

    /// New console listeners (spec 10.1 `StreamLogs`).
    #[must_use]
    pub fn console_subscribe(&self) -> broadcast::Receiver<ConsoleEvent> {
        self.console.subscribe()
    }

    /// Execute one command body (the session loop has already handled
    /// `Cancel`).
    pub async fn execute(
        &self,
        body: &pb::command::Body,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        // Spec 10.3: the helper is injected idempotently before every command
        // (a reload or navigation in a previous command drops it).
        self.executor.inject_helper(cancel).await?;
        match body {
            pb::command::Body::RunAction(action) => {
                self.executor.run_action(action, timeout_ms, cancel).await
            }
            pb::command::Body::ValidateCheck(check) => {
                self.executor
                    .validate_check(check, timeout_ms, cancel)
                    .await
            }
            pb::command::Body::KeyPress(press) => {
                self.executor.key_press(press, cancel).await?;
                Ok(CommandOutput::default())
            }
            pb::command::Body::Screenshot(_) => self.executor.screenshot(cancel).await,
            pb::command::Body::AppControl(control) => {
                let op = pb::AppOp::try_from(control.op).map_err(|_| {
                    ConnectorError::protocol(format!("unknown app op {}", control.op))
                })?;
                self.executor.app_control(op, cancel).await?;
                Ok(CommandOutput::default())
            }
            pb::command::Body::Cancel(_) => Err(ConnectorError::protocol(
                "a cancel command is handled by the session loop",
            )),
        }
    }

    /// Full `CommandResult` for one command (duration measured here).
    pub async fn execute_command(
        &self,
        command: &pb::Command,
        cancel: &CancellationToken,
    ) -> pb::CommandResult {
        let started = tokio::time::Instant::now();
        let outcome = match command.body.as_ref() {
            Some(body) => self.execute(body, command.timeout_ms, cancel).await,
            None => Err(ConnectorError::protocol("the command has no body")),
        };
        let duration_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
        match outcome {
            Ok(output) => pb::CommandResult {
                command_id: command.command_id.clone(),
                ok: true,
                duration_ms,
                error: None,
                screenshot_png: output.screenshot_png,
                observed_text: output.observed_text,
                observed_count: output.observed_count,
            },
            Err(error) => pb::CommandResult {
                command_id: command.command_id.clone(),
                ok: false,
                duration_ms,
                error: Some(error.to_proto()),
                screenshot_png: Vec::new(),
                observed_text: String::new(),
                observed_count: 0,
            },
        }
    }

    /// Out-of-band screenshot (spec 7.1): independent of the command loop.
    pub async fn capture_out_of_band(&self) -> Result<(Vec<u8>, u32, u32), ConnectorError> {
        let token = CancellationToken::new();
        let output = self.executor.screenshot(&token).await?;
        let (width, height) = crate::png::png_dimensions(&output.screenshot_png).unwrap_or((0, 0));
        Ok((output.screenshot_png, width, height))
    }

    /// Kill the browser, close the backend, stop pumps (idempotent).
    pub async fn teardown(&self) {
        if self.torn_down.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shutdown.cancel();
        self.executor.backend().close().await;
        let pump = {
            let mut guard = lock(&self.console_pump);
            guard.take()
        };
        if let Some(pump) = pump {
            pump.abort();
        }
        let browser = {
            let mut guard = lock(&self.browser);
            guard.take()
        };
        if let Some(mut browser) = browser {
            browser.kill().await;
        }
    }
}

fn spawn_console_pump(
    backend: Arc<dyn CdpBackend>,
    tx: broadcast::Sender<ConsoleEvent>,
) -> JoinHandle<()> {
    let mut stream = backend.console_events();
    tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            let _ = tx.send(event);
        }
    })
}

/// A session plus its stream bookkeeping (spec 10.1 `SessionManager` entry).
pub struct SessionHandle {
    pub session: Arc<Session>,
    pub stream_taken: AtomicBool,
    pub closed: AtomicBool,
    pub shutdown: CancellationToken,
    pub done: Arc<Notify>,
}

impl SessionHandle {
    #[must_use]
    pub fn new(session: Arc<Session>) -> Self {
        let shutdown = session.shutdown.clone();
        Self {
            session,
            stream_taken: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            shutdown,
            done: Arc::new(Notify::new()),
        }
    }

    /// Wait for the command loop to stop (bounded by the caller).
    pub async fn wait_closed(&self, timeout: Duration) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let _ = tokio::time::timeout(timeout, self.done.notified()).await;
    }
}

/// Drive one `Session` stream: strictly ordered commands, cancel by id,
/// heartbeats (spec 7.2-7.3). The returned stream ends with `SessionClosed`.
pub fn run_stream(
    handle: Arc<SessionHandle>,
    inbound: Streaming<pb::Command>,
) -> ReceiverStream<Result<pb::Event, Status>> {
    let (event_tx, event_rx) = mpsc::channel::<Result<pb::Event, Status>>(128);
    let (command_tx, command_rx) = mpsc::channel::<pb::Command>(128);
    let current = Arc::new(Mutex::new(String::new()));

    tokio::spawn(async move {
        let mut inbound = inbound;
        while let Ok(Some(command)) = inbound.message().await {
            if command_tx.send(command).await.is_err() {
                break;
            }
        }
    });

    {
        let events = event_tx.clone();
        let shutdown = handle.shutdown.clone();
        let current = current.clone();
        tokio::spawn(async move {
            let mut seq = 0u64;
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {}
                }
                seq += 1;
                let heartbeat = pb::Heartbeat {
                    seq,
                    unix_ms: unix_ms_now(),
                    current_command_id: lock(&current).clone(),
                };
                if send_event(&events, pb::event::Body::Heartbeat(heartbeat))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    tokio::spawn(session_loop(handle, command_rx, event_tx, current));
    ReceiverStream::new(event_rx)
}

async fn send_event(
    sender: &mpsc::Sender<Result<pb::Event, Status>>,
    body: pb::event::Body,
) -> Result<(), ()> {
    sender
        .send(Ok(pb::Event { body: Some(body) }))
        .await
        .map_err(|_| ())
}

async fn session_loop(
    handle: Arc<SessionHandle>,
    mut commands: mpsc::Receiver<pb::Command>,
    events: mpsc::Sender<Result<pb::Event, Status>>,
    current: Arc<Mutex<String>>,
) {
    let session = handle.session.clone();
    let shutdown = handle.shutdown.clone();
    let mut pending: VecDeque<pb::Command> = VecDeque::new();
    let mut recently_cancelled: Vec<String> = Vec::new();

    loop {
        let next = if let Some(command) = pending.pop_front() {
            Some(command)
        } else {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => None,
                command = commands.recv() => command,
            }
        };
        let Some(command) = next else { break };

        // Cancel of the in-flight command is applied immediately; the cancel
        // command's own result is answered later (spec 7.2.5, I-20).
        if let Some(pb::command::Body::Cancel(cancel)) = command.body.as_ref() {
            let target = cancel.target_command_id.clone();
            let in_flight = !target.is_empty() && *lock(&current) == target;
            let was_cancelled = in_flight || {
                if let Some(index) = recently_cancelled.iter().position(|id| *id == target) {
                    recently_cancelled.remove(index);
                    true
                } else {
                    false
                }
            };
            let result = if was_cancelled {
                ok_result(&command.command_id)
            } else {
                pb::CommandResult {
                    command_id: command.command_id.clone(),
                    ok: false,
                    duration_ms: 0,
                    error: Some(
                        ConnectorError::protocol(format!(
                            "no in-flight command '{target}' to cancel"
                        ))
                        .to_proto(),
                    ),
                    screenshot_png: Vec::new(),
                    observed_text: String::new(),
                    observed_count: 0,
                }
            };
            if send_event(&events, pb::event::Body::Result(result))
                .await
                .is_err()
            {
                break;
            }
            continue;
        }

        // Child of the session token: `Shutdown` also stops an in-flight
        // command (spec 7.2.5).
        let token = shutdown.child_token();
        *lock(&current) = command.command_id.clone();
        let execution = session.execute_command(&command, &token);
        tokio::pin!(execution);
        let mut stream_closed = false;
        let result = loop {
            if stream_closed {
                break execution.await;
            }
            tokio::select! {
                biased;
                finished = &mut execution => break finished,
                next = commands.recv() => {
                    match next {
                        Some(next) => {
                            if let Some(pb::command::Body::Cancel(cancel)) = next.body.as_ref() {
                                if cancel.target_command_id == command.command_id {
                                    recently_cancelled.push(command.command_id.clone());
                                    token.cancel();
                                }
                            }
                            pending.push_back(next);
                        }
                        None => {
                            // The client closed the stream: stop the command.
                            token.cancel();
                            stream_closed = true;
                        }
                    }
                }
            }
        };
        *lock(&current) = String::new();
        if send_event(&events, pb::event::Body::Result(result))
            .await
            .is_err()
        {
            break;
        }
    }

    let reason = if shutdown.is_cancelled() {
        "shutdown"
    } else {
        "session stream ended"
    };
    let _ = send_event(
        &events,
        pb::event::Body::Closed(pb::SessionClosed {
            reason: reason.to_string(),
            error: None,
        }),
    )
    .await;
    session.teardown().await;
    handle.closed.store(true, Ordering::SeqCst);
    handle.done.notify_waiters();
    handle.done.notify_one();
}

fn ok_result(command_id: &str) -> pb::CommandResult {
    pb::CommandResult {
        command_id: command_id.to_string(),
        ok: true,
        duration_ms: 0,
        error: None,
        screenshot_png: Vec::new(),
        observed_text: String::new(),
        observed_count: 0,
    }
}

/// Session-level log filtering helper (console level ranks, spec 7.1).
#[must_use]
pub fn console_level_rank(level: ConsoleLevel) -> u8 {
    match level {
        ConsoleLevel::Debug => 1,
        ConsoleLevel::Info => 2,
        ConsoleLevel::Warn => 3,
        ConsoleLevel::Error => 4,
    }
}

/// Map a request's `min_level` onto a console rank (unknown values accept
/// everything: log reading must never fail a session).
#[must_use]
pub fn min_level_rank(min_level: i32) -> u8 {
    match pb::LogLevel::try_from(min_level) {
        Ok(pb::LogLevel::Error) => 4,
        Ok(pb::LogLevel::Warn) => 3,
        Ok(pb::LogLevel::Info) => 2,
        Ok(pb::LogLevel::Debug) => 1,
        Ok(pb::LogLevel::Unspecified) | Err(_) => 1,
    }
}

/// Map a console event onto a wire `LogEntry`.
#[must_use]
pub fn console_to_log_entry(event: &ConsoleEvent) -> pb::LogEntry {
    let level = match event.level {
        ConsoleLevel::Debug => pb::LogLevel::Debug,
        ConsoleLevel::Info => pb::LogLevel::Info,
        ConsoleLevel::Warn => pb::LogLevel::Warn,
        ConsoleLevel::Error => pb::LogLevel::Error,
    };
    pb::LogEntry {
        unix_ms: unix_ms_now() as i64,
        level: level as i32,
        source: event.source.clone(),
        message: event.message.clone(),
    }
}
