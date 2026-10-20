//! Device transports (spec section 9): one implementation per platform
//! family. A transport prepares a device-executor endpoint, keeps host-side
//! log collectors running and tears everything down (children are
//! kill-on-drop, stderr/stdout are captured, every wait has a timeout).
//!
//! The spec writes `prepare(&mut self, dev: &Device)`; the configuration
//! type is [`DeviceConfig`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use impresario_core::apps::AppConfig;
use impresario_core::command::{WireError, WireErrorCode};
use impresario_core::config::DeviceConfig;
use impresario_core::home::Home;
use impresario_core::platforms::{Platform, TransportFamily};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::time::Instant;

use crate::adb::{AdbClient, AdbError, AdbTarget};
use crate::log::{unix_ms_now, LogBuffer, LogLevel, LogRecord};

/// How long the executor may take to start listening after `prepare` spawned
/// it. Overridable with `IMPRESARIO_AGENT_START_TIMEOUT_MS`.
const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for the Apple clean-state reinstall (`simctl`/`devicectl`).
const APPLE_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// Port defaults used when the device entry does not override them.
const ANDROID_AGENT_PORT: u16 = 50061;
const APPLE_AGENT_PORT: u16 = 50062;

/// Agent packages (must match `agents/android`, spec section 11).
const AGENT_PACKAGE: &str = "com.impresario.agent";
const AGENT_INSTRUMENTATION_PACKAGE: &str = "com.impresario.agent.test";
const AGENT_RUNNER_CLASS: &str = "com.impresario.agent.AgentRunner";
const ANDROID_JUNIT_RUNNER: &str = "androidx.test.runner.AndroidJUnitRunner";

/// A resolved executor endpoint: `host:port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    address: String,
}

impl Endpoint {
    #[must_use]
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }

    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// True when the address points at this host.
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        is_loopback_address(&self.address)
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.address)
    }
}

/// What kind of log source a transport contributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSourceKind {
    /// Executor-origin logs (`StreamLogs`), collected by the session client.
    Agent,
    /// Page/console logs from a connector.
    Page,
    /// Host-side collector (adb logcat, instrumentation output, ...).
    Host,
}

/// A log source a transport keeps alive while the device is prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSource {
    pub name: String,
    pub kind: LogSourceKind,
}

impl LogSource {
    #[must_use]
    pub fn new(name: impl Into<String>, kind: LogSourceKind) -> Self {
        Self {
            name: name.into(),
            kind,
        }
    }
}

/// Transport failures; `DeviceUnavailable` maps onto the wire error of the
/// same name (exit code 3 territory on the master side).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    #[error("device unavailable: {0}")]
    DeviceUnavailable(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

impl TransportError {
    #[must_use]
    pub fn to_wire(&self) -> WireError {
        let code = match self {
            TransportError::DeviceUnavailable(_) => WireErrorCode::DeviceUnavailable,
            TransportError::Transport(_) | TransportError::Io(_) => WireErrorCode::Transport,
        };
        WireError::new(code, self.to_string())
    }
}

/// What a [`DeviceTransport::reset_app_data`] call achieved (I-84).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetOutcome {
    /// `options.clear_app_data` is off: no clean state was requested.
    NotRequested,
    /// The app under test was wiped or reinstalled.
    Reset,
    /// The host cannot reset this app's container; the agent degrades to a
    /// cold start (Apple without `options.app_path`).
    Unavailable(String),
}

/// One host command of the Apple clean-state sequence (I-84).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetCommand {
    pub program: String,
    pub args: Vec<String>,
    /// A failing command is tolerated (uninstalling an app that is not
    /// installed is not an error).
    pub tolerate_failure: bool,
}

/// Which host tool performs the Apple clean-state reinstall (I-84).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppleResetTool {
    /// `xcrun simctl` for iOS simulators.
    Simctl,
    /// `xcrun devicectl` for iOS/tvOS devices (Xcode 15+).
    Devicectl,
    /// `ideviceinstaller` (libimobiledevice) fallback.
    IdeviceInstaller,
}

/// Pure argv builder for the Apple clean-state sequence: uninstall, then
/// install the app under test (I-84; unit-tested).
#[must_use]
pub fn apple_reset_commands(
    tool: AppleResetTool,
    udid: &str,
    app_id: &str,
    app_path: &str,
) -> Vec<ResetCommand> {
    let xcrun = |args: Vec<String>, tolerate_failure: bool| ResetCommand {
        program: "xcrun".to_string(),
        args,
        tolerate_failure,
    };
    match tool {
        AppleResetTool::Simctl => vec![
            xcrun(
                vec![
                    "simctl".to_string(),
                    "uninstall".to_string(),
                    udid.to_string(),
                    app_id.to_string(),
                ],
                true,
            ),
            xcrun(
                vec![
                    "simctl".to_string(),
                    "install".to_string(),
                    udid.to_string(),
                    app_path.to_string(),
                ],
                false,
            ),
        ],
        AppleResetTool::Devicectl => vec![
            xcrun(
                vec![
                    "devicectl".to_string(),
                    "device".to_string(),
                    "uninstall".to_string(),
                    "app".to_string(),
                    "--device".to_string(),
                    udid.to_string(),
                    app_id.to_string(),
                ],
                true,
            ),
            xcrun(
                vec![
                    "devicectl".to_string(),
                    "device".to_string(),
                    "install".to_string(),
                    "app".to_string(),
                    "--device".to_string(),
                    udid.to_string(),
                    app_path.to_string(),
                ],
                false,
            ),
        ],
        AppleResetTool::IdeviceInstaller => vec![
            ResetCommand {
                program: "ideviceinstaller".to_string(),
                args: vec![
                    "-u".to_string(),
                    udid.to_string(),
                    "-U".to_string(),
                    app_id.to_string(),
                ],
                tolerate_failure: true,
            },
            ResetCommand {
                program: "ideviceinstaller".to_string(),
                args: vec![
                    "-u".to_string(),
                    udid.to_string(),
                    "-i".to_string(),
                    app_path.to_string(),
                ],
                tolerate_failure: false,
            },
        ],
    }
}

/// One device transport per family (spec section 9).
#[async_trait]
pub trait DeviceTransport: Send {
    /// Prepare the device and return the executor endpoint (spec 7.3 step 1).
    /// `app` is `None` for an app-less web run; native transports use it for
    /// the app id/package when they need one.
    async fn prepare(
        &mut self,
        device: &DeviceConfig,
        app: Option<&AppConfig>,
    ) -> Result<Endpoint, TransportError>;

    /// Host-side clean state before a reset `AppControl` (spec 7.3 step 4,
    /// I-84). The Android family wipes the package inside the agent, so its
    /// transport keeps this default (`NotRequested`: no host-side work). The
    /// Apple family reinstalls the app under test from the app's platform
    /// `package` and returns `Reset`, or `Unavailable` when the host cannot.
    async fn reset_app_data(
        &mut self,
        _device: &DeviceConfig,
        _app: Option<&AppConfig>,
    ) -> Result<ResetOutcome, TransportError> {
        Ok(ResetOutcome::NotRequested)
    }

    /// Release everything `prepare` acquired (spec 7.3 step 6).
    async fn teardown(&mut self) -> Result<(), TransportError>;

    /// Host-side log sources this transport manages.
    fn log_sources(&self) -> Vec<LogSource>;

    /// Captured host-side log lines (empty by default).
    fn logs(&self) -> LogBuffer {
        LogBuffer::empty()
    }
}

/// The transport for a platform family (spec section 9).
#[must_use]
pub fn transport_for(platform: Platform) -> Box<dyn DeviceTransport> {
    match platform.family() {
        TransportFamily::Connector => Box::new(ConnectorTransport::new()),
        TransportFamily::Adb => Box::new(AdbTransport::new()),
        TransportFamily::Apple => Box::new(AppleTransport::new()),
    }
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

struct ManagedChild {
    name: String,
    child: tokio::process::Child,
}

impl ManagedChild {
    fn spawn(name: impl Into<String>, cmd: &mut Command) -> Result<Self, TransportError> {
        let name = name.into();
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| TransportError::Transport(format!("failed to spawn {name}: {e}")))?;
        tracing::debug!(name = %name, pid = child.id(), "child process started");
        Ok(Self { name, child })
    }

    fn take_stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.child.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.child.stderr.take()
    }

    async fn stop(&mut self) {
        tracing::debug!(name = %self.name, "stopping child process");
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

/// Capture stdout/stderr of a child into a log buffer.
fn drain_child(child: &mut ManagedChild, buffer: &LogBuffer, source: &str) {
    if let Some(stdout) = child.take_stdout() {
        capture_lines(stdout, buffer.clone(), source.to_string());
    }
    if let Some(stderr) = child.take_stderr() {
        capture_lines(stderr, buffer.clone(), format!("{source}.stderr"));
    }
}

fn capture_lines<R>(reader: R, buffer: LogBuffer, source: String)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            buffer.push(parse_captured_line(&source, &line));
        }
    });
}

fn parse_captured_line(source: &str, line: &str) -> LogRecord {
    if source.starts_with("logcat") {
        parse_logcat_line(line)
    } else {
        LogRecord {
            unix_ms: unix_ms_now(),
            level: LogLevel::Debug,
            source: source.to_string(),
            message: line.to_string(),
        }
    }
}

/// Best-effort parse of one `adb logcat -v epoch` line:
/// `<epoch>.<ms> <pid> <tid> <L> <Tag>: <message>`.
fn parse_logcat_line(line: &str) -> LogRecord {
    let mut unix_ms = unix_ms_now();
    let mut level = LogLevel::Debug;
    for (index, token) in line.split_whitespace().enumerate() {
        if index == 0 {
            if let Ok(epoch) = token.parse::<f64>() {
                if epoch > 1_000_000_000.0 && epoch < 10_000_000_000.0 {
                    unix_ms = (epoch * 1000.0) as i64;
                }
            }
        } else if token.len() == 1 {
            level = match token {
                "E" | "F" | "A" => LogLevel::Error,
                "W" => LogLevel::Warn,
                "I" => LogLevel::Info,
                _ => LogLevel::Debug,
            };
            break;
        }
    }
    LogRecord {
        unix_ms,
        level,
        source: "logcat".to_string(),
        message: line.to_string(),
    }
}

async fn command_output(
    cmd: &mut Command,
    timeout: Duration,
    what: &str,
) -> Result<std::process::Output, TransportError> {
    cmd.stdin(Stdio::null());
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(TransportError::Transport(format!(
            "failed to run {what}: {e}"
        ))),
        Err(_) => Err(TransportError::Transport(format!(
            "{what} did not finish within {} s",
            timeout.as_secs()
        ))),
    }
}

fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

async fn free_local_port() -> Result<u16, TransportError> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

async fn port_open(address: &str, timeout: Duration) -> bool {
    matches!(
        tokio::time::timeout(timeout, tokio::net::TcpStream::connect(address)).await,
        Ok(Ok(_))
    )
}

async fn wait_for_port(address: &str, timeout: Duration) -> Result<(), TransportError> {
    let deadline = Instant::now() + timeout;
    loop {
        if port_open(address, Duration::from_millis(250)).await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(TransportError::Transport(format!(
                "nothing is listening on {address} after {} s",
                timeout.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

fn start_timeout() -> Duration {
    std::env::var("IMPRESARIO_AGENT_START_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_START_TIMEOUT)
}

fn host_part(address: &str) -> &str {
    address
        .rsplit_once(':')
        .map_or(address, |(host, _port)| host)
}

fn is_loopback_address(address: &str) -> bool {
    let host = host_part(address)
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Find an executable on `PATH` (no new dependency).
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The connector binary: `$IMPRESARIO_CONNECTOR_BIN`, else a sibling of the
/// current executable, else `PATH`.
#[must_use]
pub fn connector_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("IMPRESARIO_CONNECTOR_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let candidate = dir.join(format!(
                "impresario-connector{}",
                std::env::consts::EXE_SUFFIX
            ));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    find_in_path("impresario-connector")
}

/// FNV-1a 64 over the file contents; a cheap, stable "version hash" for the
/// APK install cache (no crypto needed).
fn file_hash(path: &Path) -> Result<String, TransportError> {
    let bytes = std::fs::read(path)
        .map_err(|e| TransportError::Transport(format!("cannot read {}: {e}", path.display())))?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(format!("{hash:016x}"))
}

fn sanitize_for_filename(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// ConnectorTransport (web, webos, tizen)
// ---------------------------------------------------------------------------

/// Web-family transport: connects to a connector; when the configured
/// address is loopback and nothing is listening, spawns
/// `impresario-connector serve --listen <addr>` (killed on drop).
pub struct ConnectorTransport {
    child: Option<ManagedChild>,
    address: Option<String>,
    logs: LogBuffer,
}

impl ConnectorTransport {
    #[must_use]
    pub fn new() -> Self {
        Self {
            child: None,
            address: None,
            logs: LogBuffer::new(1000),
        }
    }
}

impl Default for ConnectorTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeviceTransport for ConnectorTransport {
    async fn prepare(
        &mut self,
        device: &DeviceConfig,
        _app: Option<&AppConfig>,
    ) -> Result<Endpoint, TransportError> {
        let address = device
            .connector_address()
            .ok_or_else(|| {
                TransportError::Transport(format!(
                    "device '{}' has no connector_address and no default for {}",
                    device.name, device.platform
                ))
            })?
            .to_string();
        self.address = Some(address.clone());

        if !is_loopback_address(&address) || port_open(&address, Duration::from_millis(300)).await {
            return Ok(Endpoint::new(address));
        }

        let binary = connector_binary().ok_or_else(|| {
            TransportError::Transport(format!(
                "nothing is listening on {address} and impresario-connector was not found; \
                 build it (cargo build -p impresario-connector -p impresario-master) or start it manually"
            ))
        })?;
        let mut cmd = Command::new(binary);
        cmd.args(["serve", "--listen", address.as_str()]);
        if let Ok(token) = std::env::var("IMPRESARIO_AGENT_TOKEN") {
            if !token.is_empty() {
                cmd.args(["--token", token.as_str()]);
            }
        }
        let mut child = ManagedChild::spawn("impresario-connector", &mut cmd)?;
        drain_child(&mut child, &self.logs, "connector");
        self.child = Some(child);

        wait_for_port(&address, start_timeout())
            .await
            .map_err(|e| {
                TransportError::Transport(format!(
                    "connector did not start listening on {address}: {e}"
                ))
            })?;
        Ok(Endpoint::new(address))
    }

    async fn teardown(&mut self) -> Result<(), TransportError> {
        if let Some(mut child) = self.child.take() {
            child.stop().await;
        }
        Ok(())
    }

    fn log_sources(&self) -> Vec<LogSource> {
        vec![LogSource::new("connector", LogSourceKind::Page)]
    }

    fn logs(&self) -> LogBuffer {
        self.logs.clone()
    }
}

// ---------------------------------------------------------------------------
// AdbTransport (android, androidtv)
// ---------------------------------------------------------------------------

/// Android-family transport (spec section 9): native ADB checks, agent APK
/// install/refresh, instrumentation runner and host-side logcat. The master
/// dials the agent over the network whenever the device has a discoverable
/// address; a direct protocol forward is only the fallback (I-62). No `adb`
/// binary is spawned (D-19).
pub struct AdbTransport {
    client: Option<AdbClient>,
    serial: Option<String>,
    local_port: Option<u16>,
    device_port: u16,
    forward: Option<tokio::task::JoinHandle<()>>,
    instrument: Option<tokio::task::JoinHandle<()>>,
    logcat: Option<tokio::task::JoinHandle<()>>,
    home: Option<Home>,
    logs: LogBuffer,
}

impl Default for AdbTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl AdbTransport {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: None,
            serial: None,
            local_port: None,
            device_port: ANDROID_AGENT_PORT,
            forward: None,
            instrument: None,
            logcat: None,
            home: None,
            logs: LogBuffer::new(2000),
        }
    }

    /// Use an explicit home instead of resolving `IMPRESARIO_HOME`. Tests and
    /// embedders use this to keep the real user home untouched.
    #[must_use]
    pub fn with_home(home: Home) -> Self {
        Self {
            home: Some(home),
            ..Self::new()
        }
    }

    fn adb_error(error: AdbError) -> TransportError {
        match error {
            AdbError::Connect { .. } => TransportError::DeviceUnavailable(error.to_string()),
            other => TransportError::Transport(other.to_string()),
        }
    }

    /// Install/refresh the agent APKs when their version hash differs
    /// (spec section 9). The installed-hash cache lives under
    /// `$IMPRESARIO_HOME/cache`, keyed by serial.
    async fn ensure_agent_installed(
        &self,
        client: &AdbClient,
        home: &Home,
    ) -> Result<(), TransportError> {
        let serial = client.target().label();
        let apks = resolve_android_apks();
        if apks.is_empty() {
            let installed = client
                .package_installed(AGENT_PACKAGE)
                .await
                .map_err(Self::adb_error)?;
            if installed {
                tracing::debug!(
                    %serial,
                    "no local agent APK found; using the package already installed on the device"
                );
                return Ok(());
            }
            return Err(TransportError::DeviceUnavailable(
                "no impresario Android agent APK found: build agents/android or set \
                 IMPRESARIO_ANDROID_AGENT_APK / IMPRESARIO_ANDROID_AGENT_TEST_APK / \
                 IMPRESARIO_ANDROID_STUB_APK"
                    .to_string(),
            ));
        }

        let cache_path = home.cache_dir().join(format!(
            "adb-install-{}.cache",
            sanitize_for_filename(&serial)
        ));
        let cached = read_cache(&cache_path);
        let mut current = BTreeMap::new();
        for (label, path) in &apks {
            current.insert(label.clone(), file_hash(path)?);
        }
        let installed = client
            .package_installed(AGENT_PACKAGE)
            .await
            .map_err(Self::adb_error)?
            && client
                .package_installed(AGENT_INSTRUMENTATION_PACKAGE)
                .await
                .map_err(Self::adb_error)?;
        if cached == current && installed {
            tracing::debug!(%serial, "agent APKs already installed with the same hashes");
            return Ok(());
        }
        for (label, path) in &apks {
            if installed && cached.get(label) == current.get(label) {
                continue;
            }
            tracing::info!(%serial, apk = %path.display(), "installing agent APK");
            client.install_apk(path).await.map_err(Self::adb_error)?;
        }
        self.logs.push(LogRecord::new(
            "adb",
            LogLevel::Info,
            format!("installed agent APKs on {serial}: {}", apks.len()),
        ));
        write_cache(&cache_path, &current);
        Ok(())
    }

    /// `pm clear <app id>` when the app option asks for it.
    async fn maybe_clear_data(
        &self,
        client: &AdbClient,
        app: Option<&AppConfig>,
        platform: Platform,
    ) -> Result<(), TransportError> {
        let Some(app) = app else {
            return Ok(());
        };
        if !app.options.clear_data {
            return Ok(());
        }
        let Some(app_id) = app
            .platform_with_fallback(platform)
            .and_then(|entry| entry.id.as_deref())
        else {
            return Ok(());
        };
        let output = client
            .clear_package(app_id)
            .await
            .map_err(Self::adb_error)?;
        let text = output.stdout_text();
        if !text.contains("Success") {
            tracing::warn!(
                serial = client.target().label(),
                app_id,
                "pm clear did not report success: {}",
                text.trim()
            );
        }
        Ok(())
    }

    /// Bind a loopback listener bridged directly to `tcp:<device_port>`.
    async fn forward_port(&mut self, client: &AdbClient) -> Result<(), TransportError> {
        if self.forward.is_some() {
            return Ok(());
        }
        let local_port = free_local_port().await?;
        let handle = crate::adb::spawn_forward(client, self.device_port, local_port)
            .await
            .map_err(Self::adb_error)?;
        self.local_port = Some(local_port);
        self.forward = Some(handle);
        wait_for_port(&format!("127.0.0.1:{local_port}"), Duration::from_secs(3)).await?;
        Ok(())
    }

    /// Reachable address of the device itself: the configured override, the
    /// TCP target host, or the source address of the device's default route.
    async fn network_address(&self, device: &DeviceConfig, client: &AdbClient) -> Option<String> {
        if let Some(configured) = device.connection.endpoint.as_deref() {
            return Some(configured.to_string());
        }
        client
            .network_host()
            .await
            .map(|host| format!("{host}:{}", self.device_port))
    }

    fn spawn_stream(
        client: &AdbClient,
        command: String,
        source: &'static str,
        logs: &LogBuffer,
    ) -> tokio::task::JoinHandle<()> {
        let client = client.clone();
        let logs = logs.clone();
        tokio::spawn(async move {
            if let Err(error) =
                crate::adb::stream_shell(&client, &command, source, logs.clone()).await
            {
                logs.push(LogRecord::new(
                    source,
                    LogLevel::Warn,
                    format!("stream stopped: {error}"),
                ));
            }
        })
    }

    /// Run `am instrument -w -r ...` over shell v2 and capture its output.
    fn spawn_instrumentation(&mut self, client: &AdbClient) -> Result<(), TransportError> {
        let runner = format!("{AGENT_INSTRUMENTATION_PACKAGE}/{ANDROID_JUNIT_RUNNER}");
        let command = format!(
            "am instrument -w -r --user 0 -e class {AGENT_RUNNER_CLASS} -e port {} {runner}",
            self.device_port
        );
        self.instrument = Some(Self::spawn_stream(
            client,
            command,
            "instrument",
            &self.logs,
        ));
        Ok(())
    }

    /// Optional host-side `logcat`, filtered to the app pid when the app is
    /// already running. USB carries a single ADB session, so it is skipped
    /// there; the instrumentation output and agent logs still flow.
    async fn spawn_logcat(
        &mut self,
        client: &AdbClient,
        app: Option<&AppConfig>,
        platform: Platform,
    ) -> Result<(), TransportError> {
        if client.target().is_usb() {
            self.logs.push(LogRecord::new(
                "logcat",
                LogLevel::Debug,
                "host-side logcat is unavailable over the single USB ADB session",
            ));
            return Ok(());
        }
        let pid = match app
            .and_then(|app| app.platform_with_fallback(platform))
            .and_then(|entry| entry.id.as_deref())
        {
            Some(app_id) => client.pid_of(app_id).await.map_err(Self::adb_error)?,
            None => None,
        };
        let mut command = "logcat -v epoch".to_string();
        match pid {
            Some(pid) => command.push_str(&format!(" --pid {pid}")),
            None => self.logs.push(LogRecord::new(
                "logcat",
                LogLevel::Debug,
                "app pid unknown at prepare time; capturing unfiltered logcat",
            )),
        }
        self.logcat = Some(Self::spawn_stream(client, command, "logcat", &self.logs));
        Ok(())
    }
}

#[async_trait]
impl DeviceTransport for AdbTransport {
    async fn prepare(
        &mut self,
        device: &DeviceConfig,
        app: Option<&AppConfig>,
    ) -> Result<Endpoint, TransportError> {
        let home = match self.home.clone() {
            Some(home) => home,
            None => Home::resolve().map_err(|error| {
                TransportError::Transport(format!("cannot resolve home: {error}"))
            })?,
        };
        home.ensure_dirs()
            .map_err(|error| TransportError::Transport(format!("cannot create home: {error}")))?;
        let target = AdbTarget::for_device(device).map_err(Self::adb_error)?;
        let client = AdbClient::new(target, &home);
        self.serial = Some(client.target().label());
        self.device_port = device.effective_agent_port().unwrap_or(ANDROID_AGENT_PORT);
        self.client = Some(client.clone());

        let probe = client.probe().await.map_err(Self::adb_error)?;
        self.logs.push(LogRecord::new(
            "adb",
            LogLevel::Info,
            format!(
                "{} {} (Android {}, {}) via {}",
                probe.manufacturer,
                probe.model,
                probe.release,
                probe.form_factor(),
                client.target().label()
            ),
        ));
        if probe.tv != matches!(device.platform, Platform::AndroidTv) {
            tracing::warn!(
                device = %device.name,
                detected = probe.form_factor(),
                configured = %device.platform,
                "the device's form factor differs from its configured platform"
            );
        }

        self.ensure_agent_installed(&client, &home).await?;
        self.maybe_clear_data(&client, app, device.platform).await?;

        // The master dials the agent's own network address whenever the
        // device has one; the direct forward is only the fallback. The agent
        // binds every interface, so the fallback can also be taken after the
        // session has started (docs/DECISIONS.md I-62).
        let network = self.network_address(device, &client).await;
        if network.is_none() {
            self.forward_port(&client).await?;
        }

        self.spawn_instrumentation(&client)?;
        self.spawn_logcat(&client, app, device.platform).await?;

        if let Some(address) = network {
            match wait_for_port(&address, start_timeout()).await {
                Ok(()) => return Ok(Endpoint::new(address)),
                Err(error) => {
                    if client.target().is_usb() {
                        return Err(TransportError::Transport(format!(
                            "the Android agent is not reachable on {address} ({error}); \
                             USB ADB carries one session, so the forward fallback cannot \
                             coexist with instrumentation. Enable Wi-Fi or set \
                             connection.endpoint to a reachable agent address"
                        )));
                    }
                    tracing::warn!(
                        %address,
                        "the Android agent is not reachable over the network ({error}); \
                         using the direct forward"
                    );
                    self.forward_port(&client).await?;
                }
            }
        }

        let local_port = self.local_port.ok_or_else(|| {
            TransportError::Transport("no local port for the ADB forward".to_string())
        })?;
        let address = format!("127.0.0.1:{local_port}");
        wait_for_port(&address, start_timeout())
            .await
            .map_err(|error| {
                TransportError::Transport(format!(
                    "the Android agent did not start listening on {address}: {error}"
                ))
            })?;
        Ok(Endpoint::new(address))
    }

    async fn teardown(&mut self) -> Result<(), TransportError> {
        if let Some(handle) = self.instrument.take() {
            handle.abort();
            let _ = handle.await;
        }
        if let Some(handle) = self.logcat.take() {
            handle.abort();
            let _ = handle.await;
        }
        if let Some(handle) = self.forward.take() {
            handle.abort();
            let _ = handle.await;
        }
        self.local_port = None;
        if let Some(client) = self.client.as_ref() {
            client.force_stop(AGENT_PACKAGE).await.ok();
        }
        Ok(())
    }

    fn log_sources(&self) -> Vec<LogSource> {
        let mut sources = vec![
            LogSource::new("instrument", LogSourceKind::Host),
            LogSource::new("logcat", LogSourceKind::Host),
        ];
        sources.push(LogSource::new("agent", LogSourceKind::Agent));
        sources
    }

    fn logs(&self) -> LogBuffer {
        self.logs.clone()
    }
}

/// Local agent APKs: env overrides first, then the Gradle debug outputs of
/// the `:agent` app (the instrumentation target), the `:agent-instrumentation`
/// test APK and the `:agent-target-stub` app under test.
fn resolve_android_apks() -> Vec<(String, PathBuf)> {
    let mut apks = Vec::new();
    for (label, env_key, module) in [
        ("agent", "IMPRESARIO_ANDROID_AGENT_APK", "agent"),
        (
            "instrumentation",
            "IMPRESARIO_ANDROID_AGENT_TEST_APK",
            "agent-instrumentation",
        ),
        ("stub", "IMPRESARIO_ANDROID_STUB_APK", "agent-target-stub"),
    ] {
        if let Ok(path) = std::env::var(env_key) {
            let path = PathBuf::from(path);
            if path.is_file() {
                apks.push((label.to_string(), path));
            }
            continue;
        }
        let candidate = PathBuf::from("agents/android")
            .join(module)
            .join("build/outputs/apk/debug")
            .join(format!("{module}-debug.apk"));
        if candidate.is_file() {
            apks.push((label.to_string(), candidate));
        }
    }
    apks
}

fn read_cache(path: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                map.insert(key.to_string(), value.to_string());
            }
        }
    }
    map
}

fn write_cache(path: &Path, map: &BTreeMap<String, String>) {
    let mut text = String::new();
    for (key, value) in map {
        let _ = writeln!(text, "{key}={value}");
    }
    let _ = std::fs::write(path, text);
}

// ---------------------------------------------------------------------------
// AppleTransport (ios, tvos)
// ---------------------------------------------------------------------------

/// Apple-family transport: macOS hosts only. iOS devices go through
/// `iproxy`; iOS simulators are reached directly on loopback; tvOS uses the
/// configured `agent_address` (network).
pub struct AppleTransport {
    runner: Option<ManagedChild>,
    iproxy: Option<ManagedChild>,
    endpoint: Option<String>,
    logs: LogBuffer,
}

impl Default for AppleTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl AppleTransport {
    #[must_use]
    pub fn new() -> Self {
        Self {
            runner: None,
            iproxy: None,
            endpoint: None,
            logs: LogBuffer::new(1000),
        }
    }

    fn start_xctestrun(&mut self, udid: &str, platform: Platform) -> Result<(), TransportError> {
        let xctestrun = std::env::var("IMPRESARIO_XCTESTRUN").map_err(|_| {
            TransportError::DeviceUnavailable(
                "set IMPRESARIO_XCTESTRUN to the .xctestrun file produced by the Apple agent \
                 build (docs/STATUS.md; the Apple build is verified on macOS only)"
                    .to_string(),
            )
        })?;
        let xcodebuild = find_in_path("xcodebuild").ok_or_else(|| {
            TransportError::DeviceUnavailable("xcodebuild not found on PATH".to_string())
        })?;
        let default_bundle = match platform {
            Platform::Tvos => "com.impresario.agent.AgentUITests-tvOS",
            _ => "com.impresario.agent.AgentUITests-iOS",
        };
        let bundle = std::env::var("IMPRESARIO_APPLE_TEST_BUNDLE")
            .unwrap_or_else(|_| default_bundle.to_string());
        let mut cmd = Command::new(xcodebuild);
        cmd.args([
            "test-without-building".to_string(),
            "-xctestrun".to_string(),
            xctestrun,
            "-destination".to_string(),
            format!("id={udid}"),
            "-only-testing".to_string(),
            format!("{bundle}/AgentRunnerTests/testServe"),
            "-test-timeouts-enabled".to_string(),
            "NO".to_string(),
            "-parallel-testing-enabled".to_string(),
            "NO".to_string(),
        ]);
        let mut child = ManagedChild::spawn("xcodebuild runner", &mut cmd)?;
        drain_child(&mut child, &self.logs, "xcodebuild");
        self.runner = Some(child);
        Ok(())
    }

    /// Tool that performs the clean-state reinstall for this target (I-84).
    async fn reset_tool(&self, udid: &str) -> AppleResetTool {
        if is_simulator_udid(udid) {
            return AppleResetTool::Simctl;
        }
        if find_in_path("xcrun").is_some() && self.devicectl_available().await {
            return AppleResetTool::Devicectl;
        }
        AppleResetTool::IdeviceInstaller
    }

    /// `xcrun devicectl` exists from Xcode 15 on; older hosts fall back to
    /// `ideviceinstaller`.
    async fn devicectl_available(&self) -> bool {
        let Some(xcrun) = find_in_path("xcrun") else {
            return false;
        };
        let mut cmd = Command::new(xcrun);
        cmd.args(["devicectl", "--version"]);
        matches!(
            command_output(
                &mut cmd,
                Duration::from_secs(30),
                "xcrun devicectl --version"
            )
            .await,
            Ok(output) if output.status.success()
        )
    }

    /// Uninstall + install the app under test (`options.app_path`) so the next
    /// launch starts from a clean container (I-84). Returns `NotRequested`
    /// when `clear_app_data` is off and `Unavailable` when the host has no way
    /// to reinstall the app (the agent then degrades to a cold start).
    async fn reset_app_data_for(
        &mut self,
        device: &DeviceConfig,
        app: Option<&AppConfig>,
    ) -> Result<ResetOutcome, TransportError> {
        let Some(app) = app else {
            return Ok(ResetOutcome::NotRequested);
        };
        if !app.options.clear_data {
            return Ok(ResetOutcome::NotRequested);
        }
        let Some(entry) = app.platform_with_fallback(device.platform) else {
            return Ok(ResetOutcome::Unavailable(format!(
                "app '{}' has no entry for platform {}",
                app.name, device.platform
            )));
        };
        let Some(app_id) = entry.id.as_deref() else {
            return Ok(ResetOutcome::Unavailable(format!(
                "app '{}' has no id for platform {}",
                app.name, device.platform
            )));
        };
        let Some(app_path) = entry.package.as_deref() else {
            return Ok(ResetOutcome::Unavailable(format!(
                "app '{}' has no package for platform {}; set package to a signed .app bundle \
                 or .ipa so the host can reinstall it, otherwise the agent only cold-starts it \
                 (docs/DEVICES.md section 3)",
                app.name, device.platform
            )));
        };
        let udid = device.target_endpoint();
        let tool = self.reset_tool(&udid).await;
        for step in apple_reset_commands(tool, &udid, app_id, app_path) {
            let program = find_in_path(&step.program).ok_or_else(|| {
                TransportError::DeviceUnavailable(format!(
                    "{} not found on PATH; install libimobiledevice with \
                     'brew install libimobiledevice'",
                    step.program
                ))
            })?;
            let mut cmd = Command::new(program);
            cmd.args(&step.args);
            let what = format!("{} {}", step.program, step.args.join(" "));
            tracing::debug!(command = %what, "resetting the app under test");
            let output = command_output(&mut cmd, APPLE_INSTALL_TIMEOUT, &what).await?;
            if output.status.success() {
                continue;
            }
            let text = output_text(&output).trim().to_string();
            if step.tolerate_failure {
                tracing::debug!(command = %what, %text, "tolerated reset failure");
                continue;
            }
            return Err(TransportError::Transport(format!(
                "{what} failed with {}: {text}",
                output.status
            )));
        }
        self.logs.push(LogRecord {
            unix_ms: unix_ms_now(),
            level: LogLevel::Info,
            source: "xcodebuild".to_string(),
            message: format!("reinstalled {app_id} from {app_path} for a clean state (I-84)"),
        });
        Ok(ResetOutcome::Reset)
    }
}

#[async_trait]
impl DeviceTransport for AppleTransport {
    async fn prepare(
        &mut self,
        device: &DeviceConfig,
        _app: Option<&AppConfig>,
    ) -> Result<Endpoint, TransportError> {
        if !cfg!(target_os = "macos") {
            return Err(TransportError::DeviceUnavailable(format!(
                "device '{}' ({}) needs a macOS host: iproxy/xcodebuild are not available on {}",
                device.name,
                device.platform,
                std::env::consts::OS
            )));
        }
        let udid = device.target_endpoint();
        let agent_port = device.effective_agent_port().unwrap_or(APPLE_AGENT_PORT);
        match device.platform {
            Platform::Tvos => {
                let address = device.connection.endpoint.as_deref().ok_or_else(|| {
                    TransportError::DeviceUnavailable(format!(
                        "tvOS device '{}' requires connection.endpoint (Bonjour discovery is not \
                         implemented, I-09)",
                        device.name
                    ))
                })?;
                self.endpoint = Some(address.to_string());
                Ok(Endpoint::new(address))
            }
            Platform::Ios if is_simulator_udid(&udid) => {
                self.start_xctestrun(&udid, device.platform)?;
                let address = format!("127.0.0.1:{agent_port}");
                wait_for_port(&address, start_timeout()).await?;
                self.endpoint = Some(address.clone());
                Ok(Endpoint::new(address))
            }
            Platform::Ios => {
                let iproxy = find_in_path("iproxy").ok_or_else(|| {
                    TransportError::DeviceUnavailable(
                        "iproxy (libimobiledevice) not found on PATH; install it with \
                         'brew install libimobiledevice'"
                            .to_string(),
                    )
                })?;
                let local_port = free_local_port().await?;
                let mut cmd = Command::new(iproxy);
                cmd.args([
                    format!("{local_port}:{agent_port}"),
                    "-u".to_string(),
                    udid.clone(),
                ]);
                let mut child = ManagedChild::spawn("iproxy", &mut cmd)?;
                drain_child(&mut child, &self.logs, "iproxy");
                self.iproxy = Some(child);
                let address = format!("127.0.0.1:{local_port}");
                wait_for_port(&address, start_timeout())
                    .await
                    .map_err(|e| {
                        TransportError::Transport(format!(
                            "iproxy did not open {address} for device {udid}: {e}"
                        ))
                    })?;
                self.start_xctestrun(&udid, device.platform)?;
                self.endpoint = Some(address.clone());
                Ok(Endpoint::new(address))
            }
            other => Err(TransportError::Transport(format!(
                "the Apple transport cannot serve platform {other}"
            ))),
        }
    }

    async fn reset_app_data(
        &mut self,
        device: &DeviceConfig,
        app: Option<&AppConfig>,
    ) -> Result<ResetOutcome, TransportError> {
        if !cfg!(target_os = "macos") {
            return Err(TransportError::DeviceUnavailable(format!(
                "device '{}' ({}) needs a macOS host to reset the app under test",
                device.name, device.platform
            )));
        }
        self.reset_app_data_for(device, app).await
    }

    async fn teardown(&mut self) -> Result<(), TransportError> {
        if let Some(mut runner) = self.runner.take() {
            runner.stop().await;
        }
        if let Some(mut iproxy) = self.iproxy.take() {
            iproxy.stop().await;
        }
        Ok(())
    }

    fn log_sources(&self) -> Vec<LogSource> {
        vec![
            LogSource::new("xcodebuild", LogSourceKind::Host),
            LogSource::new("agent", LogSourceKind::Agent),
        ]
    }

    fn logs(&self) -> LogBuffer {
        self.logs.clone()
    }
}

/// Simulator UDIDs are RFC 4122 UUIDs; device UDIDs are 40 hex characters or
/// `XXXXXXXX-XXXXXXXXXXXXXXXX`.
#[must_use]
pub fn is_simulator_udid(udid: &str) -> bool {
    let format = udid.split('-').map(str::len).collect::<Vec<_>>();
    if format == [8, 4, 4, 4, 12] {
        return udid.chars().all(|c| c == '-' || c.is_ascii_hexdigit());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_loopback_detection() {
        assert!(Endpoint::new("127.0.0.1:50051").is_loopback());
        assert!(Endpoint::new("localhost:50051").is_loopback());
        assert!(Endpoint::new("[::1]:50051").is_loopback());
        assert!(!Endpoint::new("192.168.1.50:9998").is_loopback());
        assert_eq!(Endpoint::new("a:1").address(), "a:1");
        assert_eq!(Endpoint::new("a:1").to_string(), "a:1");
    }

    #[test]
    fn logcat_lines_get_levels_and_epoch() {
        let record = parse_logcat_line("1697662800.123  1234  1234 I impresario: hello world");
        assert_eq!(record.level, LogLevel::Info);
        assert_eq!(record.unix_ms, 1_697_662_800_123);
        assert_eq!(record.source, "logcat");

        let record = parse_logcat_line("1697662801.000  1234  1234 E ActivityManager: crash");
        assert_eq!(record.level, LogLevel::Error);

        let record = parse_logcat_line("not a logcat line");
        assert_eq!(record.level, LogLevel::Debug);
    }

    #[test]
    fn simulator_udid_detection() {
        assert!(is_simulator_udid("9E3C7A6D-0B4B-4F1A-9C7E-2A6D5B8F4E1C"));
        assert!(!is_simulator_udid("00008110-001A2345021E001E"));
        assert!(!is_simulator_udid(
            "1234567890123456789012345678901234567890"
        ));
        assert!(!is_simulator_udid("9E3C7A6D0B4B4F1A9C7E2A6D5B8F4E1C"));
    }

    #[test]
    fn file_hash_changes_with_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        std::fs::write(&a, b"one").expect("write");
        std::fs::write(&b, b"two").expect("write");
        let ha = file_hash(&a).expect("hash");
        let hb = file_hash(&b).expect("hash");
        assert_ne!(ha, hb);
        assert_eq!(ha, file_hash(&a).expect("hash"));
    }

    #[test]
    fn sanitize_keeps_serials_readable() {
        assert_eq!(
            sanitize_for_filename("192.168.1.60:5555"),
            "192.168.1.60_5555"
        );
        assert_eq!(sanitize_for_filename("emulator-5554"), "emulator-5554");
    }

    #[test]
    fn transport_selection_by_family() {
        assert!(transport_for(Platform::WebOs).log_sources()[0].name == "connector");
        assert_eq!(transport_for(Platform::Android).log_sources().len(), 3);
        assert_eq!(
            transport_for(Platform::Tvos).log_sources()[0].name,
            "xcodebuild"
        );
    }

    fn apple_device(platform: Platform, udid: &str) -> DeviceConfig {
        DeviceConfig {
            name: "apple-device".to_string(),
            platform,
            connection: impresario_core::config::DeviceConnection {
                udid: Some(udid.to_string()),
                ..impresario_core::config::DeviceConnection::default()
            },
            auth: impresario_core::config::DeviceAuth::default(),
            labels: Vec::new(),
            options: impresario_core::config::DeviceOptions::default(),
        }
    }

    fn apple_app(clear_data: bool, package: Option<&str>) -> AppConfig {
        let mut platforms = std::collections::BTreeMap::new();
        platforms.insert(
            Platform::Ios,
            impresario_core::apps::AppPlatform {
                id: Some("com.example.app".to_string()),
                package: package.map(str::to_string),
                launch_activity: None,
                url: None,
            },
        );
        AppConfig {
            name: "example".to_string(),
            description: None,
            platforms,
            options: impresario_core::apps::AppOptions {
                clear_data,
                launch_params: std::collections::BTreeMap::new(),
            },
        }
    }

    #[test]
    fn apple_reset_plan_for_simulators() {
        let steps = apple_reset_commands(
            AppleResetTool::Simctl,
            "11111111-2222-3333-4444-555555555555",
            "com.example.app",
            "/tmp/Example.app",
        );
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].program, "xcrun");
        assert_eq!(
            steps[0].args,
            vec![
                "simctl",
                "uninstall",
                "11111111-2222-3333-4444-555555555555",
                "com.example.app"
            ]
        );
        assert!(steps[0].tolerate_failure, "uninstall must tolerate absence");
        assert_eq!(
            steps[1].args,
            vec![
                "simctl",
                "install",
                "11111111-2222-3333-4444-555555555555",
                "/tmp/Example.app"
            ]
        );
        assert!(!steps[1].tolerate_failure, "install failures are fatal");
    }

    #[test]
    fn apple_reset_plan_for_devices_and_fallback() {
        let steps = apple_reset_commands(
            AppleResetTool::Devicectl,
            "00008110-000123456789001E",
            "com.example.app",
            "/tmp/Example.ipa",
        );
        assert_eq!(
            steps[0].args,
            vec![
                "devicectl",
                "device",
                "uninstall",
                "app",
                "--device",
                "00008110-000123456789001E",
                "com.example.app"
            ]
        );
        assert_eq!(
            steps[1].args,
            vec![
                "devicectl",
                "device",
                "install",
                "app",
                "--device",
                "00008110-000123456789001E",
                "/tmp/Example.ipa"
            ]
        );

        let fallback = apple_reset_commands(
            AppleResetTool::IdeviceInstaller,
            "00008110-000123456789001E",
            "com.example.app",
            "/tmp/Example.ipa",
        );
        assert_eq!(fallback[0].program, "ideviceinstaller");
        assert_eq!(
            fallback[0].args,
            vec!["-u", "00008110-000123456789001E", "-U", "com.example.app"]
        );
        assert_eq!(
            fallback[1].args,
            vec!["-u", "00008110-000123456789001E", "-i", "/tmp/Example.ipa"]
        );
    }

    #[tokio::test]
    async fn apple_reset_is_a_noop_without_clear_app_data() {
        let mut transport = AppleTransport::new();
        let device = apple_device(Platform::Ios, "11111111-2222-3333-4444-555555555555");
        let app = apple_app(false, None);
        let outcome = transport
            .reset_app_data_for(&device, Some(&app))
            .await
            .expect("no-op reset");
        assert_eq!(outcome, ResetOutcome::NotRequested);
    }

    #[tokio::test]
    async fn apple_reset_reports_a_missing_app_path() {
        let mut transport = AppleTransport::new();
        let device = apple_device(Platform::Ios, "11111111-2222-3333-4444-555555555555");
        let app = apple_app(true, None);
        let outcome = transport
            .reset_app_data_for(&device, Some(&app))
            .await
            .expect("unavailable reset");
        match outcome {
            ResetOutcome::Unavailable(reason) => {
                assert!(reason.contains("package"), "{reason}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apple_reset_requires_a_macos_host() {
        if cfg!(target_os = "macos") {
            return;
        }
        let mut transport = AppleTransport::new();
        let device = apple_device(Platform::Tvos, "00008110-000123456789001E");
        let app = apple_app(true, Some("/tmp/Example.ipa"));
        let error = transport
            .reset_app_data(&device, Some(&app))
            .await
            .expect_err("host check");
        assert!(matches!(error, TransportError::DeviceUnavailable(_)));
    }
}
