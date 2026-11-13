//! `impresario inspector`: the standalone inspector Web UI and its device
//! attachment loop (TASK-INSPECTOR sections 10-14).
//!
//! The process is deliberately independent of `impresario web` and of a
//! resident master. It attaches to one configured device through the existing
//! transport + `Agent` handshake, then polls the additive `Inspector` service.
//! A small HTTP/1.1 server (same hand-written transport as the web console)
//! serves the embedded SPA, JSON endpoints and an SSE stream. MCP tools call
//! the same loopback API.
//!
//! Nothing here changes the run path: the inspector uses its own session and
//! never opens the bidirectional `Agent.Session` stream.

mod api;
mod assets;
mod convert;
mod scene;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use impresario_clients::agent_client::{AgentClient, Capability, HandshakeParams};
use impresario_clients::inspector_client::InspectorClient;
use impresario_clients::transport::{transport_for, DeviceTransport};
use impresario_core::apps::{self, AppConfig};
use impresario_core::config::{self, DeviceConfig, ImpresarioConfig};
use impresario_core::home::Home;
use impresario_core::platforms::Platform;
use impresario_inspector_core::compact::{compact, CompactOptions};
use impresario_inspector_core::diff::{apply_plan, plan_update, UpdatePlan};
use impresario_inspector_core::prune::{prune_snapshot, PruningOptions};
use impresario_inspector_core::snapshot::{ScreenInfo, ScrollInfo, Snapshot, SnapshotNode};
use impresario_locator::{Rect, Role};
use impresario_proto::v1 as pb;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use scene::{AuditEntry, StoredEdit, UndoRecord};

/// Default inspector port (TASK-INSPECTOR section 10.1).
pub const DEFAULT_PORT: u16 = 7311;
/// Default bind address.
pub const DEFAULT_LISTEN: &str = "127.0.0.1";
const MAX_CONNECTIONS: usize = 64;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(20);
/// The SPA needs CSSOM positioning for its virtualized tree and overlay.
const INSPECTOR_CSP: &str = "default-src 'none'; script-src 'self'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
     connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// `impresario inspector [status]`.
#[derive(Debug, Args)]
pub struct InspectorArgs {
    #[command(subcommand)]
    pub command: Option<InspectorCommand>,
    /// Device name to attach at startup (default: the first configured device).
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,
    /// App name from impresario.apps.config to launch/attach.
    #[arg(long, value_name = "NAME")]
    pub app: Option<String>,
    /// Port to listen on (0 = any free port, printed on start).
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,
    /// Bind address (loopback by default).
    #[arg(long, value_name = "ADDR")]
    pub listen: Option<String>,
    /// Bearer token; required for a non-loopback bind.
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
    /// Open the UI in the default browser after binding.
    #[arg(long)]
    pub open: bool,
}

/// Inspector subcommands.
#[derive(Debug, Subcommand)]
pub enum InspectorCommand {
    /// Show the running inspector's URL, device and revision.
    Status {
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run the hardware-free core pipeline benchmark (mock snapshots).
    ///
    /// Measures pruning, diffing, delta application, compact rendering and
    /// JSON serialization for the requested scenario. Real-device timings
    /// remain a manual run; this command makes the mock path reproducible in
    /// CI and records payload sizes.
    Bench {
        /// Number of samples.
        #[arg(long, default_value_t = 200)]
        samples: usize,
        /// Number of synthetic nodes before pruning.
        #[arg(long, default_value_t = 1200)]
        nodes: usize,
        /// Scenario: scroll, navigate or modal.
        #[arg(long, default_value = "scroll")]
        scenario: String,
        /// Device name label for the report (no device is attached).
        #[arg(long, value_name = "NAME")]
        device: Option<String>,
        /// Machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

/// One structured HTTP/MCP error.
#[derive(Debug)]
pub(crate) struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

impl ApiError {
    pub(crate) fn new(
        status: u16,
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            details,
        }
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, "BAD_REQUEST", message, None)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, "NOT_FOUND", message, None)
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(500, "INTERNAL", message, None)
    }

    pub(crate) fn unauthorized() -> Self {
        Self::new(
            401,
            "UNAUTHORIZED",
            "a bearer token is required for this inspector",
            None,
        )
    }

    pub(crate) fn to_value(&self) -> Value {
        json!({
            "ok": false,
            "error": {
                "code": self.code,
                "message": self.message,
                "details": self.details,
            }
        })
    }
}

/// Live attachment metadata returned by `/api/status`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AttachmentStatus {
    pub attached: bool,
    pub device: Option<String>,
    pub platform: Option<String>,
    pub session_id: Option<String>,
    pub rev: u64,
    pub settled: bool,
    pub acquisition_ms: u64,
    pub nodes: usize,
    pub truncated: bool,
    pub error: Option<String>,
    pub frozen: bool,
}

/// Decide the master-owned revision for a new capture.
///
/// `None` means the capture renders identically to the last published content
/// and must not wake SSE subscribers. Otherwise the revision is strictly
/// greater than `last_rev` and never zero, so it stays monotonic across
/// executor reconnects and reattach.
fn next_publish_rev(
    previous: Option<&Snapshot>,
    last_rev: u64,
    status_rev: u64,
    candidate: &Snapshot,
) -> Option<u64> {
    if let Some(prev) = previous {
        if prev.rev == last_rev && status_rev == prev.rev && prev.same_content(candidate) {
            return None;
        }
    }
    Some(last_rev.saturating_add(1).max(1))
}

/// Shared state for the HTTP threads and the device actor.
pub(crate) struct Shared {
    pub home: Home,
    pub base: PathBuf,
    pub config: ImpresarioConfig,
    pub devices: Vec<DeviceConfig>,
    pub apps: Vec<AppConfig>,
    pub app_name: Option<String>,
    pub token: Option<String>,
    pub bind: String,
    pub port: u16,
    pub loopback: bool,
    pub url: String,
    pub started_at: String,
    pub started: Instant,
    pub latest: RwLock<Option<Arc<Snapshot>>>,
    pub status: RwLock<AttachmentStatus>,
    pub subscribers: Mutex<Vec<std::sync::mpsc::Sender<String>>>,
    pub changed: Mutex<u64>,
    pub changed_cv: Condvar,
    pub commands: mpsc::UnboundedSender<DeviceCommand>,
    pub handle: tokio::runtime::Handle,
    pub edits: Mutex<std::collections::HashMap<String, StoredEdit>>,
    pub undo: Mutex<std::collections::HashMap<PathBuf, UndoRecord>>,
    pub audit: Mutex<Vec<AuditEntry>>,
    pub frozen: AtomicBool,
    /// Last master-owned revision. It survives detach/reattach and only
    /// advances when the published content actually changes.
    pub published_rev: AtomicU64,
    pub connections: AtomicUsize,
}

impl Shared {
    #[allow(clippy::too_many_arguments)]
    fn new(
        home: Home,
        base: PathBuf,
        config: ImpresarioConfig,
        devices: Vec<DeviceConfig>,
        apps: Vec<AppConfig>,
        app_name: Option<String>,
        token: Option<String>,
        bind: String,
        port: u16,
        loopback: bool,
        url: String,
        commands: mpsc::UnboundedSender<DeviceCommand>,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            home,
            base,
            config,
            devices,
            apps,
            app_name,
            token,
            bind,
            port,
            loopback,
            url,
            started_at: crate::time_fmt::rfc3339_utc(crate::time_fmt::now_millis()),
            started: Instant::now(),
            latest: RwLock::new(None),
            status: RwLock::new(AttachmentStatus::default()),
            subscribers: Mutex::new(Vec::new()),
            changed: Mutex::new(0),
            changed_cv: Condvar::new(),
            commands,
            handle,
            edits: Mutex::new(std::collections::HashMap::new()),
            undo: Mutex::new(std::collections::HashMap::new()),
            audit: Mutex::new(Vec::new()),
            frozen: AtomicBool::new(false),
            published_rev: AtomicU64::new(0),
            connections: AtomicUsize::new(0),
        }
    }

    /// The latest snapshot, if one was acquired.
    pub(crate) fn latest_snapshot(&self) -> Option<Arc<Snapshot>> {
        self.latest
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Current attachment status with the frozen flag applied.
    pub(crate) fn status(&self) -> AttachmentStatus {
        let mut status = self
            .status
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        status.frozen = self.frozen.load(Ordering::Relaxed);
        status
    }

    /// Register one SSE subscriber.
    pub(crate) fn subscribe(&self) -> std::sync::mpsc::Receiver<String> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        subscribers.push(sender);
        receiver
    }

    /// Block until the revision advances past `since`, or the timeout elapses.
    pub(crate) fn wait_for_rev(&self, since: u64, timeout: Duration) -> u64 {
        let mut rev = self
            .changed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if *rev <= since {
            let (next, _) = self
                .changed_cv
                .wait_timeout_while(rev, timeout, |current| *current <= since)
                .unwrap_or_else(|poison| poison.into_inner());
            rev = next;
        }
        *rev
    }

    /// Publish a snapshot: choose a full/delta update, store it and fan out.
    ///
    /// The executor owns its own wire revision and may increment it on every
    /// poll. The master owns the published revision: it only advances when the
    /// normalized content changes, so a static screen keeps one revision and
    /// idle SSE clients see no traffic (TASK-INSPECTOR 6.5).
    pub(crate) fn publish(&self, mut snapshot: Snapshot) {
        let previous = self.latest_snapshot();
        let last_rev = self.published_rev.load(Ordering::Acquire);
        let current = self.status();
        let Some(rev) = next_publish_rev(previous.as_deref(), last_rev, current.rev, &snapshot)
        else {
            // Identical content: refresh the counters shown in the footer
            // without advancing the revision or waking subscribers.
            if let Some(prev) = previous.as_deref() {
                self.refresh_status(prev, &snapshot);
            }
            return;
        };
        snapshot.rev = rev;
        self.published_rev.store(rev, Ordering::Release);
        let plan = match previous.as_deref() {
            Some(prev) if prev.rev != snapshot.rev => plan_update(prev, &snapshot),
            _ => UpdatePlan::Full(snapshot.clone()),
        };
        let status = self.status_with_snapshot(&snapshot);
        let screen = snapshot.screen.clone();
        let degraded = snapshot.degraded.clone();
        let settled = snapshot.settled;
        let event = match &plan {
            UpdatePlan::Full(full) => json!({
                "kind": "full",
                "snapshot": full,
                "status": status,
            }),
            UpdatePlan::Delta(delta) => json!({
                "kind": "delta",
                "delta": delta,
                // A delta only carries nodes; screen rotation, degraded
                // reasons and settled state still need to reach the client.
                "screen": screen,
                "degraded": degraded,
                "settled": settled,
                "status": status,
            }),
        };
        let text = event.to_string();
        {
            let mut latest = self
                .latest
                .write()
                .unwrap_or_else(|poison| poison.into_inner());
            *latest = Some(Arc::new(snapshot.clone()));
        }
        {
            let mut current = self
                .status
                .write()
                .unwrap_or_else(|poison| poison.into_inner());
            *current = status;
        }
        {
            let mut subscribers = self
                .subscribers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            subscribers.retain(|sender| sender.send(text.clone()).is_ok());
        }
        {
            let mut rev = self
                .changed
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            *rev = snapshot.rev;
        }
        self.changed_cv.notify_all();
    }

    /// Refresh per-poll counters without publishing a revision.
    fn refresh_status(&self, previous: &Snapshot, next: &Snapshot) {
        let mut status = self
            .status
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        status.rev = previous.rev;
        status.settled = next.settled;
        status.acquisition_ms = next.stats.acquisition_ms;
        status.nodes = next.nodes.len();
        status.truncated = next.degraded.iter().any(|entry| entry.partial);
        status.error = None;
    }

    fn status_with_snapshot(&self, snapshot: &Snapshot) -> AttachmentStatus {
        let mut status = self.status();
        status.rev = snapshot.rev;
        status.settled = snapshot.settled;
        status.acquisition_ms = snapshot.stats.acquisition_ms;
        status.nodes = snapshot.nodes.len();
        status.truncated = snapshot.degraded.iter().any(|entry| entry.partial);
        status.error = None;
        status
    }

    /// Replace the attachment status and wake SSE clients.
    pub(crate) fn set_status(&self, status: AttachmentStatus) {
        let rev = status.rev;
        {
            let mut current = self
                .status
                .write()
                .unwrap_or_else(|poison| poison.into_inner());
            *current = status.clone();
        }
        {
            let mut current = self
                .changed
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            *current = rev;
        }
        {
            let event = json!({ "kind": "status", "status": status }).to_string();
            let mut subscribers = self
                .subscribers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            subscribers.retain(|sender| sender.send(event.clone()).is_ok());
        }
        self.changed_cv.notify_all();
    }

    /// Send a command to the device actor and block for its reply.
    pub(crate) fn command<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, String>>) -> DeviceCommand,
    ) -> Result<T, String> {
        let (sender, receiver) = oneshot::channel();
        self.commands
            .send(build(sender))
            .map_err(|_| "the inspector device loop is not running".to_string())?;
        self.handle
            .block_on(receiver)
            .map_err(|_| "the inspector device loop dropped the request".to_string())?
    }
}

/// Commands handled by the device actor.
pub(crate) enum DeviceCommand {
    Attach {
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Detach {
        reply: oneshot::Sender<Result<(), String>>,
    },
    Input {
        request: pb::InspectorInputRequest,
        reply: oneshot::Sender<Result<pb::InspectorInputResponse, String>>,
    },
    Screenshot {
        reply: oneshot::Sender<Result<pb::InspectorSnapshotResponse, String>>,
    },
    Stop,
}

struct Attachment {
    device: DeviceConfig,
    transport: Box<dyn DeviceTransport>,
    agent: AgentClient,
    session_id: String,
    inspector: InspectorClient,
}

struct Actor {
    shared: Arc<Shared>,
    attachment: Option<Attachment>,
}

impl Actor {
    async fn attach(&mut self, name: &str) -> Result<(), String> {
        if self
            .attachment
            .as_ref()
            .is_some_and(|attachment| attachment.device.name == name)
        {
            return Ok(());
        }
        self.detach().await;
        let device = self
            .shared
            .devices
            .iter()
            .find(|device| device.name == name)
            .cloned()
            .ok_or_else(|| format!("unknown device '{name}'"))?;
        let app = self
            .shared
            .app_name
            .as_ref()
            .and_then(|app_name| self.shared.apps.iter().find(|app| &app.name == app_name));
        let attachment = connect(&self.shared, device, app).await?;
        self.shared.set_status(AttachmentStatus {
            attached: true,
            device: Some(attachment.device.name.clone()),
            platform: Some(attachment.device.platform.as_str().to_string()),
            session_id: Some(attachment.session_id.clone()),
            ..AttachmentStatus::default()
        });
        self.attachment = Some(attachment);
        if let Err(error) = self.poll(false).await {
            let mut status = self.shared.status();
            status.error = Some(error);
            self.shared.set_status(status);
        }
        Ok(())
    }

    async fn detach(&mut self) {
        let Some(attachment) = self.attachment.take() else {
            return;
        };
        let Attachment {
            mut transport,
            agent,
            session_id,
            ..
        } = attachment;
        let _ = agent.shutdown_session(&session_id, false).await;
        let _ = transport.teardown().await;
        // A detached inspector has no current screen: the next attach must
        // start from a full snapshot, and /api/snapshot must not serve the
        // previous device's tree. The published revision itself stays
        // monotonic across reattach.
        *self
            .shared
            .latest
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
        self.shared.set_status(AttachmentStatus {
            attached: false,
            error: None,
            ..AttachmentStatus::default()
        });
    }

    fn connection(&self) -> Result<(InspectorClient, String, DeviceConfig), String> {
        let Some(attachment) = self.attachment.as_ref() else {
            return Err("no device is attached".to_string());
        };
        Ok((
            attachment.inspector.clone(),
            attachment.session_id.clone(),
            attachment.device.clone(),
        ))
    }

    async fn poll(&mut self, include_screenshot: bool) -> Result<(), String> {
        let (inspector, session_id, device) = self.connection()?;
        let request = pb::InspectorSnapshotRequest {
            include_system_ui: self.shared.config.inspector.include_system_ui,
            min_visible_fraction: self.shared.config.inspector.min_visible_fraction(),
            max_nodes: 0,
            max_depth: 0,
            include_raw: false,
            include_screenshot,
            screenshot_only: false,
            screenshot_scale: u32::from(self.shared.config.inspector.screenshot_scale),
        };
        let response =
            tokio::time::timeout(SNAPSHOT_TIMEOUT, inspector.snapshot(&session_id, request))
                .await
                .map_err(|_| "snapshot acquisition timed out".to_string())?
                .map_err(|error| error.to_string())?;
        let snapshot = convert::snapshot_from_proto(&response, &device);
        self.shared.publish(snapshot);
        Ok(())
    }

    async fn input(
        &mut self,
        request: pb::InspectorInputRequest,
    ) -> Result<pb::InspectorInputResponse, String> {
        let (inspector, session_id, _) = self.connection()?;
        let response =
            tokio::time::timeout(SNAPSHOT_TIMEOUT, inspector.input(&session_id, request))
                .await
                .map_err(|_| "input action timed out".to_string())?
                .map_err(|error| error.to_string())?;
        if let Err(error) = self.poll(false).await {
            tracing::debug!(%error, "post-input snapshot failed");
        }
        Ok(response)
    }

    /// Capture one frame without acquiring a tree. The Web UI polls the
    /// screenshot independently; avoiding a full traversal keeps the device
    /// actor free for snapshot polls (TASK-INSPECTOR 6 / 10.2).
    async fn screenshot(&mut self) -> Result<pb::InspectorSnapshotResponse, String> {
        let (inspector, session_id, _) = self.connection()?;
        let request = pb::InspectorSnapshotRequest {
            include_system_ui: false,
            min_visible_fraction: 0.0,
            max_nodes: 0,
            max_depth: 0,
            include_raw: false,
            include_screenshot: true,
            screenshot_only: true,
            screenshot_scale: u32::from(self.shared.config.inspector.screenshot_scale),
        };
        tokio::time::timeout(SNAPSHOT_TIMEOUT, inspector.snapshot(&session_id, request))
            .await
            .map_err(|_| "screenshot acquisition timed out".to_string())?
            .map_err(|error| error.to_string())
    }
}

async fn connect(
    shared: &Shared,
    device: DeviceConfig,
    app: Option<&AppConfig>,
) -> Result<Attachment, String> {
    let mut transport = transport_for(device.platform);
    let endpoint = transport
        .prepare(&device, app)
        .await
        .map_err(|error| error.to_string())?;
    let token = crate::cli::agent_token();
    let agent = match AgentClient::connect_retry(
        endpoint.address(),
        Duration::from_millis(shared.config.agent_start_timeout_ms),
    )
    .await
    {
        Ok(agent) => agent.with_token(token.clone()),
        Err(error) => {
            let _ = transport.teardown().await;
            return Err(error.to_string());
        }
    };
    let app_entry = app.and_then(|app| app.platform(device.platform));
    let mut params = HandshakeParams::new(
        device.name.clone(),
        device.platform,
        transport.target_endpoint(&device),
        env!("CARGO_PKG_VERSION"),
    );
    params.app_id = app_entry.and_then(|entry| entry.id.clone());
    params.app_url = app_entry.and_then(|entry| entry.url.clone());
    params.auth_token = token.clone();
    params.options = handshake_options(&device, app, &shared.config);
    let handshake = match agent.handshake(&params).await {
        Ok(handshake) => handshake,
        Err(error) => {
            let _ = transport.teardown().await;
            return Err(error.to_string());
        }
    };
    if !handshake.has_capability(Capability::Inspector) {
        let _ = agent.shutdown_session(&handshake.session_id, false).await;
        let _ = transport.teardown().await;
        return Err(
            "NOT_SUPPORTED: the executor did not advertise the INSPECTOR capability".to_string(),
        );
    }
    let inspector = match InspectorClient::connect(endpoint.address()).await {
        Ok(inspector) => inspector.with_token(token),
        Err(error) => {
            let _ = agent.shutdown_session(&handshake.session_id, false).await;
            let _ = transport.teardown().await;
            return Err(error.to_string());
        }
    };
    Ok(Attachment {
        device,
        transport,
        agent,
        session_id: handshake.session_id,
        inspector,
    })
}

fn handshake_options(
    device: &DeviceConfig,
    app: Option<&AppConfig>,
    config: &ImpresarioConfig,
) -> BTreeMap<String, String> {
    let mut options = BTreeMap::new();
    options.insert(
        "cdp_backend".to_string(),
        device.options.cdp_backend.as_str().to_string(),
    );
    options.insert(
        "key_dispatch".to_string(),
        device.options.key_dispatch.as_str().to_string(),
    );
    options.insert(
        "clear_app_data".to_string(),
        app.is_some_and(|app| app.options.clear_data).to_string(),
    );
    options.insert(
        "animations_disabled".to_string(),
        device.options.animations_disabled.to_string(),
    );
    if let Some(page_match) = &device.options.page_match {
        options.insert("page_match".to_string(), page_match.clone());
    }
    options.insert(
        "poll_interval_ms".to_string(),
        config.poll_interval_ms.to_string(),
    );
    options.insert(
        "scroll_max_attempts".to_string(),
        config.scroll_max_attempts.to_string(),
    );
    options.insert(
        "focus_max_steps".to_string(),
        config.focus_max_steps.to_string(),
    );
    options
}

async fn run_actor(
    shared: Arc<Shared>,
    mut receiver: mpsc::UnboundedReceiver<DeviceCommand>,
    initial: Option<String>,
) {
    let mut actor = Actor {
        shared: Arc::clone(&shared),
        attachment: None,
    };
    if let Some(name) = initial {
        if let Err(error) = actor.attach(&name).await {
            let mut status = actor.shared.status();
            status.error = Some(error);
            actor.shared.set_status(status);
        }
    }
    let mut ticker = tokio::time::interval(shared.config.inspector.poll_interval());
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            command = receiver.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    DeviceCommand::Attach { name, reply } => {
                        let result = actor.attach(&name).await;
                        let _ = reply.send(result);
                    }
                    DeviceCommand::Detach { reply } => {
                        actor.detach().await;
                        let _ = reply.send(Ok(()));
                    }
                    DeviceCommand::Input { request, reply } => {
                        let result = actor.input(request).await;
                        let _ = reply.send(result);
                    }
                    DeviceCommand::Screenshot { reply } => {
                        let result = actor.screenshot().await;
                        let _ = reply.send(result);
                    }
                    DeviceCommand::Stop => break,
                }
            }
            _ = ticker.tick() => {
                // Freeze is a client-side view state (TASK-INSPECTOR 10.4):
                // the server keeps acquiring so unfreeze can catch up with a
                // full snapshot immediately.
                if let Err(error) = actor.poll(false).await {
                    let mut status = actor.shared.status();
                    status.error = Some(error);
                    actor.shared.set_status(status);
                }
            }
        }
    }
    actor.detach().await;
}

/// `impresario inspector` entry point.
pub fn run(
    home: Home,
    base: PathBuf,
    config_path: PathBuf,
    devices_path: PathBuf,
    args: &InspectorArgs,
) -> i32 {
    if let Some(command) = &args.command {
        match command {
            InspectorCommand::Status { json } => return status_command(&home, *json),
            InspectorCommand::Bench {
                samples,
                nodes,
                scenario,
                device,
                json,
            } => return bench_command(*samples, *nodes, scenario, device.as_deref(), *json),
        }
    }
    let config_outcome = crate::cli::load_effective_config(&base, &config_path);
    crate::render::eprint_all(&config_outcome.diagnostics);
    let Some(config) = config_outcome.config else {
        eprintln!(
            "inspector: configuration errors in {}",
            config_path.display()
        );
        return crate::cli::EXIT_USAGE;
    };
    let devices_outcome = config::load_devices_merged(&devices_path);
    crate::render::eprint_all(&devices_outcome.diagnostics);
    let Some(devices) = devices_outcome.config else {
        eprintln!(
            "inspector: configuration errors in {}",
            devices_path.display()
        );
        return crate::cli::EXIT_USAGE;
    };
    if devices.devices.is_empty() {
        eprintln!(
            "inspector: no devices configured (see {})",
            devices_path.display()
        );
        return crate::cli::EXIT_USAGE;
    }
    let apps_outcome = apps::load_apps_merged(&base.join("impresario.apps.config"));
    crate::render::eprint_all(&apps_outcome.diagnostics);
    let apps = apps_outcome
        .config
        .map(|config| config.apps)
        .unwrap_or_default();

    let listen = args
        .listen
        .clone()
        .unwrap_or_else(|| config.inspector.listen.clone());
    let bind_ip: IpAddr = match listen.parse() {
        Ok(ip) => ip,
        Err(error) => {
            eprintln!("inspector: invalid bind address '{listen}': {error}");
            return crate::cli::EXIT_USAGE;
        }
    };
    let loopback = bind_ip.is_loopback();
    let token = args
        .token
        .clone()
        .or_else(|| (!config.inspector.token.is_empty()).then(|| config.inspector.token.clone()))
        .filter(|value| !value.is_empty());
    if !loopback && token.is_none() {
        eprintln!(
            "inspector: {bind_ip} is not a loopback address; pass --token (or set inspector.token)"
        );
        return crate::cli::EXIT_USAGE;
    }
    let port = args.port.unwrap_or(config.inspector.port);
    let listener = match bind_listener(bind_ip, port) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("inspector: cannot bind {bind_ip}:{port}: {error}");
            return crate::cli::EXIT_INFRA;
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        eprintln!("inspector: cannot configure the listener: {error}");
        return crate::cli::EXIT_INFRA;
    }
    let actual_port = listener
        .local_addr()
        .map(|address| address.port())
        .unwrap_or(port);
    let host = match bind_ip {
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
        IpAddr::V6(_) => format!("[{bind_ip}]"),
        IpAddr::V4(_) => bind_ip.to_string(),
    };
    let url = format!("http://{host}:{actual_port}/");
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("inspector: cannot start the async runtime: {error}");
            return crate::cli::EXIT_INFRA;
        }
    };
    let (commands, receiver) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared::new(
        home.clone(),
        base.clone(),
        config.clone(),
        devices.devices.clone(),
        apps,
        args.app.clone(),
        token.clone(),
        listen,
        actual_port,
        loopback,
        url.clone(),
        commands,
        runtime.handle().clone(),
    ));
    let initial = args
        .device
        .clone()
        .or_else(|| devices.devices.first().map(|device| device.name.clone()));
    runtime.spawn(run_actor(Arc::clone(&shared), receiver, initial));
    if let Err(error) = write_state_file(&shared) {
        eprintln!("inspector: cannot write the inspector state file: {error}");
    }
    println!("impresario inspector: listening on {url}");
    if !loopback {
        println!(
            "impresario inspector: non-loopback bind; bearer token required (see {})",
            home.inspector_state_path().display()
        );
    }
    eprintln!("impresario inspector: workspace {}", base.display());
    if args.open {
        if let Err(error) = crate::web::open_url(&url) {
            eprintln!("inspector: cannot open the browser: {error}");
        }
    }
    let shutdown = Arc::new(AtomicBool::new(false));
    let ctrl_c_shutdown = Arc::clone(&shutdown);
    runtime.spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            ctrl_c_shutdown.store(true, Ordering::Relaxed);
        }
    });
    serve(&listener, Arc::clone(&shared), Arc::clone(&shutdown));
    let _ = shared.commands.send(DeviceCommand::Stop);
    runtime.shutdown_timeout(Duration::from_secs(2));
    let _ = std::fs::remove_file(home.inspector_state_path());
    crate::cli::EXIT_OK
}

fn bind_listener(bind_ip: IpAddr, port: u16) -> std::io::Result<TcpListener> {
    match TcpListener::bind(SocketAddr::new(bind_ip, port)) {
        Ok(listener) => Ok(listener),
        Err(error)
            if error.kind() == std::io::ErrorKind::AddrInUse && port != 0 && port < u16::MAX =>
        {
            for offset in 1..=20u16 {
                let Some(candidate) = port.checked_add(offset) else {
                    break;
                };
                if let Ok(listener) = TcpListener::bind(SocketAddr::new(bind_ip, candidate)) {
                    return Ok(listener);
                }
            }
            TcpListener::bind(SocketAddr::new(bind_ip, 0))
        }
        Err(error) => Err(error),
    }
}

fn serve(listener: &TcpListener, shared: Arc<Shared>, shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                if !try_enter(&shared) {
                    let mut stream = stream;
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                    let response = crate::web::http::Response::error(
                        503,
                        "busy",
                        "the inspector has too many open connections",
                    );
                    let _ = crate::web::http::write_response(&mut stream, &response, false);
                    continue;
                }
                let connection_state = Arc::clone(&shared);
                let spawned = std::thread::Builder::new()
                    .name("impresario-inspector".to_string())
                    .spawn(move || {
                        handle_connection(stream, &connection_state);
                        connection_state.connections.fetch_sub(1, Ordering::AcqRel);
                    });
                if spawned.is_err() {
                    shared.connections.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                eprintln!("inspector: accept failed: {error}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn try_enter(shared: &Shared) -> bool {
    let mut current = shared.connections.load(Ordering::Relaxed);
    loop {
        if current >= MAX_CONNECTIONS {
            return false;
        }
        match shared.connections.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

fn handle_connection(mut stream: TcpStream, shared: &Shared) {
    // Accepted sockets inherit the listener's non-blocking flag on Windows;
    // restore blocking mode so a separately-sent request body is awaited.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let request = match crate::web::http::read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let _ = crate::web::http::write_response(&mut stream, &error.response(), false);
            return;
        }
    };
    if request.method == "GET" && request.path == "/api/events" {
        api::serve_events(&mut stream, shared, &request);
        return;
    }
    let head_only = request.method == "HEAD";
    let response = api::handle(shared, &request);
    let _ =
        crate::web::http::write_response_with_csp(&mut stream, &response, head_only, INSPECTOR_CSP);
    let _ = stream.flush();
}

#[derive(Debug, Serialize)]
struct StateFile {
    url: String,
    bind: String,
    port: u16,
    token: Option<String>,
    device: Option<String>,
    platform: Option<String>,
    workspace: String,
    pid: u32,
    version: String,
    started_at: String,
}

fn write_state_file(shared: &Shared) -> Result<(), String> {
    let status = shared.status();
    let state = StateFile {
        url: shared.url.clone(),
        bind: shared.bind.clone(),
        port: shared.port,
        token: shared.token.clone(),
        device: status.device,
        platform: status.platform,
        workspace: shared.base.display().to_string(),
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        started_at: shared.started_at.clone(),
    };
    let _ = shared.home.ensure_dirs();
    let path = shared.home.inspector_state_path();
    let bytes = serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?;
    std::fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    restrict_state_file(&path);
    Ok(())
}

#[cfg(unix)]
fn restrict_state_file(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        let _ = std::fs::set_permissions(path, permissions);
    }
}

#[cfg(not(unix))]
fn restrict_state_file(_path: &std::path::Path) {}

fn status_command(home: &Home, json_output: bool) -> i32 {
    let path = home.inspector_state_path();
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!(
                "inspector: no running inspector ({}: {error}); start one with `impresario inspector`",
                path.display()
            );
            return crate::cli::EXIT_INFRA;
        }
    };
    let state: Value = match serde_json::from_str(&source) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("inspector: cannot parse {}: {error}", path.display());
            return crate::cli::EXIT_INFRA;
        }
    };
    let Some(url) = state.get("url").and_then(Value::as_str) else {
        eprintln!("inspector: {} has no url", path.display());
        return crate::cli::EXIT_INFRA;
    };
    let token = state.get("token").and_then(Value::as_str);
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .build();
    let mut request = agent.get(&format!("{}/healthz", url.trim_end_matches('/')));
    if let Some(token) = token.filter(|value| !value.is_empty()) {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let response = match request.call() {
        Ok(response) => response,
        Err(error) => {
            eprintln!("inspector: {url} is not reachable ({error}); the process may have exited");
            return crate::cli::EXIT_INFRA;
        }
    };
    let text = match response.into_string() {
        Ok(text) => text,
        Err(error) => {
            eprintln!("inspector: cannot read the health response: {error}");
            return crate::cli::EXIT_INFRA;
        }
    };
    let body: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("inspector: invalid health response: {error}");
            return crate::cli::EXIT_INFRA;
        }
    };
    if json_output {
        match serde_json::to_string_pretty(&body) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("inspector: cannot print status: {error}");
                return crate::cli::EXIT_INFRA;
            }
        }
        return crate::cli::EXIT_OK;
    }
    let device = body
        .get("device")
        .and_then(Value::as_str)
        .unwrap_or("(none)");
    let platform = body.get("platform").and_then(Value::as_str).unwrap_or("-");
    let rev = body.get("rev").and_then(Value::as_u64).unwrap_or(0);
    let nodes = body.get("nodes").and_then(Value::as_u64).unwrap_or(0);
    println!("inspector: {url}");
    println!("device: {device} ({platform})  rev: {rev}  nodes: {nodes}");
    if let Some(error) = body.get("error").and_then(Value::as_str) {
        println!("last error: {error}");
    }
    crate::cli::EXIT_OK
}

const BENCH_MAX_SAMPLES: usize = 10_000;
const BENCH_MAX_NODES: usize = 20_000;
const BENCH_COLUMNS: usize = 32;
const BENCH_CELL_HEIGHT: f32 = 36.0;
const BENCH_VIEWPORT: (f32, f32) = (1280.0, 720.0);
const BENCH_NOTE: &str =
    "mock pipeline over synthetic snapshots; browser paint, transport and device CPU are measured \
     only by the manual device procedure";

/// One measured stage distribution, in microseconds.
#[derive(Debug, Clone, Serialize)]
struct BenchStage {
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    mean_us: u64,
}

/// The hardware-free pipeline report printed by `impresario inspector bench`.
#[derive(Debug, Clone, Serialize)]
struct BenchReport {
    scenario: String,
    device: String,
    samples: usize,
    synthetic_nodes: usize,
    nodes_total: usize,
    nodes_in_view: usize,
    snapshot_bytes: usize,
    update_bytes: usize,
    compact_bytes: usize,
    update_kind: String,
    cpu_us_per_sample: u64,
    stages: BTreeMap<String, BenchStage>,
    event_to_render: BenchStage,
    note: String,
}

fn bench_command(
    samples: usize,
    nodes: usize,
    scenario: &str,
    device: Option<&str>,
    json_output: bool,
) -> i32 {
    let report = match bench_report(samples, nodes, scenario, device) {
        Ok(report) => report,
        Err(message) => {
            eprintln!("inspector bench: {message}");
            return crate::cli::EXIT_USAGE;
        }
    };
    if json_output {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("inspector bench: cannot print the report: {error}");
                return crate::cli::EXIT_INFRA;
            }
        }
        return crate::cli::EXIT_OK;
    }
    print_bench_report(&report);
    crate::cli::EXIT_OK
}

fn bench_report(
    samples: usize,
    nodes: usize,
    scenario: &str,
    device: Option<&str>,
) -> Result<BenchReport, String> {
    if !(1..=BENCH_MAX_SAMPLES).contains(&samples) {
        return Err(format!(
            "--samples must be between 1 and {BENCH_MAX_SAMPLES}"
        ));
    }
    if !(1..=BENCH_MAX_NODES).contains(&nodes) {
        return Err(format!("--nodes must be between 1 and {BENCH_MAX_NODES}"));
    }
    if !matches!(scenario, "scroll" | "navigate" | "modal") {
        return Err("--scenario must be scroll, navigate or modal".to_string());
    }

    let screen = ScreenInfo::logical(BENCH_VIEWPORT);
    let options = PruningOptions::for_screen(&screen);
    let mut prev = mock_snapshot(scenario, nodes, 0);
    prune_snapshot(&mut prev, options);

    let mut acquisition = Vec::with_capacity(samples);
    let mut pruning = Vec::with_capacity(samples);
    let mut diffing = Vec::with_capacity(samples);
    let mut applying = Vec::with_capacity(samples);
    let mut compacting = Vec::with_capacity(samples);
    let mut serializing = Vec::with_capacity(samples);
    let mut rendering = Vec::with_capacity(samples);
    let mut cpu = Vec::with_capacity(samples);
    let mut nodes_total = 0usize;
    let mut nodes_in_view = 0usize;
    let mut snapshot_bytes = 0usize;
    let mut update_bytes = 0usize;
    let mut compact_bytes = 0usize;
    let mut update_kind = String::new();

    for _ in 0..samples {
        let start = Instant::now();
        let mut next = mock_snapshot(scenario, nodes, 1);
        let acquisition_us = micros(start.elapsed());
        nodes_total = next.nodes.len();

        let start = Instant::now();
        prune_snapshot(&mut next, options);
        let prune_us = micros(start.elapsed());
        nodes_in_view = next.nodes.len();

        let start = Instant::now();
        let plan = plan_update(&prev, &next);
        let diff_us = micros(start.elapsed());

        let start = Instant::now();
        let applied = apply_plan(&prev, &plan)
            .map_err(|error| format!("delta application failed: {error}"))?;
        let apply_us = micros(start.elapsed());
        if applied.rev != next.rev || applied.nodes != next.nodes {
            return Err(
                "internal error: the applied update does not match the snapshot".to_string(),
            );
        }

        let start = Instant::now();
        let text = compact(&next, &CompactOptions::default());
        let compact_us = micros(start.elapsed());
        compact_bytes = text.len();

        let start = Instant::now();
        let snapshot_json = serde_json::to_vec(&next)
            .map_err(|error| format!("cannot serialize the snapshot: {error}"))?;
        let update_json = serde_json::to_vec(&plan)
            .map_err(|error| format!("cannot serialize the update: {error}"))?;
        let serialize_us = micros(start.elapsed());
        snapshot_bytes = snapshot_json.len();
        update_bytes = update_json.len();
        update_kind = match &plan {
            UpdatePlan::Full(_) => "full".to_string(),
            UpdatePlan::Delta(_) => "delta".to_string(),
        };

        acquisition.push(acquisition_us);
        pruning.push(prune_us);
        diffing.push(diff_us);
        applying.push(apply_us);
        compacting.push(compact_us);
        serializing.push(serialize_us);
        rendering.push(
            diff_us
                .saturating_add(apply_us)
                .saturating_add(compact_us)
                .saturating_add(serialize_us),
        );
        cpu.push(
            acquisition_us
                .saturating_add(prune_us)
                .saturating_add(diff_us)
                .saturating_add(apply_us)
                .saturating_add(compact_us)
                .saturating_add(serialize_us),
        );
    }

    let mut stages = BTreeMap::new();
    stages.insert("acquisition_mock".to_string(), summarize(&mut acquisition));
    stages.insert("prune".to_string(), summarize(&mut pruning));
    stages.insert("diff".to_string(), summarize(&mut diffing));
    stages.insert("apply".to_string(), summarize(&mut applying));
    stages.insert("compact".to_string(), summarize(&mut compacting));
    stages.insert("serialize".to_string(), summarize(&mut serializing));

    Ok(BenchReport {
        scenario: scenario.to_string(),
        device: device.unwrap_or("mock").to_string(),
        samples,
        synthetic_nodes: nodes,
        nodes_total,
        nodes_in_view,
        snapshot_bytes,
        update_bytes,
        compact_bytes,
        update_kind,
        cpu_us_per_sample: mean(&cpu),
        stages,
        event_to_render: summarize(&mut rendering),
        note: BENCH_NOTE.to_string(),
    })
}

fn print_bench_report(report: &BenchReport) {
    let scenario = &report.scenario;
    let device = &report.device;
    let nodes_in_view = report.nodes_in_view;
    let nodes_total = report.nodes_total;
    let synthetic_nodes = report.synthetic_nodes;
    let update_bytes = report.update_bytes;
    let update_kind = &report.update_kind;
    let snapshot_bytes = report.snapshot_bytes;
    let compact_bytes = report.compact_bytes;
    let cpu = millis(report.cpu_us_per_sample);
    let render = &report.event_to_render;
    println!("# impresario inspector bench (mock pipeline)");
    println!();
    println!("- scenario: {scenario}");
    println!("- device: {device} (synthetic; no device attached)");
    println!("- samples: {}", report.samples);
    println!(
        "- nodes: {nodes_in_view} in view / {nodes_total} total ({synthetic_nodes} synthetic)"
    );
    println!(
        "- payload: {update_bytes} bytes ({update_kind} update), {snapshot_bytes} bytes (full \
         snapshot), {compact_bytes} bytes (compact text)"
    );
    println!("- cpu (mock, single-threaded): {cpu}");
    println!(
        "- event->render (mock pipeline): p50 {}, p95 {}, p99 {}",
        millis(render.p50_us),
        millis(render.p95_us),
        millis(render.p99_us)
    );
    println!();
    println!("| stage | p50 | p95 | p99 | mean |");
    println!("|---|---:|---:|---:|---:|");
    for (name, stage) in &report.stages {
        println!(
            "| {name} | {} | {} | {} | {} |",
            millis(stage.p50_us),
            millis(stage.p95_us),
            millis(stage.p99_us),
            millis(stage.mean_us)
        );
    }
    println!();
    println!("note: {}", report.note);
}

fn summarize(values: &mut [u64]) -> BenchStage {
    values.sort_unstable();
    BenchStage {
        p50_us: percentile(values, 50),
        p95_us: percentile(values, 95),
        p99_us: percentile(values, 99),
        mean_us: mean(values),
    }
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = percent.saturating_mul(sorted.len().saturating_sub(1)) / 100;
    sorted.get(rank).copied().unwrap_or(0)
}

fn mean(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let total = values.iter().copied().fold(0u64, u64::saturating_add);
    total / values.len() as u64
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn millis(micros: u64) -> String {
    format!("{:.3} ms", micros as f64 / 1000.0)
}

/// Build a deterministic synthetic snapshot for the mock pipeline.
///
/// `step` 0 is the base screen; step 1 applies the scenario change:
/// scroll moves the page, navigate rewrites names and focuses a link, and
/// modal hides part of the background and adds an overlay dialog.
fn mock_snapshot(scenario: &str, nodes: usize, step: u64) -> Snapshot {
    let screen = ScreenInfo::logical(BENCH_VIEWPORT);
    let columns = BENCH_COLUMNS.max(1);
    let cell_width = BENCH_VIEWPORT.0 / columns as f32;
    let offset = if scenario == "scroll" && step > 0 {
        BENCH_VIEWPORT.1 / 2.0
    } else {
        0.0
    };
    let mut list = Vec::with_capacity(nodes.saturating_add(10));
    list.push(SnapshotNode::new(
        1,
        0,
        Role::Generic,
        "bench-app",
        Rect::new(0.0, 0.0, BENCH_VIEWPORT.0, BENCH_VIEWPORT.1),
    ));
    let roles = [
        Role::Button,
        Role::Link,
        Role::Text,
        Role::ListItem,
        Role::Image,
    ];
    for index in 0..nodes {
        let id = index as u64 + 2;
        let column = index % columns;
        let row = index / columns;
        let role = roles
            .get(index % roles.len())
            .copied()
            .unwrap_or(Role::Generic);
        let mut node = SnapshotNode::new(
            id,
            1,
            role,
            format!("Item {index}"),
            Rect::new(
                column as f32 * cell_width,
                row as f32 * BENCH_CELL_HEIGHT - offset,
                (cell_width - 2.0).max(1.0),
                BENCH_CELL_HEIGHT - 2.0,
            ),
        )
        .with_text(format!("Bench item {index}"));
        if index % 11 == 0 {
            node = node.with_test_id(format!("item-{index}"));
        }
        if scenario == "navigate" && step > 0 && index % 5 == 0 {
            node.name = format!("Item {index} updated");
            node.text = format!("Bench item {index} updated");
        }
        if scenario == "modal" && step > 0 && index % 3 == 0 {
            node.state.hidden_by_a11y = true;
        }
        list.push(node);
    }
    if scenario == "modal" && step > 0 {
        let dialog_id = nodes as u64 + 2;
        list.push(
            SnapshotNode::new(
                dialog_id,
                1,
                Role::Dialog,
                "Bench dialog",
                Rect::new(320.0, 180.0, 640.0, 360.0),
            )
            .with_escapes_clip(true),
        );
        for index in 0..8 {
            let id = dialog_id + 1 + index as u64;
            list.push(SnapshotNode::new(
                id,
                dialog_id,
                Role::Button,
                format!("Action {index}"),
                Rect::new(352.0, 220.0 + index as f32 * 36.0, 576.0, 32.0),
            ));
        }
    }
    if scenario == "scroll" && step > 0 {
        if let Some(root) = list.first_mut() {
            root.scroll = Some(ScrollInfo::vertical(true, true));
        }
    }
    if scenario == "navigate" && step > 0 {
        if let Some(node) = list.get_mut(1) {
            node.state.focusable = true;
            node.state.focused = true;
        }
    }
    Snapshot::new("bench-device", "bench-device", Platform::Web, screen, list)
        .with_rev(step.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_interval_is_bounded() {
        let mut config = ImpresarioConfig {
            project: "t".to_string(),
            timeout_default_ms: 1000,
            screenshot_on_failure: false,
            report_output: PathBuf::from("./reports"),
            plays_dir: PathBuf::from("./plays"),
            scenes_dir: PathBuf::from("./scenes"),
            theatres_dir: PathBuf::from("./theatres"),
            retries: 0,
            parallel_devices: 1,
            app_reset: impresario_core::config::AppReset::PerAct,
            poll_interval_ms: 100,
            scroll_max_attempts: 1,
            focus_max_steps: 1,
            agent_start_timeout_ms: 1000,
            command_grace_ms: 0,
            reporters: Vec::new(),
            unverifiable_policy: impresario_core::probe::UnverifiablePolicy::Fail,
            schedules: Vec::new(),
            watchers: Vec::new(),
            notifications: Default::default(),
            inspector: Default::default(),
        };
        assert_eq!(config.inspector.poll_interval(), Duration::from_millis(250));
        config.inspector.fps = 0;
        assert_eq!(config.inspector.poll_interval(), Duration::from_secs(1));
        config.inspector.fps = 100;
        assert_eq!(config.inspector.poll_interval(), Duration::from_millis(33));
    }

    #[test]
    fn state_file_is_json() {
        let state = StateFile {
            url: "http://127.0.0.1:7311/".to_string(),
            bind: "127.0.0.1".to_string(),
            port: 7311,
            token: None,
            device: Some("tv".to_string()),
            platform: Some("webos".to_string()),
            workspace: ".".to_string(),
            pid: 1,
            version: "0.0.0".to_string(),
            started_at: "now".to_string(),
        };
        let text = serde_json::to_string(&state).expect("state serializes");
        assert!(text.contains("\"port\":7311"));
    }

    #[test]
    fn bench_percentiles_are_monotonic() {
        let mut values: Vec<u64> = (1..=100).collect();
        let stage = summarize(&mut values);
        assert_eq!(stage.p50_us, 50);
        assert_eq!(stage.p95_us, 95);
        assert_eq!(stage.p99_us, 99);
        assert_eq!(stage.mean_us, 50);
        assert_eq!(percentile(&[], 95), 0);
        assert_eq!(mean(&[]), 0);
    }

    #[test]
    fn bench_scenarios_apply_cleanly() {
        let screen = ScreenInfo::logical(BENCH_VIEWPORT);
        let options = PruningOptions::for_screen(&screen);
        for scenario in ["scroll", "navigate", "modal"] {
            let mut prev = mock_snapshot(scenario, 300, 0);
            prune_snapshot(&mut prev, options);
            let mut next = mock_snapshot(scenario, 300, 1);
            prune_snapshot(&mut next, options);
            assert!(!next.nodes.is_empty(), "{scenario} keeps visible nodes");
            next.validate().expect("pruned snapshot is valid");
            let plan = plan_update(&prev, &next);
            let applied = apply_plan(&prev, &plan).expect("the update applies");
            assert_eq!(applied.rev, next.rev);
            assert_eq!(applied.nodes, next.nodes);
            if scenario == "modal" {
                assert!(next.nodes.iter().any(|node| node.role == Role::Dialog));
            }
            if scenario == "scroll" {
                assert!(next
                    .nodes
                    .first()
                    .and_then(|node| node.scroll)
                    .is_some_and(|scroll| scroll.can_scroll_forward));
            }
        }
    }

    #[test]
    fn bench_report_has_stable_shape() {
        let report = bench_report(3, 1200, "scroll", Some("tv")).expect("bench report");
        assert_eq!(report.device, "tv");
        assert_eq!(report.scenario, "scroll");
        assert_eq!(report.samples, 3);
        assert_eq!(report.synthetic_nodes, 1200);
        assert_eq!(report.nodes_total, 1201);
        assert!(report.nodes_in_view > 0);
        assert!(report.nodes_in_view < report.nodes_total);
        assert!(report.snapshot_bytes > 0);
        assert!(report.update_bytes > 0);
        assert!(report.compact_bytes > 0);
        assert!(matches!(report.update_kind.as_str(), "full" | "delta"));
        assert_eq!(report.stages.len(), 6);
        assert!(report.stages.contains_key("prune"));
        assert!(report.stages.contains_key("serialize"));
    }

    #[test]
    fn bench_rejects_invalid_arguments() {
        assert!(bench_report(0, 10, "scroll", None).is_err());
        assert!(bench_report(1, 0, "scroll", None).is_err());
        assert!(bench_report(1, 10, "unknown", None).is_err());
    }

    #[test]
    fn publish_revision_only_advances_on_content_change() {
        let first = mock_snapshot("scroll", 8, 0);
        assert_eq!(next_publish_rev(None, 0, 0, &first), Some(1));

        let mut same = first.clone();
        same.rev = 99;
        same.stats.acquisition_ms = 500;
        assert_eq!(next_publish_rev(Some(&first), 1, 1, &same), None);

        // Reattach resets the status revision but must not reset the master
        // revision.
        assert_eq!(next_publish_rev(Some(&first), 1, 0, &same), Some(2));

        let mut changed = same.clone();
        changed.nodes[1].name = "changed".to_string();
        assert_eq!(next_publish_rev(Some(&first), 1, 1, &changed), Some(2));
        assert_eq!(next_publish_rev(Some(&first), 7, 7, &same), None);
        assert_eq!(next_publish_rev(Some(&first), 7, 7, &changed), Some(8));

        // No previous snapshot (fresh process or after detach): full update.
        assert_eq!(next_publish_rev(None, 7, 0, &same), Some(8));
    }
}
