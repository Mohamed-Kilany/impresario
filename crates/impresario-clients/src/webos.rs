//! Native webOS transport (TASK-2 6.3): SSH/SFTP, the developer key service,
//! luna calls and inspector port forwarding. No `ssh`, `scp`, `openssl`,
//! `luna-send` or `ares-*` process is started (D-19).
//!
//! The exact luna addresses and install parameters come from the Apache-2.0
//! `@webosose/ares-cli` sources (`files/conf/command-service.json`,
//! `lib/install.js`, `lib/launch.js`, `lib/inspect.js`). Hardware behaviour is
//! still unverified; see `docs/DEVICES.md` and HD-3/HD-7.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use impresario_core::apps::AppConfig;
use impresario_core::config::DeviceConfig;
use impresario_core::home::Home;
use impresario_core::platforms::Platform;
use russh::client;
use russh::keys::{
    decode_secret_key, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey,
    PublicKeyOrCertificate,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::agent_client::DeviceInfo;
use crate::log::{unix_ms_now, LogBuffer, LogLevel, LogRecord};
use crate::transport::{
    ConnectorTransport, DeviceTransport, Endpoint, LogSource, LogSourceKind, NativeProbe,
    ResetOutcome, TransportError,
};

const SSH_USER: &str = "prisoner";
const KEY_SERVICE_PORT: u16 = 9991;
const DEFAULT_INSPECTOR_PORT: u16 = 9998;
const REMOTE_TEMP_DIR: &str = "/media/developer/temp";
const EXEC_TIMEOUT: Duration = Duration::from_secs(30);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const KEYRING_SERVICE: &str = "impresario.webos";
const MAX_KEY_BYTES: usize = 1024 * 1024;

/// webOS-specific failures. Every path that claims success performs the
/// protocol work first.
#[derive(Debug, thiserror::Error)]
pub enum WebOsError {
    #[error("webOS host is not configured (connection.host)")]
    NoHost,
    #[error("webOS app is required for a native run")]
    NoApp,
    #[error("webOS app has no id for this platform")]
    NoAppId,
    #[error("webOS SSH: {0}")]
    Ssh(String),
    #[error("webOS key service: {0}")]
    KeyService(String),
    #[error("webOS key passphrase: {0}")]
    Passphrase(String),
    #[error("webOS luna call failed: {0}")]
    Luna(String),
    #[error("webOS package: {0}")]
    Package(String),
    #[error("webOS {0} did not finish within {1:?}")]
    Timeout(&'static str, Duration),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
}

impl From<WebOsError> for TransportError {
    fn from(value: WebOsError) -> Self {
        match value {
            WebOsError::NoHost
            | WebOsError::NoApp
            | WebOsError::NoAppId
            | WebOsError::KeyService(_)
            | WebOsError::Passphrase(_) => TransportError::DeviceUnavailable(value.to_string()),
            other => TransportError::Transport(other.to_string()),
        }
    }
}

/// Outcome of one SSH exec channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: Option<u32>,
}

impl ExecOutput {
    #[must_use]
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    #[must_use]
    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

struct HostKeyHandler {
    expected: ExpectedHostKey,
    home: Home,
    host: String,
    port: u16,
}

#[derive(Clone)]
enum ExpectedHostKey {
    /// `SHA256:...` fingerprint or an OpenSSH public-key file.
    Pinned(Pin),
    /// Trust on first use, persisted as a fingerprint. A later mismatch is a
    /// hard failure.
    KnownHosts,
}

#[derive(Clone)]
enum Pin {
    Fingerprint(String),
    PublicKey(PublicKey),
}

impl client::Handler for HostKeyHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = server_public_key.public_key();
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        match &self.expected {
            ExpectedHostKey::Pinned(Pin::Fingerprint(expected)) => {
                let ok = constant_time_eq(expected.as_bytes(), fingerprint.as_bytes());
                if !ok {
                    tracing::warn!(host = %self.host, %fingerprint, expected = %expected, "webOS host key mismatch");
                }
                Ok(ok)
            }
            ExpectedHostKey::Pinned(Pin::PublicKey(expected)) => {
                Ok(expected.key_data() == key.key_data())
            }
            ExpectedHostKey::KnownHosts => {
                match known_host_fingerprint(&self.home, &self.host, self.port) {
                    Some(expected) => {
                        let ok = constant_time_eq(expected.as_bytes(), fingerprint.as_bytes());
                        if !ok {
                            tracing::warn!(host = %self.host, %fingerprint, expected = %expected, "webOS known-host key changed");
                        }
                        Ok(ok)
                    }
                    None => {
                        if remember_host_key(&self.home, &self.host, self.port, &fingerprint)
                            .is_err()
                        {
                            return Ok(false);
                        }
                        tracing::info!(host = %self.host, %fingerprint, "webOS host key pinned on first use");
                        Ok(true)
                    }
                }
            }
        }
    }
}

/// A connected webOS SSH session. Clones of the handle can open new channels
/// for the forwarding task.
pub struct WebOsSession {
    handle: Arc<Mutex<client::Handle<HostKeyHandler>>>,
}

impl WebOsSession {
    /// Connect, verify the host key and authenticate with the configured or
    /// developer-mode key.
    pub async fn connect(device: &DeviceConfig, home: &Home) -> Result<Self, WebOsError> {
        let host = device
            .connection
            .host
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(WebOsError::NoHost)?;
        let port = device
            .connection
            .port_or_default(device.platform)
            .unwrap_or(9922);
        let expected = expected_host_key(device, home)?;
        let key = load_or_fetch_key(device, home, host).await?;
        let config = Arc::new(client::Config {
            nodelay: true,
            inactivity_timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        });
        let handler = HostKeyHandler {
            expected,
            home: home.clone(),
            host: host.to_string(),
            port,
        };
        let mut session = timeout(
            Duration::from_secs(15),
            client::connect(config, (host, port), handler),
        )
        .await
        .map_err(|_| WebOsError::Timeout("SSH connect", Duration::from_secs(15)))?
        .map_err(|error| WebOsError::Ssh(format!("connect {host}:{port}: {error}")))?;

        let hash = session
            .best_supported_rsa_hash()
            .await
            .map_err(|error| WebOsError::Ssh(format!("negotiate RSA hash: {error}")))?
            .flatten();
        let auth = session
            .authenticate_publickey(SSH_USER, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
            .map_err(|error| WebOsError::Ssh(format!("authenticate: {error}")))?;
        if !auth.success() {
            return Err(WebOsError::Ssh(
                "the TV rejected the developer key; accept the prompt on the TV and retry".into(),
            ));
        }
        Ok(Self {
            handle: Arc::new(Mutex::new(session)),
        })
    }

    /// Run one command and collect stdout/stderr and the exit status.
    pub async fn exec(&self, command: &str) -> Result<ExecOutput, WebOsError> {
        self.exec_timeout(command, EXEC_TIMEOUT).await
    }

    /// Run one command with an explicit deadline.
    pub async fn exec_timeout(
        &self,
        command: &str,
        deadline: Duration,
    ) -> Result<ExecOutput, WebOsError> {
        let future = async {
            let mut channel = self
                .handle
                .lock()
                .await
                .channel_open_session()
                .await
                .map_err(|error| WebOsError::Ssh(format!("open exec channel: {error}")))?;
            channel
                .exec(true, command.as_bytes())
                .await
                .map_err(|error| WebOsError::Ssh(format!("exec: {error}")))?;
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut exit_status = None;
            while let Some(message) = channel.wait().await {
                match message {
                    russh::ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                    russh::ChannelMsg::ExtendedData { data, ext: 1 } => {
                        stderr.extend_from_slice(&data);
                    }
                    russh::ChannelMsg::ExitStatus { exit_status: code } => {
                        exit_status = Some(code);
                    }
                    russh::ChannelMsg::Close => break,
                    _ => {}
                }
            }
            Ok(ExecOutput {
                stdout,
                stderr,
                exit_status,
            })
        };
        timeout(deadline, future)
            .await
            .map_err(|_| WebOsError::Timeout("command", deadline))?
    }

    /// Upload one file through the SFTP subsystem.
    pub async fn upload(&self, remote_path: &str, bytes: &[u8]) -> Result<(), WebOsError> {
        let channel = self
            .handle
            .lock()
            .await
            .channel_open_session()
            .await
            .map_err(|error| WebOsError::Ssh(format!("open SFTP channel: {error}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| WebOsError::Ssh(format!("request sftp: {error}")))?;
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|error| WebOsError::Ssh(format!("start SFTP: {error}")))?;
        sftp.write(remote_path, bytes)
            .await
            .map_err(|error| WebOsError::Ssh(format!("upload {remote_path}: {error}")))?;
        let _ = sftp.close().await;
        Ok(())
    }

    /// Call one luna method and return the parsed JSON response.
    pub async fn luna(
        &self,
        service: &str,
        folder: Option<&str>,
        method: &str,
        payload: &str,
    ) -> Result<serde_json::Value, WebOsError> {
        let url = luna_url(service, folder, method);
        let command = format!("luna-send-pub -n 1 -f {url} {}", shell_quote(payload));
        let output = self.exec(&command).await?;
        parse_luna_response(&output, &url)
    }

    /// Install a local `.ipk` after uploading it to the device temp directory.
    ///
    /// `luna-send-pub -i` keeps its subscription open after the last progress
    /// line, so this method streams replies until a terminal state, closes the
    /// channel, then verifies the app through `listApps`. A first "installing"
    /// reply is never reported as success.
    pub async fn install(&self, app_id: &str, local_ipk: &Path) -> Result<(), WebOsError> {
        let bytes = std::fs::read(local_ipk).map_err(|error| {
            WebOsError::Package(format!("read {}: {error}", local_ipk.display()))
        })?;
        // Keep the remote name ASCII and bounded; the local cache name contains
        // a hash, which is unnecessary on the TV.
        let remote = format!(
            "{REMOTE_TEMP_DIR}/impresario-{}.ipk",
            safe_component(app_id)
        );
        self.upload(&remote, &bytes).await?;
        let was_installed = self
            .list_apps()
            .await
            .ok()
            .map(|apps| apps.iter().any(|id| id == app_id));
        let payload = serde_json::json!({
            "id": app_id,
            "ipkUrl": remote,
            "subscribe": true,
        })
        .to_string();
        let url = luna_url("com.webos.appInstallService", Some("dev"), "install");
        let command = format!("luna-send-pub -i -f {url} {}", shell_quote(&payload));
        let result = self
            .install_subscription(&command, app_id, was_installed)
            .await;
        // The reference installer removes its temporary upload; this is not a
        // reason to turn a successful install into a failure.
        if let Err(error) = self.exec(&format!("rm -f {}", shell_quote(&remote))).await {
            tracing::debug!(%error, remote, "webOS package cleanup failed");
        }
        result
    }

    async fn install_subscription(
        &self,
        command: &str,
        app_id: &str,
        was_installed: Option<bool>,
    ) -> Result<(), WebOsError> {
        let future = async {
            let mut channel = self
                .handle
                .lock()
                .await
                .channel_open_session()
                .await
                .map_err(|error| WebOsError::Ssh(format!("open install channel: {error}")))?;
            channel
                .exec(true, command.as_bytes())
                .await
                .map_err(|error| WebOsError::Ssh(format!("exec install: {error}")))?;
            let deadline = tokio::time::Instant::now() + INSTALL_TIMEOUT;
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut pending = String::new();
            let mut terminal = None;
            loop {
                let message = tokio::time::timeout_at(deadline, channel.wait()).await;
                let message =
                    message.map_err(|_| WebOsError::Timeout("install", INSTALL_TIMEOUT))?;
                let Some(message) = message else {
                    break;
                };
                match message {
                    russh::ChannelMsg::Data { data } => {
                        stdout.extend_from_slice(&data);
                        pending.push_str(&String::from_utf8_lossy(&data));
                        while let Some(index) = pending.find('\n') {
                            let line = pending[..index].trim().to_string();
                            pending.drain(..=index);
                            if let Some(found) = parse_install_line(&line) {
                                terminal = Some(found);
                                break;
                            }
                        }
                        if terminal.is_some() {
                            break;
                        }
                    }
                    russh::ChannelMsg::ExtendedData { data, ext: 1 } => {
                        stderr.extend_from_slice(&data);
                    }
                    russh::ChannelMsg::ExitStatus { exit_status } => {
                        if exit_status != 0 && terminal.is_none() {
                            // luna-send can exit non-zero after printing an
                            // error object; parse the output below for detail.
                            break;
                        }
                    }
                    russh::ChannelMsg::Close => break,
                    _ => {}
                }
            }
            if terminal.is_none() && !pending.trim().is_empty() {
                terminal = parse_install_line(pending.trim());
            }
            // The subscription is the reason `-i` stays open; closing the
            // channel asks the remote luna client to stop cleanly.
            let _ = channel.close().await;
            Ok::<_, WebOsError>((terminal, stdout, stderr))
        };
        let (terminal, stdout, stderr) = future.await?;
        match terminal {
            Some(InstallTerminal::Failed(message)) => {
                return Err(WebOsError::Luna(format!("install failed: {message}")));
            }
            Some(InstallTerminal::Installed) => {}
            None => {
                if was_installed != Some(false) {
                    let text = String::from_utf8_lossy(&stdout);
                    let reason = if was_installed == Some(true) {
                        "the app was already installed, so its presence is not proof"
                    } else {
                        "the pre-install app list was unavailable, so its presence is not proof"
                    };
                    return Err(WebOsError::Luna(format!(
                        "install subscription ended without a terminal state ({reason}); \
                         output: {}",
                        concise(&text, &stderr)
                    )));
                }
            }
        }
        // Verify through the app manager before reporting success. Updates can
        // take a moment to appear; retry briefly.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self
                .list_apps()
                .await
                .map(|apps| apps.iter().any(|id| id == app_id))
                .unwrap_or(false)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(WebOsError::Luna(format!(
                    "install did not make '{app_id}' appear in listApps"
                )));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Wait until an SSH `direct-tcpip` connection to the inspector succeeds.
    pub async fn wait_for_remote_port(&self, port: u16, wait: Duration) -> Result<(), WebOsError> {
        let deadline = Instant::now() + wait;
        loop {
            let opened = self
                .handle
                .lock()
                .await
                .channel_open_direct_tcpip("127.0.0.1", u32::from(port), "127.0.0.1", 0)
                .await;
            if let Ok(channel) = opened {
                let _ = channel.close().await;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(WebOsError::Timeout("inspector readiness", wait));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Installed application ids.
    pub async fn list_apps(&self) -> Result<Vec<String>, WebOsError> {
        let value = self
            .luna(
                "com.webos.applicationManager",
                Some("dev"),
                "listApps",
                r#"{"subscribe":false}"#,
            )
            .await?;
        require_luna_success(&value, "listApps")?;
        let apps = value
            .get("apps")
            .and_then(|value| value.as_array())
            .ok_or_else(|| WebOsError::Luna("listApps response has no apps array".to_string()))?;
        let mut ids = Vec::with_capacity(apps.len());
        for app in apps {
            let id = app
                .get("id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| WebOsError::Luna("listApps entry has no id".to_string()))?;
            ids.push(id.to_string());
        }
        Ok(ids)
    }

    /// Launch an app by id.
    pub async fn launch(&self, app_id: &str) -> Result<(), WebOsError> {
        let value = self
            .luna(
                "com.webos.applicationManager",
                None,
                "launch",
                &format!(r#"{{"id":{}}}"#, json_string(app_id)),
            )
            .await?;
        require_luna_success(&value, "launch")
    }

    /// Close an app (best effort; "not running" is not a failure).
    pub async fn close(&self, app_id: &str) -> Result<(), WebOsError> {
        let value = self
            .luna(
                "com.webos.applicationManager",
                Some("dev"),
                "closeByAppId",
                &format!(r#"{{"id":{}}}"#, json_string(app_id)),
            )
            .await?;
        match value
            .get("returnValue")
            .and_then(serde_json::Value::as_bool)
        {
            Some(true) => Ok(()),
            Some(false) => {
                let text = luna_error_text(&value);
                if text.to_ascii_lowercase().contains("not running") {
                    Ok(())
                } else {
                    Err(WebOsError::Luna(text))
                }
            }
            None => Err(WebOsError::Luna(
                "closeByAppId response has no returnValue".to_string(),
            )),
        }
    }

    /// A blocked device info query used by `device check`/`doctor`.
    pub async fn device_info(&self) -> Result<serde_json::Value, WebOsError> {
        let value = self
            .luna(
                "com.webos.service.systemservice",
                Some("deviceInfo"),
                "query",
                "{}",
            )
            .await?;
        require_luna_success(&value, "deviceInfo/query")?;
        Ok(value)
    }

    /// Forward a remote TCP port to a fresh loopback listener. Each accepted
    /// host connection opens a `direct-tcpip` channel.
    pub async fn forward(&self, remote_port: u16) -> Result<Forward, WebOsError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let handle = Arc::clone(&self.handle);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, originator)) = listener.accept().await else {
                    break;
                };
                let handle = Arc::clone(&handle);
                tokio::spawn(async move {
                    let opened = handle
                        .lock()
                        .await
                        .channel_open_direct_tcpip(
                            "127.0.0.1",
                            u32::from(remote_port),
                            originator.ip().to_string(),
                            u32::from(originator.port()),
                        )
                        .await;
                    match opened {
                        Ok(channel) => {
                            let mut channel = channel.into_stream();
                            let mut stream = stream;
                            let _ = tokio::io::copy_bidirectional(&mut stream, &mut channel).await;
                        }
                        Err(error) => {
                            tracing::warn!(%error, remote_port, "webOS direct-tcpip failed");
                        }
                    }
                });
            }
        });
        Ok(Forward {
            local_port: port,
            task,
        })
    }

    pub async fn disconnect(&self) {
        let _ = self
            .handle
            .lock()
            .await
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

/// Handle for the local end of an SSH port forward.
pub struct Forward {
    pub local_port: u16,
    task: JoinHandle<()>,
}

impl Forward {
    pub fn stop(self) {
        self.task.abort();
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The webOS transport. It owns the connector process (rung 1 sidecar),
/// an SSH session, one inspector forward and the running app id.
pub struct WebOsTransport {
    connector: ConnectorTransport,
    session: Option<WebOsSession>,
    forward: Option<Forward>,
    target: Option<String>,
    app_id: Option<String>,
    home: Option<Home>,
    logs: LogBuffer,
}

impl Default for WebOsTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl WebOsTransport {
    #[must_use]
    pub fn new() -> Self {
        Self {
            connector: ConnectorTransport::new(),
            session: None,
            forward: None,
            target: None,
            app_id: None,
            home: None,
            logs: LogBuffer::new(1000),
        }
    }

    fn push_log(&self, level: LogLevel, message: impl Into<String>) {
        self.logs.push(LogRecord {
            unix_ms: unix_ms_now(),
            level,
            source: "webos".to_string(),
            message: message.into(),
        });
    }

    /// Read-only access to the connected session for `device check` and the
    /// signing/doctor commands. The transport must be prepared first.
    #[must_use]
    pub fn session(&self) -> Option<&WebOsSession> {
        self.session.as_ref()
    }

    /// SSH/luna probe used by `devices check` and `doctor`. It sends only
    /// read-only queries and never installs or launches an app.
    pub async fn probe_device(&mut self, device: &DeviceConfig) -> Result<NativeProbe, WebOsError> {
        if device.platform != Platform::WebOs {
            return Err(WebOsError::Ssh(format!(
                "WebOsTransport cannot probe {}",
                device.platform
            )));
        }
        let home = Home::resolve()
            .map_err(|error| WebOsError::Ssh(format!("resolve IMPRESARIO home: {error}")))?;
        home.ensure_dirs()
            .map_err(|error| WebOsError::Ssh(format!("initialise IMPRESARIO home: {error}")))?;
        self.home = Some(home.clone());
        let session = WebOsSession::connect(device, &home).await?;
        let host = device.connection.host.clone().unwrap_or_default();
        let port = device
            .connection
            .port_or_default(Platform::WebOs)
            .unwrap_or(9922);
        let mut details = vec![
            format!("SSH {SSH_USER}@{host}:{port}"),
            if device
                .auth
                .identity
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                "host key pinned by auth.identity".to_string()
            } else {
                "host key trust-on-first-use in keys/webos_known_hosts".to_string()
            },
        ];
        let mut warnings = Vec::new();
        let info_value = match session.device_info().await {
            Ok(value) => Some(value),
            Err(error) => {
                warnings.push(format!("deviceInfo query failed: {error}"));
                None
            }
        };
        match session.list_apps().await {
            Ok(apps) => details.push(format!("{} app(s) installed", apps.len())),
            Err(error) => warnings.push(format!("listApps failed: {error}")),
        }
        let model = info_value
            .as_ref()
            .and_then(|value| device_info_string(value, &["modelName", "model", "productName"]))
            .unwrap_or("webOS TV")
            .to_string();
        let os_version = info_value
            .as_ref()
            .and_then(|value| {
                device_info_string(
                    value,
                    &["webOSVersion", "version", "productVersion", "sdkVersion"],
                )
            })
            .unwrap_or("unknown")
            .to_string();
        let width = info_value
            .as_ref()
            .and_then(|value| device_info_u32(value, &["screenWidth", "displayWidth"]))
            .unwrap_or(0);
        let height = info_value
            .as_ref()
            .and_then(|value| device_info_u32(value, &["screenHeight", "displayHeight"]))
            .unwrap_or(0);
        if let Some(value) = info_value.as_ref() {
            for key in ["firmwareVersion", "serialNumber"] {
                if let Some(found) = device_info_string(value, &[key]) {
                    details.push(format!("{key} {found}"));
                }
            }
        }
        self.session = Some(session);
        Ok(NativeProbe {
            info: DeviceInfo {
                model,
                os_name: "webOS".to_string(),
                os_version,
                screen_width_px: width,
                screen_height_px: height,
                pixel_ratio: 1.0,
                runtime: "SSH/luna (native)".to_string(),
            },
            details,
            warnings,
        })
    }
}

#[async_trait]
impl DeviceTransport for WebOsTransport {
    async fn prepare(
        &mut self,
        device: &DeviceConfig,
        app: Option<&AppConfig>,
    ) -> Result<Endpoint, TransportError> {
        if device.platform != Platform::WebOs {
            return Err(TransportError::Transport(format!(
                "WebOsTransport cannot prepare {}",
                device.platform
            )));
        }
        let app = app.ok_or_else(|| TransportError::from(WebOsError::NoApp))?;
        let entry = app
            .platform(Platform::WebOs)
            .ok_or_else(|| TransportError::from(WebOsError::NoApp))?;
        let app_id = entry
            .id
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| TransportError::from(WebOsError::NoAppId))?
            .to_string();
        let home = Home::resolve().map_err(|error| {
            TransportError::Transport(format!("cannot resolve IMPRESARIO home: {error}"))
        })?;
        home.ensure_dirs().map_err(|error| {
            TransportError::Transport(format!("cannot initialise IMPRESARIO home: {error}"))
        })?;
        self.home = Some(home.clone());
        self.app_id = Some(app_id.clone());

        // The connector is the executor for webOS (D-03). It listens on the
        // configured loopback address; the handshake target is the SSH
        // forward, supplied by target_endpoint below.
        let endpoint = self.connector.prepare(device, Some(app)).await?;
        let session = WebOsSession::connect(device, &home)
            .await
            .map_err(TransportError::from)?;
        self.push_log(LogLevel::Info, "SSH connected");
        if let Some(package) = entry.package.as_deref().filter(|value| !value.is_empty()) {
            let source = PathBuf::from(package);
            let cache = home.cache_dir().join("webos-packages");
            let path = crate::webos_package::prepare_package(&source, &cache).map_err(|error| {
                self.push_log(LogLevel::Error, error.to_string());
                TransportError::from(WebOsError::Package(error.to_string()))
            })?;
            self.push_log(
                LogLevel::Info,
                format!("installing {} (from {})", path.display(), source.display()),
            );
            session.install(&app_id, &path).await.map_err(|error| {
                self.push_log(LogLevel::Error, error.to_string());
                TransportError::from(error)
            })?;
            self.push_log(LogLevel::Info, format!("installed {app_id}"));
        }
        if let Err(first) = session.launch(&app_id).await {
            // A running app may refuse a second launch; close and retry once.
            session.close(&app_id).await.map_err(TransportError::from)?;
            session.launch(&app_id).await.map_err(|error| {
                let _ = first;
                TransportError::from(error)
            })?;
        }
        self.push_log(LogLevel::Info, format!("launched {app_id}"));
        let inspector_port = inspector_port(device);
        let readiness = inspector_readiness_timeout();
        session
            .wait_for_remote_port(inspector_port, readiness)
            .await
            .map_err(|error| {
                self.push_log(LogLevel::Error, error.to_string());
                TransportError::from(error)
            })?;
        let forward = session
            .forward(inspector_port)
            .await
            .map_err(TransportError::from)?;
        self.target = Some(format!("ws://127.0.0.1:{}", forward.local_port));
        self.push_log(
            LogLevel::Info,
            format!(
                "inspector 127.0.0.1:{inspector_port} forwarded to 127.0.0.1:{}",
                forward.local_port
            ),
        );
        self.forward = Some(forward);
        self.session = Some(session);
        Ok(endpoint)
    }

    async fn probe(
        &mut self,
        device: &DeviceConfig,
    ) -> Result<Option<NativeProbe>, TransportError> {
        self.probe_device(device).await.map(Some).map_err(|error| {
            self.push_log(LogLevel::Error, error.to_string());
            TransportError::from(error)
        })
    }

    async fn reset_app_data(
        &mut self,
        _device: &DeviceConfig,
        _app: Option<&AppConfig>,
    ) -> Result<ResetOutcome, TransportError> {
        // webOS has no host-side data wipe; the app is reinstalled by prepare
        // when a package is configured.
        Ok(ResetOutcome::NotRequested)
    }

    fn target_endpoint(&self, device: &DeviceConfig) -> String {
        self.target
            .clone()
            .unwrap_or_else(|| device.target_endpoint())
    }

    async fn teardown(&mut self) -> Result<(), TransportError> {
        if let Some(forward) = self.forward.take() {
            forward.stop();
        }
        let app_id = self.app_id.take();
        if let Some(session) = self.session.take() {
            if let Some(app_id) = app_id.as_deref() {
                if let Err(error) = session.close(app_id).await {
                    self.push_log(
                        LogLevel::Warn,
                        format!("could not close {app_id} during teardown: {error}"),
                    );
                }
            }
            session.disconnect().await;
        }
        self.target = None;
        self.connector.teardown().await?;
        Ok(())
    }

    fn log_sources(&self) -> Vec<LogSource> {
        let mut sources = self.connector.log_sources();
        sources.push(LogSource::new("webos", LogSourceKind::Host));
        sources
    }

    fn logs(&self) -> LogBuffer {
        let connector = self.connector.logs();
        let all = LogBuffer::new(2000);
        for record in self.logs.snapshot().into_iter().chain(connector.snapshot()) {
            all.push(record);
        }
        all
    }
}

// ---------------------------------------------------------------------------
// key retrieval/decryption, host pinning, luna parsing
// ---------------------------------------------------------------------------

fn expected_host_key(device: &DeviceConfig, home: &Home) -> Result<ExpectedHostKey, WebOsError> {
    let Some(value) = device
        .auth
        .identity
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return Ok(ExpectedHostKey::KnownHosts);
    };
    if let Some(fingerprint) = value.strip_prefix("SHA256:") {
        return Ok(ExpectedHostKey::Pinned(Pin::Fingerprint(format!(
            "SHA256:{fingerprint}"
        ))));
    }
    if value.starts_with("ssh-") || value.starts_with("ecdsa-") {
        let key = russh::keys::ssh_key::PublicKey::from_openssh(value.trim())
            .map_err(|error| WebOsError::Ssh(format!("parse pinned public key: {error}")))?;
        return Ok(ExpectedHostKey::Pinned(Pin::PublicKey(key)));
    }
    let path = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        home.keys_dir().join(value)
    };
    let text = std::fs::read_to_string(&path).map_err(|error| {
        WebOsError::Ssh(format!("read pinned host key {}: {error}", path.display()))
    })?;
    let key = russh::keys::ssh_key::PublicKey::from_openssh(text.trim())
        .map_err(|error| WebOsError::Ssh(format!("parse {}: {error}", path.display())))?;
    Ok(ExpectedHostKey::Pinned(Pin::PublicKey(key)))
}

fn known_hosts_path(home: &Home) -> PathBuf {
    home.keys_dir().join("webos_known_hosts")
}

fn known_host_fingerprint(home: &Home, host: &str, port: u16) -> Option<String> {
    let text = std::fs::read_to_string(known_hosts_path(home)).ok()?;
    let prefix = format!("[{host}]:{port} ");
    text.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(|value| value.trim().to_string())
    })
}

fn remember_host_key(
    home: &Home,
    host: &str,
    port: u16,
    fingerprint: &str,
) -> Result<(), std::io::Error> {
    let path = known_hosts_path(home);
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("[{host}]:{port} {fingerprint}\n"));
    std::fs::write(&path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(metadata) = std::fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            let _ = std::fs::set_permissions(&path, permissions);
        }
    }
    Ok(())
}

async fn load_or_fetch_key(
    device: &DeviceConfig,
    home: &Home,
    host: &str,
) -> Result<PrivateKey, WebOsError> {
    let configured = device.auth.key.as_deref().filter(|value| !value.is_empty());
    let cached = home
        .keys_dir()
        .join("webos")
        .join(format!("{}.pem", sanitize_file_component(host)));
    let (path, text) = if let Some(reference) = configured {
        if let Some(entry) = reference.strip_prefix("keychain:") {
            let password = keyring_get("impresario.keys", entry).map_err(|error| {
                WebOsError::Passphrase(format!("keychain entry '{entry}': {error}"))
            })?;
            return decode_secret_key(&password, None).map_err(|error| {
                WebOsError::Passphrase(format!("parse key from keychain field: {error}"))
            });
        }
        let path = PathBuf::from(reference);
        let text = std::fs::read_to_string(&path).map_err(|error| {
            WebOsError::Ssh(format!("read SSH key {}: {error}", path.display()))
        })?;
        (path, text)
    } else if cached.is_file() {
        let text = std::fs::read_to_string(&cached)?;
        (cached, text)
    } else {
        let text = fetch_dev_key(host).await?;
        if let Some(parent) = cached.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&cached, &text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            if let Ok(metadata) = std::fs::metadata(&cached) {
                let mut permissions = metadata.permissions();
                permissions.set_mode(0o600);
                let _ = std::fs::set_permissions(&cached, permissions);
            }
        }
        (cached, text)
    };

    let encrypted = text.contains("ENCRYPTED") || text.contains("DEK-Info:");
    let passphrase = if encrypted {
        Some(webos_passphrase(host)?)
    } else {
        None
    };
    if let Ok(key) = decode_secret_key(&text, passphrase.as_deref()) {
        return Ok(key);
    }
    // russh's OpenSSL PEM reader rejects some padded base64 bodies; try the
    // same input without padding before falling back to the DEK decipher.
    let stripped = strip_pem_base64_padding(&text);
    if let Ok(key) = decode_secret_key(&stripped, passphrase.as_deref()) {
        return Ok(key);
    }
    let passphrase = passphrase.ok_or_else(|| {
        WebOsError::Passphrase(format!(
            "{} is encrypted but no passphrase is available",
            path.display()
        ))
    })?;
    let der = decrypt_legacy_pem(&text, &passphrase)?;
    private_key_from_pkcs1_der(&der).map_err(|error| WebOsError::Passphrase(error.to_string()))
}

fn webos_passphrase(host: &str) -> Result<String, WebOsError> {
    if let Ok(value) = std::env::var("IMPRESARIO_WEBOS_PASSPHRASE") {
        if !value.is_empty() {
            return Ok(value);
        }
    }
    keyring_get(KEYRING_SERVICE, host).map_err(|error| {
        WebOsError::Passphrase(format!(
            "no passphrase in the OS keychain ({error}); set IMPRESARIO_WEBOS_PASSPHRASE \
             or store it with the webOS key setup command"
        ))
    })
}

fn keyring_get(service: &str, user: &str) -> Result<String, keyring::Error> {
    keyring::Entry::new(service, user)?.get_password()
}

/// Store the six-character TV passphrase in the OS keychain. Used by the
/// device command and documented in docs/SIGNING.md; never written to YAML.
pub fn store_webos_passphrase(host: &str, passphrase: &str) -> Result<(), keyring::Error> {
    keyring::Entry::new(KEYRING_SERVICE, host)?.set_password(passphrase)
}

/// Retrieve the developer-mode key over the TV's plain HTTP key service.
pub async fn fetch_dev_key(host: &str) -> Result<String, WebOsError> {
    let mut stream = timeout(
        Duration::from_secs(10),
        TcpStream::connect((host, KEY_SERVICE_PORT)),
    )
    .await
    .map_err(|_| WebOsError::Timeout("developer key service connect", Duration::from_secs(10)))?
    .map_err(|error| WebOsError::KeyService(format!("{host}:{KEY_SERVICE_PORT}: {error}")))?;
    let request = format!(
        "GET /webos_rsa HTTP/1.1\r\nHost: {host}:{KEY_SERVICE_PORT}\r\n\
         Connection: close\r\nUser-Agent: impresario\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if response.len() > MAX_KEY_BYTES {
            return Err(WebOsError::KeyService(
                "key response exceeds 1 MiB".to_string(),
            ));
        }
    }
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| WebOsError::KeyService("malformed HTTP response".to_string()))?;
    let headers = String::from_utf8_lossy(response.get(..split).unwrap_or_default());
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    if status != 200 {
        return Err(WebOsError::KeyService(format!(
            "key service returned HTTP {status}; enable Developer Mode and accept the key request on the TV"
        )));
    }
    let body = response.get(split.saturating_add(4)..).unwrap_or_default();
    let body = if headers.lines().any(|line| {
        line.to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
    }) {
        decode_chunked(body)?
    } else {
        body.to_vec()
    };
    let text = String::from_utf8(body)
        .map_err(|_| WebOsError::KeyService("key response is not UTF-8 PEM".to_string()))?;
    if !text.contains("-----BEGIN") {
        return Err(WebOsError::KeyService(
            "response is not a PEM private key".to_string(),
        ));
    }
    Ok(text)
}

fn decode_chunked(body: &[u8]) -> Result<Vec<u8>, WebOsError> {
    let mut output = Vec::new();
    let mut rest = body;
    loop {
        let end = rest
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| WebOsError::KeyService("malformed chunked body".to_string()))?;
        let size_text = String::from_utf8_lossy(rest.get(..end).unwrap_or_default());
        let size_text = size_text.split(';').next().unwrap_or("0").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| WebOsError::KeyService("bad chunk size".to_string()))?;
        rest = rest
            .get(end.saturating_add(2)..)
            .ok_or_else(|| WebOsError::KeyService("malformed chunked body".to_string()))?;
        if size == 0 {
            break;
        }
        let after = size
            .checked_add(2)
            .ok_or_else(|| WebOsError::KeyService("bad chunk size".to_string()))?;
        if rest.len() < after {
            return Err(WebOsError::KeyService("truncated chunk".to_string()));
        }
        output.extend_from_slice(rest.get(..size).unwrap_or_default());
        rest = rest
            .get(after..)
            .ok_or_else(|| WebOsError::KeyService("truncated chunk".to_string()))?;
    }
    Ok(output)
}

fn strip_pem_base64_padding(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for line in text.lines() {
        if line.starts_with("-----") || line.contains(':') || line.is_empty() {
            output.push_str(line);
            output.push('\n');
        } else {
            output.push_str(line.trim_end_matches('='));
            output.push('\n');
        }
    }
    output
}

/// Decrypt an OpenSSL legacy encrypted PEM (`Proc-Type: 4,ENCRYPTED`,
/// `DEK-Info: AES-128/192/256-CBC,<iv>`). The key is the OpenSSL EVP
/// BytesToKey MD5 derivation, matching ssh2-streams/ares-cli.
fn decrypt_legacy_pem(pem: &str, passphrase: &str) -> Result<Vec<u8>, WebOsError> {
    use md5::Digest as _;

    let mut cipher = None;
    let mut iv: Option<[u8; 16]> = None;
    let mut body = String::new();
    let mut started = false;
    for line in pem.lines() {
        if line.starts_with("-----BEGIN ") {
            started = true;
            continue;
        }
        if line.starts_with("-----END ") {
            break;
        }
        if !started {
            continue;
        }
        if let Some(rest) = line.strip_prefix("DEK-Info:") {
            let rest = rest.trim();
            let (name, iv_hex) = rest
                .split_once(',')
                .ok_or_else(|| WebOsError::Passphrase("malformed DEK-Info".to_string()))?;
            cipher = Some(name.trim().to_ascii_uppercase());
            let bytes = hex_decode(iv_hex.trim())
                .map_err(|error| WebOsError::Passphrase(format!("bad DEK IV: {error}")))?;
            iv = Some(
                bytes
                    .try_into()
                    .map_err(|_| WebOsError::Passphrase("DEK IV is not 16 bytes".to_string()))?,
            );
            continue;
        }
        if line.starts_with("Proc-Type:") || line.is_empty() {
            continue;
        }
        body.push_str(line);
    }
    let cipher = cipher.ok_or_else(|| {
        WebOsError::Passphrase("encrypted PEM has no DEK-Info header".to_string())
    })?;
    let iv = iv.ok_or_else(|| WebOsError::Passphrase("encrypted PEM has no IV".to_string()))?;
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .map_err(|error| WebOsError::Passphrase(format!("base64 body: {error}")))?;
    let mut derived = Vec::with_capacity(48);
    let mut previous: Vec<u8> = Vec::new();
    while derived.len() < 48 {
        let mut hasher = md5::Md5::new();
        hasher.update(&previous);
        hasher.update(passphrase.as_bytes());
        hasher.update(&iv[..8]);
        previous = hasher.finalize().to_vec();
        derived.extend_from_slice(&previous);
    }
    let derived_key = |length: usize| {
        derived
            .get(..length)
            .ok_or_else(|| WebOsError::Passphrase("short EVP key derivation".to_string()))
    };
    let plain = match cipher.as_str() {
        "AES-128-CBC" => decrypt_cbc::<aes::Aes128>(derived_key(16)?, &iv, &ciphertext)?,
        "AES-192-CBC" => decrypt_cbc::<aes::Aes192>(derived_key(24)?, &iv, &ciphertext)?,
        "AES-256-CBC" => decrypt_cbc::<aes::Aes256>(derived_key(32)?, &iv, &ciphertext)?,
        other => {
            return Err(WebOsError::Passphrase(format!(
                "unsupported DEK cipher {other}"
            )))
        }
    };
    Ok(plain)
}

fn decrypt_cbc<C>(key: &[u8], iv: &[u8; 16], ciphertext: &[u8]) -> Result<Vec<u8>, WebOsError>
where
    C: aes::cipher::BlockDecryptMut + aes::cipher::KeyInit + aes::cipher::BlockCipher,
{
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    let decryptor = cbc::Decryptor::<C>::new_from_slices(key, iv)
        .map_err(|error| WebOsError::Passphrase(format!("CBC init: {error}")))?;
    let mut buffer = ciphertext.to_vec();
    decryptor
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map(|plain| plain.to_vec())
        .map_err(|error| WebOsError::Passphrase(format!("decrypt: {error}")))
}

fn private_key_from_pkcs1_der(der: &[u8]) -> Result<PrivateKey, WebOsError> {
    use pkcs1_russh::DecodeRsaPrivateKey as _;
    use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
    let key = rsa_russh::RsaPrivateKey::from_pkcs1_der(der)
        .map_err(|error| WebOsError::Passphrase(format!("parse PKCS#1 RSA key: {error}")))?;
    let pair = RsaKeypair::try_from(key)
        .map_err(|error| WebOsError::Passphrase(format!("convert RSA key: {error}")))?;
    PrivateKey::new(KeypairData::Rsa(pair), "webos")
        .map_err(|error| WebOsError::Passphrase(format!("build SSH key: {error}")))
}

fn find_json_key<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    if let Some(found) = value.get(key) {
        return Some(found);
    }
    match value {
        serde_json::Value::Object(map) => map.values().find_map(|child| find_json_key(child, key)),
        serde_json::Value::Array(items) => items.iter().find_map(|child| find_json_key(child, key)),
        _ => None,
    }
}

fn device_info_string<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        find_json_key(value, key)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
    })
}

fn device_info_u32(value: &serde_json::Value, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|key| {
        let found = find_json_key(value, key)?;
        found
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| found.as_str()?.parse().ok())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InstallTerminal {
    Installed,
    Failed(String),
}

/// Interpret one line of a `com.webos.appInstallService/dev/install`
/// subscription. Progress states return `None`.
fn parse_install_line(line: &str) -> Option<InstallTerminal> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value
        .get("returnValue")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
    {
        return Some(InstallTerminal::Failed(luna_error_text(&value)));
    }
    let state = value
        .pointer("/details/state")
        .or_else(|| value.get("state"))
        .and_then(serde_json::Value::as_str)?
        .to_ascii_lowercase();
    if state.contains("fail") || state.contains("error") || state.contains("cancel") {
        return Some(InstallTerminal::Failed(
            value
                .get("errorText")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| format!("install state: {state}"), str::to_string),
        ));
    }
    if matches!(
        state.as_str(),
        "installed" | "complete" | "completed" | "succeeded" | "success"
    ) {
        return Some(InstallTerminal::Installed);
    }
    None
}

fn concise(stdout: &str, stderr: &[u8]) -> String {
    let mut text = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string();
    let error = String::from_utf8_lossy(stderr);
    if !error.trim().is_empty() {
        if !text.is_empty() {
            text.push_str(" / ");
        }
        text.push_str(error.trim());
    }
    if text.len() > 400 {
        text.truncate(400);
        text.push('…');
    }
    text
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn parse_luna_response(output: &ExecOutput, url: &str) -> Result<serde_json::Value, WebOsError> {
    let text = output.stdout_lossy();
    for line in text.lines().rev() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str(line) {
            return Ok(value);
        }
    }
    if let Ok(value) = serde_json::from_str(text.trim()) {
        return Ok(value);
    }
    let stderr = output.stderr_lossy();
    Err(WebOsError::Luna(format!(
        "{url} exited {:?}: {}{}",
        output.exit_status,
        text.trim(),
        if stderr.trim().is_empty() {
            String::new()
        } else {
            format!(" ({})", stderr.trim())
        }
    )))
}

fn luna_error_text(value: &serde_json::Value) -> String {
    value
        .get("errorText")
        .and_then(|value| value.as_str())
        .or_else(|| value.get("error").and_then(|value| value.as_str()))
        .unwrap_or("luna returned returnValue:false")
        .to_string()
}

/// luna methods used here always answer with an explicit boolean. A missing
/// `returnValue` is a malformed reply, not an implicit success.
fn require_luna_success(value: &serde_json::Value, operation: &str) -> Result<(), WebOsError> {
    match value
        .get("returnValue")
        .and_then(serde_json::Value::as_bool)
    {
        Some(true) => Ok(()),
        Some(false) => Err(WebOsError::Luna(luna_error_text(value))),
        None => Err(WebOsError::Luna(format!(
            "{operation} response has no returnValue"
        ))),
    }
}

#[must_use]
pub fn luna_url(service: &str, folder: Option<&str>, method: &str) -> String {
    match folder {
        Some(folder) if !folder.is_empty() => format!("luna://{service}/{folder}/{method}"),
        _ => format!("luna://{service}/{method}"),
    }
}

/// POSIX single-quote escaping. The returned value includes the quotes.
#[must_use]
pub fn shell_quote(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('\'');
    for (index, part) in value.split('\'').enumerate() {
        if index > 0 {
            output.push_str("'\\''");
        }
        output.push_str(part);
    }
    output.push('\'');
    output
}

fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

fn sanitize_file_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn hex_decode(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("odd length".to_string());
    }
    let mut output = Vec::with_capacity(value.len() / 2);
    for index in (0..value.len()).step_by(2) {
        let pair = value
            .get(index..index + 2)
            .ok_or_else(|| "invalid range".to_string())?;
        output.push(u8::from_str_radix(pair, 16).map_err(|error| format!("invalid hex: {error}"))?);
    }
    Ok(output)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

/// The inspector is normally on 9998; a configured endpoint may override it.
fn inspector_port(device: &DeviceConfig) -> u16 {
    let Some(endpoint) = device.connection.endpoint.as_deref() else {
        return DEFAULT_INSPECTOR_PORT;
    };
    endpoint
        .rsplit_once(':')
        .and_then(|(_, value)| {
            value
                .trim_end_matches('/')
                .split('/')
                .next()
                .unwrap_or_default()
                .parse::<u16>()
                .ok()
        })
        .unwrap_or(DEFAULT_INSPECTOR_PORT)
}

fn inspector_readiness_timeout() -> Duration {
    std::env::var("IMPRESARIO_WEBOS_INSPECTOR_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map_or(Duration::from_secs(20), Duration::from_millis)
}

/// Parse `<ip>[:port]`, used by doctor and tests.
#[must_use]
pub fn split_host(value: &str, default_port: u16) -> (String, u16) {
    match value.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => match port.parse::<u16>() {
            Ok(port) => (host.to_string(), port),
            Err(_) => (value.to_string(), default_port),
        },
        _ => (value.to_string(), default_port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::server::{Msg, Server as _};
    use russh::{Channel, ChannelId};

    #[test]
    fn luna_urls_match_ares_commands() {
        assert_eq!(
            luna_url("com.webos.applicationManager", Some("dev"), "running"),
            "luna://com.webos.applicationManager/dev/running"
        );
        assert_eq!(
            luna_url("com.webos.applicationManager", None, "launch"),
            "luna://com.webos.applicationManager/launch"
        );
    }

    #[test]
    fn shell_quoting_handles_apostrophes() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn parses_last_json_luna_line() -> Result<(), Box<dyn std::error::Error>> {
        let output = ExecOutput {
            stdout:
                b"noise\n{\"returnValue\":false,\"errorText\":\"first\"}\n{\"returnValue\":true}\n"
                    .to_vec(),
            stderr: Vec::new(),
            exit_status: Some(0),
        };
        let value = parse_luna_response(&output, "luna://x")?;
        assert_eq!(value["returnValue"], true);
        Ok(())
    }

    #[test]
    fn rejects_missing_json() {
        let output = ExecOutput {
            stdout: b"error".to_vec(),
            stderr: Vec::new(),
            exit_status: Some(1),
        };
        assert!(parse_luna_response(&output, "luna://x").is_err());
    }

    #[test]
    fn host_split_keeps_ipv4_and_port() {
        assert_eq!(split_host("10.0.0.2:9922", 22), ("10.0.0.2".into(), 9922));
        assert_eq!(split_host("10.0.0.2", 22), ("10.0.0.2".into(), 22));
    }

    #[test]
    fn chunked_decoding_is_bounded() -> Result<(), WebOsError> {
        assert_eq!(decode_chunked(b"3\r\nabc\r\n0\r\n\r\n")?, b"abc");
        Ok(())
    }

    #[derive(Default)]
    struct FakeSshState {
        commands: Mutex<Vec<String>>,
        uploads: Mutex<std::collections::HashMap<String, Vec<u8>>>,
        apps: Mutex<Vec<String>>,
        direct_tcpip_ports: Mutex<Vec<u32>>,
    }

    #[derive(Clone)]
    struct FakeSsh {
        state: Arc<FakeSshState>,
    }

    struct FakeSshHandler {
        state: Arc<FakeSshState>,
        channels: std::collections::HashMap<ChannelId, Channel<Msg>>,
    }

    impl russh::server::Server for FakeSsh {
        type Handler = FakeSshHandler;

        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
            FakeSshHandler {
                state: Arc::clone(&self.state),
                channels: std::collections::HashMap::new(),
            }
        }
    }

    impl russh::server::Handler for FakeSshHandler {
        type Error = russh::Error;

        async fn auth_publickey_offered(
            &mut self,
            user: &str,
            _public_key: &russh::keys::PublicKey,
        ) -> Result<russh::server::Auth, Self::Error> {
            Ok(if user == SSH_USER {
                russh::server::Auth::Accept
            } else {
                russh::server::Auth::reject()
            })
        }

        async fn auth_publickey(
            &mut self,
            user: &str,
            _public_key: &russh::keys::PublicKey,
        ) -> Result<russh::server::Auth, Self::Error> {
            Ok(if user == SSH_USER {
                russh::server::Auth::Accept
            } else {
                russh::server::Auth::reject()
            })
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<Msg>,
            reply: russh::server::ChannelOpenHandle,
            _session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            self.channels.insert(channel.id(), channel);
            reply.accept().await;
            Ok(())
        }

        async fn subsystem_request(
            &mut self,
            channel_id: ChannelId,
            name: &str,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            if name != "sftp" {
                session.channel_failure(channel_id)?;
                return Ok(());
            }
            let Some(channel) = self.channels.remove(&channel_id) else {
                session.channel_failure(channel_id)?;
                return Ok(());
            };
            session.channel_success(channel_id)?;
            russh_sftp::server::run(
                channel.into_stream(),
                FakeSftp {
                    state: Arc::clone(&self.state),
                },
            )
            .await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            let command = String::from_utf8_lossy(data).into_owned();
            self.state.commands.lock().await.push(command.clone());
            let output = if command.contains("com.webos.appInstallService/dev/install") {
                let app = "com.example.test".to_string();
                let mut apps = self.state.apps.lock().await;
                if !apps.contains(&app) {
                    apps.push(app);
                }
                "{\"returnValue\":true,\"details\":{\"state\":\"installed\"}}\n".to_string()
            } else if command.contains("applicationManager/dev/listApps") {
                let apps = self.state.apps.lock().await;
                let entries = apps
                    .iter()
                    .map(|id| format!("{{\"id\":{}}}", json_string(id)))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{\"returnValue\":true,\"apps\":[{entries}]}}\n")
            } else if command.contains("applicationManager/launch")
                || command.contains("applicationManager/dev/closeByAppId")
            {
                "{\"returnValue\":true}\n".to_string()
            } else if command.contains("com.webos.service.systemservice/deviceInfo/query") {
                "{\"returnValue\":true,\"modelName\":\"Fake webOS TV\",\"sdkVersion\":\"8.0\"}\n"
                    .to_string()
            } else {
                String::new()
            };
            session.channel_success(channel)?;
            if !output.is_empty() {
                session.data(channel, output)?;
            }
            session.exit_status_request(channel, 0)?;
            session.close(channel)?;
            Ok(())
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: Channel<Msg>,
            _host_to_connect: &str,
            port_to_connect: u32,
            _originator_address: &str,
            _originator_port: u32,
            reply: russh::server::ChannelOpenHandle,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            let id = channel.id();
            self.state
                .direct_tcpip_ports
                .lock()
                .await
                .push(port_to_connect);
            let handle = session.handle();
            reply.accept().await;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(25)).await;
                if handle.data(id, b"inspector-ok".to_vec()).await.is_ok() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = handle.close(id).await;
                }
            });
            Ok(())
        }

        async fn channel_eof(
            &mut self,
            _channel: ChannelId,
            _session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct FakeSftp {
        state: Arc<FakeSshState>,
    }

    impl russh_sftp::server::Handler for FakeSftp {
        type Error = russh_sftp::protocol::StatusCode;

        fn unimplemented(&self) -> Self::Error {
            russh_sftp::protocol::StatusCode::OpUnsupported
        }

        async fn init(
            &mut self,
            _version: u32,
            _extensions: std::collections::HashMap<String, String>,
        ) -> Result<russh_sftp::protocol::Version, Self::Error> {
            Ok(russh_sftp::protocol::Version::new())
        }

        async fn open(
            &mut self,
            id: u32,
            filename: String,
            _pflags: russh_sftp::protocol::OpenFlags,
            _attrs: russh_sftp::protocol::FileAttributes,
        ) -> Result<russh_sftp::protocol::Handle, Self::Error> {
            self.state
                .uploads
                .lock()
                .await
                .insert(filename.clone(), Vec::new());
            Ok(russh_sftp::protocol::Handle {
                id,
                handle: filename,
            })
        }

        async fn write(
            &mut self,
            id: u32,
            handle: String,
            offset: u64,
            data: Vec<u8>,
        ) -> Result<russh_sftp::protocol::Status, Self::Error> {
            let mut uploads = self.state.uploads.lock().await;
            let file = uploads.entry(handle).or_default();
            let offset =
                usize::try_from(offset).map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
            if file.len() < offset {
                file.resize(offset, 0);
            }
            if file.len() == offset {
                file.extend_from_slice(&data);
            } else {
                let end = offset.saturating_add(data.len());
                if file.len() < end {
                    file.resize(end, 0);
                }
                file[offset..end].copy_from_slice(&data);
            }
            Ok(russh_sftp::protocol::Status {
                id,
                status_code: russh_sftp::protocol::StatusCode::Ok,
                error_message: "Ok".to_string(),
                language_tag: "en-US".to_string(),
            })
        }

        async fn close(
            &mut self,
            id: u32,
            _handle: String,
        ) -> Result<russh_sftp::protocol::Status, Self::Error> {
            Ok(russh_sftp::protocol::Status {
                id,
                status_code: russh_sftp::protocol::StatusCode::Ok,
                error_message: "Ok".to_string(),
                language_tag: "en-US".to_string(),
            })
        }
    }

    struct FakeSshRunning {
        addr: std::net::SocketAddr,
        public_key: String,
        state: Arc<FakeSshState>,
        task: JoinHandle<()>,
    }

    impl FakeSshRunning {
        async fn start() -> Result<Self, Box<dyn std::error::Error>> {
            use russh::keys::Algorithm;
            let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
            let state = Arc::new(FakeSshState::default());
            let config = Arc::new(russh::server::Config {
                inactivity_timeout: Some(Duration::from_secs(60)),
                keys: vec![host_key.clone()],
                ..Default::default()
            });
            let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
            let addr = listener.local_addr()?;
            let public_key = host_key.public_key().to_openssh()?;
            let task_state = Arc::clone(&state);
            let task = tokio::spawn(async move {
                let mut server = FakeSsh { state: task_state };
                let running = server.run_on_socket(config, &listener);
                let _ = running.await;
            });
            Ok(Self {
                addr,
                public_key,
                state,
                task,
            })
        }
    }

    impl Drop for FakeSshRunning {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn fake_device(
        server: &FakeSshRunning,
        client_key: &Path,
    ) -> impresario_core::config::DeviceConfig {
        impresario_core::config::DeviceConfig {
            name: "fake_webos".to_string(),
            platform: Platform::WebOs,
            connection: impresario_core::config::DeviceConnection {
                host: Some("127.0.0.1".to_string()),
                port: Some(server.addr.port()),
                ..Default::default()
            },
            auth: impresario_core::config::DeviceAuth {
                key: Some(client_key.display().to_string()),
                identity: Some(server.public_key.clone()),
                ..Default::default()
            },
            labels: Vec::new(),
            options: impresario_core::config::DeviceOptions::default(),
        }
    }

    #[tokio::test]
    async fn fake_ssh_replays_luna_sftp_install_and_forwarding(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use russh::keys::ssh_key::LineEnding;
        use russh::keys::Algorithm;
        use tokio::io::AsyncReadExt as _;

        let server = FakeSshRunning::start().await?;
        let temporary = tempfile::tempdir()?;
        let client_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
        let key_path = temporary.path().join("id_ed25519");
        std::fs::write(&key_path, client_key.to_openssh(LineEnding::LF)?.as_bytes())?;
        let home = Home::at(temporary.path().join("home"));
        let device = fake_device(&server, &key_path);

        let session = WebOsSession::connect(&device, &home).await?;
        let info = session
            .luna(
                "com.webos.service.systemservice",
                Some("deviceInfo"),
                "query",
                "{}",
            )
            .await?;
        assert_eq!(info["modelName"], "Fake webOS TV");

        let ipk = temporary.path().join("app.ipk");
        std::fs::write(&ipk, b"fake-ipk-bytes")?;
        session.install("com.example.test", &ipk).await?;
        assert_eq!(
            server.state.apps.lock().await.clone(),
            vec!["com.example.test".to_string()]
        );
        let uploads = server.state.uploads.lock().await;
        assert_eq!(
            uploads
                .get("/media/developer/temp/impresario-com.example.test.ipk")
                .map(Vec::as_slice),
            Some(b"fake-ipk-bytes".as_slice())
        );
        drop(uploads);

        session.launch("com.example.test").await?;
        session.close("com.example.test").await?;
        session
            .wait_for_remote_port(DEFAULT_INSPECTOR_PORT, Duration::from_secs(2))
            .await?;
        server.state.direct_tcpip_ports.lock().await.clear();
        let mut direct_channel = session
            .handle
            .lock()
            .await
            .channel_open_direct_tcpip(
                "127.0.0.1",
                u32::from(DEFAULT_INSPECTOR_PORT),
                "127.0.0.1",
                0,
            )
            .await?;
        let message = timeout(Duration::from_secs(2), direct_channel.wait()).await?;
        assert!(matches!(
            message,
            Some(russh::ChannelMsg::Data { ref data }) if data.as_ref() == b"inspector-ok"
        ));
        let forward = session.forward(DEFAULT_INSPECTOR_PORT).await?;
        let mut stream = TcpStream::connect(("127.0.0.1", forward.local_port)).await?;
        let direct = timeout(Duration::from_secs(3), async {
            loop {
                if !server.state.direct_tcpip_ports.lock().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(direct.is_ok(), "direct-tcpip channel was not opened");
        let mut greeting = [0u8; 12];
        timeout(Duration::from_secs(2), stream.read_exact(&mut greeting)).await??;
        assert_eq!(&greeting, b"inspector-ok");
        forward.stop();

        let commands = server.state.commands.lock().await.clone();
        assert!(commands
            .iter()
            .any(|command| command.contains("luna://com.webos.appInstallService/dev/install")));
        assert!(commands
            .iter()
            .any(|command| command.contains("com.webos.appInstallService/dev/install")));
        assert!(commands
            .iter()
            .any(|command| command.contains("rm -f '/media/developer/temp/")));
        session.disconnect().await;
        Ok(())
    }

    #[tokio::test]
    async fn host_key_pin_mismatch_fails_before_auth() -> Result<(), Box<dyn std::error::Error>> {
        use russh::keys::ssh_key::LineEnding;
        use russh::keys::Algorithm;

        let server = FakeSshRunning::start().await?;
        let temporary = tempfile::tempdir()?;
        let client_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
        let key_path = temporary.path().join("id_ed25519");
        std::fs::write(&key_path, client_key.to_openssh(LineEnding::LF)?.as_bytes())?;
        let wrong = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
        let mut device = fake_device(&server, &key_path);
        device.auth.identity = Some(wrong.public_key().to_openssh()?);
        let result = WebOsSession::connect(&device, &Home::at(temporary.path())).await;
        assert!(result.is_err(), "a mismatched host key must be rejected");
        Ok(())
    }
}
