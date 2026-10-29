//! Native Android Debug Bridge transport (TASK-2 section 6.1).
//!
//! This module speaks the ADB wire protocol directly through `libadb`; no
//! `adb` binary is spawned at runtime (D-19). It owns the host key in the
//! Impresario key store, TCP and USB connections, wireless pairing, shell v2,
//! sync pushes and log streaming. Higher layers only see [`AdbClient`] and
//! the plain data types below.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use impresario_core::config::{DeviceConfig, TransportKind};
use impresario_core::home::Home;
use libadb::keys::rsa::rand_core::OsRng;
use libadb::keys::{store, AdbKey};
use libadb::shell::v2::{self as shell_v2, Frame};
use libadb::sync;
use libadb::tls::{TlsClientConfig, TlsIdentity};
use libadb::transport::any;
use libadb::transport::common::Transport;
use libadb::transport::nusb::Nusb;
use libadb::transport::runtime::{Runtime, Tokio};
use libadb::transport::tls::MaybeTls;
use libadb::{Connection, Feature};

/// Default port for classic (non-TLS) ADB over TCP.
pub const DEFAULT_TCP_PORT: u16 = 5555;
/// Features this client requires. `shell_v2` provides exit codes and
/// separate stdout/stderr; `sync` provides streamed APK pushes.
const REQUIRED_FEATURES: &[Feature] = &[
    Feature::ShellV2,
    Feature::StatV2,
    Feature::LsV2,
    Feature::SendrecvV2,
];

/// A native ADB connection with TLS-capable TCP and USB halves.
pub type AdbConnection = Connection<
    Transport<MaybeTls<<Tokio as Runtime>::Tcp>, <Nusb as libadb::UsbBackend<Tokio>>::Transport>,
>;

/// How to reach the device's `adbd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdbTarget {
    /// Direct TCP, including Android 11+ wireless debugging (TLS).
    Tcp { host: String, port: u16 },
    /// USB selector: a device serial, `VID:PID`, or empty for the first
    /// device that exposes the ADB interface.
    Usb { selector: String },
}

impl AdbTarget {
    /// Resolve the target from a device definition.
    pub fn for_device(device: &DeviceConfig) -> Result<Self, AdbError> {
        match device.effective_transport() {
            TransportKind::Usb => {
                let selector = device
                    .connection
                    .serial
                    .clone()
                    .or_else(|| device.connection.host.clone())
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        AdbError::Unsupported(format!(
                            "device '{}' uses the USB transport but has no serial; \
                             run `impresario device discover` and set connection.serial",
                            device.name
                        ))
                    })?;
                Ok(Self::Usb { selector })
            }
            TransportKind::Tcp | TransportKind::Network => {
                if let Some(serial) = device.connection.serial.as_deref() {
                    if let Some((host, port)) = split_host_port(serial) {
                        return Ok(Self::Tcp { host, port });
                    }
                    return Err(AdbError::Unsupported(format!(
                        "device '{}' serial '{serial}' is not HOST:PORT; use transport usb \
                         for a USB serial",
                        device.name
                    )));
                }
                let host = device.host().to_string();
                if host.trim().is_empty() {
                    return Err(AdbError::Unsupported(format!(
                        "device '{}' has no IP",
                        device.name
                    )));
                }
                let port = device.protocol_port().unwrap_or(DEFAULT_TCP_PORT);
                Ok(Self::Tcp { host, port })
            }
            other => Err(AdbError::Unsupported(format!(
                "transport '{}' does not support an Android device",
                other.as_str()
            ))),
        }
    }

    /// Parse an explicit target from the wizard/CLI: `HOST:PORT`, `usb://`,
    /// `usb://serial/...`, or a bare USB serial.
    pub fn parse(value: &str) -> Result<Self, AdbError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(AdbError::Usage("empty ADB target".to_string()));
        }
        if let Some(rest) = value.strip_prefix("usb://") {
            let selector = rest.strip_prefix("serial/").unwrap_or(rest);
            return Ok(Self::Usb {
                selector: selector.to_string(),
            });
        }
        if let Some((host, port)) = split_host_port(value) {
            return Ok(Self::Tcp { host, port });
        }
        if value.contains(':') || value.parse::<std::net::IpAddr>().is_ok() {
            return Err(AdbError::Usage(format!(
                "'{value}' is not HOST:PORT; wireless ADB needs the port shown on the device"
            )));
        }
        Ok(Self::Usb {
            selector: value.to_string(),
        })
    }

    /// The `libadb` URI for this target.
    #[must_use]
    pub fn uri(&self) -> String {
        match self {
            Self::Tcp { host, port } => format!("tcp://{host}:{port}"),
            Self::Usb { selector } if selector.is_empty() => "usb://".to_string(),
            Self::Usb { selector } => format!("usb://serial/{selector}"),
        }
    }

    /// Stable display label used in diagnostics and log messages.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Tcp { host, port } => format!("{host}:{port}"),
            Self::Usb { selector } if selector.is_empty() => "usb".to_string(),
            Self::Usb { selector } => selector.clone(),
        }
    }

    #[must_use]
    pub fn is_usb(&self) -> bool {
        matches!(self, Self::Usb { .. })
    }

    #[must_use]
    pub fn host(&self) -> Option<&str> {
        match self {
            Self::Tcp { host, .. } => Some(host),
            Self::Usb { .. } => None,
        }
    }
}

fn split_host_port(value: &str) -> Option<(String, u16)> {
    let (host, port) = value.rsplit_once(':')?;
    if host.is_empty() || port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port))
}

/// A failure in the native ADB layer. Callers map this to
/// [`crate::transport::TransportError`] or a CLI diagnostic.
#[derive(Debug, thiserror::Error)]
pub enum AdbError {
    #[error("{0}")]
    Usage(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("Android key store: {0}")]
    Key(String),
    #[error("cannot reach Android device at {target}: {message}")]
    Connect { target: String, message: String },
    #[error("Android command `{command}` failed with exit code {exit}: {message}")]
    Command {
        command: String,
        exit: u8,
        message: String,
    },
    #[error("Android command `{command}` timed out after {seconds} s")]
    Timeout { command: String, seconds: u64 },
    #[error("ADB protocol error: {0}")]
    Protocol(String),
}

impl AdbError {
    fn connect(target: &AdbTarget, error: impl std::fmt::Display) -> Self {
        Self::Connect {
            target: target.label(),
            message: error.to_string(),
        }
    }

    fn protocol(error: impl std::fmt::Display) -> Self {
        Self::Protocol(error.to_string())
    }
}

/// One command's captured output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: u8,
}

impl AdbOutput {
    #[must_use]
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    #[must_use]
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    fn summary(&self, limit: usize) -> String {
        let mut text = self.stderr_text();
        if text.trim().is_empty() {
            text = self.stdout_text();
        }
        let text = text.trim();
        let clipped = text
            .char_indices()
            .nth(limit)
            .map_or(text, |(index, _)| text.get(..index).unwrap_or(text));
        if clipped.len() < text.len() {
            format!("{clipped}...")
        } else {
            clipped.to_string()
        }
    }
}

/// What `getprop`/feature probing found on a reachable device.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AndroidProbe {
    pub serial: String,
    pub model: String,
    pub manufacturer: String,
    pub release: String,
    pub sdk: String,
    pub characteristics: String,
    pub abi: String,
    pub leanback: bool,
    pub tv: bool,
}

impl AndroidProbe {
    /// `Android TV` or `phone/tablet` for display and role checks.
    #[must_use]
    pub fn form_factor(&self) -> &'static str {
        if self.tv {
            "tv"
        } else {
            "phone"
        }
    }
}

/// Native ADB operations against one target.
#[derive(Debug, Clone)]
pub struct AdbClient {
    target: AdbTarget,
    key_dir: PathBuf,
    connect_timeout: Duration,
}

impl AdbClient {
    /// Client using the Impresario key store (`$IMPRESARIO_HOME/keys`).
    #[must_use]
    pub fn new(target: AdbTarget, home: &Home) -> Self {
        Self {
            target,
            key_dir: home.keys_dir(),
            connect_timeout: Duration::from_secs(20),
        }
    }

    /// Client with an explicit key directory (tests, portable installs).
    #[must_use]
    pub fn with_key_dir(target: AdbTarget, key_dir: impl Into<PathBuf>) -> Self {
        Self {
            target,
            key_dir: key_dir.into(),
            connect_timeout: Duration::from_secs(20),
        }
    }

    #[must_use]
    pub fn target(&self) -> &AdbTarget {
        &self.target
    }

    /// Load or create the host key and open one connection. Each call
    /// creates a fresh connection; the key on disk is reused.
    pub async fn connect(&self) -> Result<AdbConnection, AdbError> {
        let key = load_or_generate_key(&self.key_dir)?;
        let identity = TlsIdentity::from_key(&key, &mut OsRng)
            .map_err(|error| AdbError::Key(format!("cannot build the TLS identity: {error}")))?;
        let tls = TlsClientConfig::adb(&identity)
            .map_err(|error| AdbError::Key(format!("cannot configure ADB TLS: {error}")))?;
        let uri = self.target.uri();
        let transport =
            tokio::time::timeout(self.connect_timeout, any::connect::<Tokio, Nusb>(&uri))
                .await
                .map_err(|_| AdbError::Connect {
                    target: self.target.label(),
                    message: format!(
                        "opening {} timed out after {} s",
                        self.target.label(),
                        self.connect_timeout.as_secs()
                    ),
                })?
                .map_err(|error| AdbError::connect(&self.target, error))?;
        let connection = tokio::time::timeout(
            self.connect_timeout,
            Connection::connect_tls(transport.tls_ready(), key, REQUIRED_FEATURES, &tls),
        )
        .await
        .map_err(|_| AdbError::Connect {
            target: self.target.label(),
            message: format!(
                "the ADB handshake timed out after {} s",
                self.connect_timeout.as_secs()
            ),
        })?
        .map_err(|error| {
            AdbError::connect(
                &self.target,
                format!(
                    "{error}. If this is a new host key, confirm the USB debugging prompt \
                     on the device (or pair wireless debugging with `impresario device pair`)"
                ),
            )
        })?;
        Ok(connection)
    }

    /// Execute a shell command with the default query timeout.
    pub async fn exec(&self, command: &str) -> Result<AdbOutput, AdbError> {
        self.exec_with_timeout(command, Duration::from_secs(30))
            .await
    }

    /// Execute a shell command and require exit status 0.
    pub async fn exec_checked(&self, command: &str) -> Result<AdbOutput, AdbError> {
        let output = self.exec(command).await?;
        if output.exit_code != 0 {
            return Err(AdbError::Command {
                command: command.to_string(),
                exit: output.exit_code,
                message: output.summary(600),
            });
        }
        Ok(output)
    }

    /// Execute a shell command, capturing output even on a non-zero exit.
    pub async fn exec_with_timeout(
        &self,
        command: &str,
        timeout: Duration,
    ) -> Result<AdbOutput, AdbError> {
        let mut connection = self.connect().await?;
        let mut rx = vec![0u8; 64 * 1024];
        let output =
            tokio::time::timeout(timeout, shell_v2::exec(&mut connection, command, &mut rx))
                .await
                .map_err(|_| AdbError::Timeout {
                    command: command.to_string(),
                    seconds: timeout.as_secs(),
                })?
                .map_err(AdbError::protocol)?;
        Ok(AdbOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
        })
    }

    /// Probe the model, Android version and TV role.
    pub async fn probe(&self) -> Result<AndroidProbe, AdbError> {
        let command = concat!(
            "echo \"model=$(getprop ro.product.model)\";",
            "echo \"manufacturer=$(getprop ro.product.manufacturer)\";",
            "echo \"release=$(getprop ro.build.version.release)\";",
            "echo \"sdk=$(getprop ro.build.version.sdk)\";",
            "echo \"characteristics=$(getprop ro.build.characteristics)\";",
            "echo \"abi=$(getprop ro.product.cpu.abi)\";",
            "echo \"leanback=$(pm list features | grep -c '^feature:android.software.leanback')\""
        );
        let output = self.exec_checked(command).await?;
        let fields = parse_key_values(&output.stdout_text());
        let characteristics = fields.get("characteristics").cloned().unwrap_or_default();
        let leanback = fields
            .get("leanback")
            .is_some_and(|value| value.trim() == "1");
        let tv = leanback
            || characteristics
                .split(',')
                .any(|value| matches!(value.trim(), "tv" | "leanback"));
        Ok(AndroidProbe {
            serial: self.target.label(),
            model: fields.get("model").cloned().unwrap_or_default(),
            manufacturer: fields.get("manufacturer").cloned().unwrap_or_default(),
            release: fields.get("release").cloned().unwrap_or_default(),
            sdk: fields.get("sdk").cloned().unwrap_or_default(),
            characteristics,
            abi: fields.get("abi").cloned().unwrap_or_default(),
            leanback,
            tv,
        })
    }

    /// `pm list packages` contains `package`.
    pub async fn package_installed(&self, package: &str) -> Result<bool, AdbError> {
        let output = self.exec_checked("pm list packages").await?;
        Ok(package_installed_in(&output.stdout_text(), package))
    }

    /// Push one APK and install it for user 0. The APK is streamed in
    /// chunks through the sync protocol; installation itself goes through
    /// `cmd package install` (falling back to `pm install` on old devices).
    pub async fn install_apk(&self, apk: &Path) -> Result<(), AdbError> {
        let file = tokio::fs::File::open(apk).await.map_err(|error| {
            AdbError::Usage(format!("cannot read APK {}: {error}", apk.display()))
        })?;
        let file_name = apk
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                AdbError::Usage(format!("APK path {} has no file name", apk.display()))
            })?;
        let remote = format!("/data/local/tmp/impresario-{file_name}");
        let mut connection = self.connect().await?;
        let mut rx = vec![0u8; 64 * 1024];
        let mut session = sync::open(&mut connection, &mut rx)
            .await
            .map_err(AdbError::protocol)?;
        session
            .send_start(&remote, 0o100644)
            .await
            .map_err(AdbError::protocol)?;
        let mut file = file;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = tokio::io::AsyncReadExt::read(&mut file, &mut buffer)
                .await
                .map_err(|error| {
                    AdbError::Usage(format!("cannot read APK {}: {error}", apk.display()))
                })?;
            if read == 0 {
                break;
            }
            let chunk = buffer.get(..read).ok_or_else(|| {
                AdbError::Protocol(format!("sync read returned an invalid length ({read})"))
            })?;
            session.send_data(chunk).await.map_err(AdbError::protocol)?;
        }
        session.send_done(0).await.map_err(AdbError::protocol)?;
        session.quit().await.map_err(AdbError::protocol)?;

        let install = format!("cmd package install --user 0 -r -t {remote}");
        let mut output = self
            .exec_with_timeout(&install, Duration::from_secs(300))
            .await?;
        if output.exit_code != 0 || !output.stdout_text().contains("Success") {
            let fallback = format!("pm install --user 0 -r -t {remote}");
            output = self
                .exec_with_timeout(&fallback, Duration::from_secs(300))
                .await?;
        }
        let _ = self.exec(&format!("rm -f {remote}")).await;
        if output.exit_code != 0 || !output.stdout_text().contains("Success") {
            return Err(AdbError::Command {
                command: install,
                exit: output.exit_code,
                message: output.summary(800),
            });
        }
        Ok(())
    }

    /// Best-effort `pm clear` for the app under test.
    pub async fn clear_package(&self, package: &str) -> Result<AdbOutput, AdbError> {
        self.exec(&format!("pm clear --user 0 {package}")).await
    }

    /// Best-effort force-stop.
    pub async fn force_stop(&self, package: &str) -> Result<(), AdbError> {
        let _ = self
            .exec(&format!("am force-stop --user 0 {package}"))
            .await;
        Ok(())
    }

    /// PID of `package`, if running.
    pub async fn pid_of(&self, package: &str) -> Result<Option<u32>, AdbError> {
        let output = self.exec(&format!("pidof {package}")).await?;
        if output.exit_code != 0 {
            return Ok(None);
        }
        Ok(output
            .stdout_text()
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u32>().ok()))
    }

    /// One `getprop` value (never interpolates unchecked text into the shell).
    pub async fn getprop(&self, key: &str) -> Result<String, AdbError> {
        if key.is_empty()
            || !key.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
            })
        {
            return Err(AdbError::Usage(format!(
                "invalid getprop key '{key}'; expected [A-Za-z0-9._-]"
            )));
        }
        let output = self.exec_checked(&format!("getprop {key}")).await?;
        Ok(output.stdout_text().trim().to_string())
    }

    /// PNG screenshot bytes from `screencap -p` (device-side, no adb binary).
    pub async fn screencap(&self) -> Result<Vec<u8>, AdbError> {
        let output = self
            .exec_with_timeout("screencap -p", Duration::from_secs(30))
            .await?;
        if output.exit_code != 0 {
            return Err(AdbError::Command {
                command: "screencap -p".to_string(),
                exit: output.exit_code,
                message: output.summary(600),
            });
        }
        if !output.stdout.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(AdbError::Protocol(
                "screencap -p did not return PNG data".to_string(),
            ));
        }
        Ok(output.stdout)
    }
}

/// The comment shown in the device's "Allow USB debugging" dialog.
fn key_comment() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".to_string());
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".to_string());
    format!("impresario@{host}-{user}")
}

/// Load `adbkey`, generating it on first use.
///
/// `libadb`'s store has a Windows-specific bug: its mkdir loop turns the `D:`
/// path prefix into a `create_dir("D:")` call and fails with Access denied
/// before it ever reaches `keys/`. Do the first-run generation here with the
/// same file names and formats, and leave existing keys (including ones made
/// by `adb`) to `libadb::keys::store::load`.
fn load_or_generate_key(key_dir: &Path) -> Result<AdbKey, AdbError> {
    std::fs::create_dir_all(key_dir).map_err(|error| {
        AdbError::Key(format!(
            "cannot create key directory {}: {error}",
            key_dir.display()
        ))
    })?;
    let private = key_dir.join("adbkey");
    if private.exists() {
        let key = store::load(key_dir, &mut OsRng, &key_comment())
            .map_err(|error| AdbError::Key(error.to_string()))?;
        write_public_key_if_missing(key_dir, &key)?;
        return Ok(key);
    }

    let key = AdbKey::generate(&mut OsRng, &key_comment())
        .map_err(|error| AdbError::Key(format!("cannot generate the ADB host key: {error}")))?;
    let pem = key
        .to_pkcs8_pem()
        .map_err(|error| AdbError::Key(format!("cannot encode the ADB host key: {error}")))?;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let temporary = key_dir.join(format!(
        ".adbkey.{}.{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let write_temporary = (|| -> Result<(), String> {
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
        file.write_all(pem.as_bytes())
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("cannot flush {}: {error}", temporary.display()))?;
        Ok(())
    })();
    if let Err(error) = write_temporary {
        let _ = std::fs::remove_file(&temporary);
        return Err(AdbError::Key(error));
    }

    // A hard link publishes the key only if nobody else got there first. A
    // plain rename would overwrite another process's key, which is exactly the
    // key the device may already have been asked to trust.
    let published = match std::fs::hard_link(&temporary, &private) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(AdbError::Key(format!(
                "cannot install {}: {error}",
                private.display()
            )));
        }
    };
    let _ = std::fs::remove_file(&temporary);
    if !published {
        let key = store::load(key_dir, &mut OsRng, &key_comment())
            .map_err(|error| AdbError::Key(error.to_string()))?;
        write_public_key_if_missing(key_dir, &key)?;
        return Ok(key);
    }
    write_public_key_if_missing(key_dir, &key)?;
    Ok(key)
}

/// Keep the convenience `adbkey.pub` beside the private key. It is derived,
/// never read back, and may already exist from another tool.
fn write_public_key_if_missing(key_dir: &Path, key: &AdbKey) -> Result<(), AdbError> {
    let path = key_dir.join("adbkey.pub");
    if path.exists() {
        return Ok(());
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    match options.open(&path) {
        Ok(mut file) => file
            .write_all(key.public_key_line())
            .map_err(|error| AdbError::Key(format!("cannot write {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(AdbError::Key(format!(
            "cannot create {}: {error}",
            path.display()
        ))),
    }
}

fn parse_key_values(text: &str) -> std::collections::BTreeMap<String, String> {
    let mut values = std::collections::BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    values
}

fn package_installed_in(listing: &str, package: &str) -> bool {
    listing
        .lines()
        .any(|line| line.trim() == format!("package:{package}"))
}

/// Pair this host's key with Android 11+ wireless debugging.
pub async fn pair_wireless(target: &str, code: &str, key_dir: &Path) -> Result<String, AdbError> {
    use libadb::pairing::pair;
    use libadb::uri::{self, Uri};

    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AdbError::Usage(
            "the wireless pairing code is exactly six digits".to_string(),
        ));
    }
    let uri = if target.contains("://") {
        target.to_string()
    } else {
        format!("tcp://{target}")
    };
    let Ok(Uri::Tcp { host, port }) = uri::parse(&uri) else {
        return Err(AdbError::Usage(format!(
            "'{target}' is not HOST:PORT (the pairing port shown on the device)"
        )));
    };
    let key = load_or_generate_key(key_dir)?;
    let identity = TlsIdentity::from_key(&key, &mut OsRng)
        .map_err(|error| AdbError::Key(format!("cannot build the TLS identity: {error}")))?;
    let tls = TlsClientConfig::adb(&identity)
        .map_err(|error| AdbError::Key(format!("cannot configure ADB TLS: {error}")))?;
    let socket = <Tokio as Runtime>::connect_tcp(host, port)
        .await
        .map_err(|error| AdbError::Connect {
            target: target.to_string(),
            message: error.to_string(),
        })?;
    let mut transport = MaybeTls::plain(socket);
    let paired = pair(&mut transport, &tls, &key, code, &mut OsRng)
        .await
        .map_err(|error| AdbError::Connect {
            target: target.to_string(),
            message: error.to_string(),
        })?;
    Ok(paired.guid)
}

/// One USB device exposing an ADB interface.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UsbAndroidDevice {
    pub serial: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: String,
    pub manufacturer: String,
    pub driver: Option<String>,
    pub adb_interface: bool,
}

/// Enumerate USB devices without opening or claiming them. On Windows the
/// driver field shows whether WinUSB is bound; a missing ADB interface means
/// the device is in charging-only mode or the OEM driver is unavailable.
pub fn discover_usb() -> Result<Vec<UsbAndroidDevice>, AdbError> {
    let devices = nusb::list_devices()
        .map_err(|error| AdbError::Protocol(format!("USB enumeration failed: {error}")))?;
    let mut found = Vec::new();
    for device in devices {
        let adb_interface = device.interfaces().any(|interface| {
            interface.class() == 0xff
                && interface.subclass() == 0x42
                && interface.protocol() == 0x01
        });
        let vendor_id = device.vendor_id();
        let product_id = device.product_id();
        let serial = device
            .serial_number()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("{vendor_id:04x}:{product_id:04x}"));
        #[cfg(target_os = "windows")]
        let driver = device.driver().map(ToOwned::to_owned);
        #[cfg(not(target_os = "windows"))]
        let driver = None;
        found.push(UsbAndroidDevice {
            serial,
            vendor_id,
            product_id,
            product: device.product_string().unwrap_or("").to_string(),
            manufacturer: device.manufacturer_string().unwrap_or("").to_string(),
            driver,
            adb_interface,
        });
    }
    found.sort_by(|left, right| left.serial.cmp(&right.serial));
    Ok(found)
}

/// A discovered wireless-debugging endpoint from mDNS.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MdnsAndroidDevice {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub kind: String,
}

/// Browse `_adb-tls-connect._tcp` and `_adb._tcp` for a short window.
pub fn discover_mdns(timeout: Duration) -> Result<Vec<MdnsAndroidDevice>, AdbError> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    use std::collections::BTreeMap;

    let daemon = ServiceDaemon::new()
        .map_err(|error| AdbError::Protocol(format!("cannot start mDNS discovery: {error}")))?;
    let services = [
        ("_adb-tls-connect._tcp.local.", "wireless"),
        ("_adb-tls-pairing._tcp.local.", "pairing"),
        ("_adb._tcp.local.", "classic"),
    ];
    let mut receivers = Vec::new();
    for (service, kind) in services {
        let receiver = daemon
            .browse(service)
            .map_err(|error| AdbError::Protocol(format!("cannot browse {service}: {error}")))?;
        receivers.push((kind, receiver));
    }
    let deadline = std::time::Instant::now() + timeout;
    let mut found: BTreeMap<String, MdnsAndroidDevice> = BTreeMap::new();
    while std::time::Instant::now() < deadline {
        for (kind, receiver) in &receivers {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            if let Ok(ServiceEvent::ServiceResolved(info)) = receiver.recv_timeout(remaining) {
                for address in info.get_addresses_v4() {
                    let host = address.to_string();
                    let port = info.get_port();
                    let name = info.get_fullname().to_string();
                    let key = format!("{host}:{port}");
                    found.entry(key).or_insert(MdnsAndroidDevice {
                        name,
                        host,
                        port,
                        kind: (*kind).to_string(),
                    });
                }
            }
        }
    }
    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}

/// Read frames from a long-running shell until it exits, pushing each line
/// into `logs`. The connection is dropped when the task is aborted.
pub async fn stream_shell(
    client: &AdbClient,
    command: &str,
    source: &str,
    logs: crate::log::LogBuffer,
) -> Result<(), AdbError> {
    let mut connection = client.connect().await?;
    let mut rx = vec![0u8; 64 * 1024];
    let mut shell = shell_v2::open(&mut connection, command, &mut rx)
        .await
        .map_err(AdbError::protocol)?;
    let mut partial = String::new();
    loop {
        let frame = shell.read_frame().await.map_err(AdbError::protocol)?;
        match frame {
            Frame::Stdout(bytes) | Frame::Stderr(bytes) => {
                partial.push_str(&String::from_utf8_lossy(&bytes));
                push_complete_lines(&mut partial, source, &logs);
            }
            Frame::Exit(code) => {
                if !partial.is_empty() {
                    logs.push(crate::log::LogRecord::new(
                        source,
                        crate::log::LogLevel::Debug,
                        partial,
                    ));
                }
                logs.push(crate::log::LogRecord::new(
                    source,
                    crate::log::LogLevel::Debug,
                    format!("stream ended (exit {code})"),
                ));
                return Ok(());
            }
            Frame::Other { .. } => {}
        }
    }
}

fn push_complete_lines(partial: &mut String, source: &str, logs: &crate::log::LogBuffer) {
    while let Some(index) = partial.find('\n') {
        let line = partial[..index].trim_end_matches('\r').to_string();
        partial.drain(..=index);
        if !line.trim().is_empty() {
            logs.push(crate::log::LogRecord::new(source, parse_level(&line), line));
        }
    }
}

fn parse_level(line: &str) -> crate::log::LogLevel {
    use crate::log::LogLevel;
    if line.contains(" E/") || line.contains(" F/") || line.starts_with('E') {
        LogLevel::Error
    } else if line.contains(" W/") || line.starts_with('W') {
        LogLevel::Warn
    } else if line.contains(" I/") || line.starts_with('I') {
        LogLevel::Info
    } else {
        LogLevel::Debug
    }
}

/// Forward a local loopback port to `tcp:<device_port>` using a direct
/// ADB connection. Returns the local port; the task owns the listener and
/// connection and stops when aborted.
pub async fn spawn_forward(
    client: &AdbClient,
    device_port: u16,
    local_port: u16,
) -> Result<tokio::task::JoinHandle<()>, AdbError> {
    use libadb::channel::SelectResult;

    let target = client.target().label();
    let client = client.clone();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", local_port))
        .await
        .map_err(AdbError::protocol)?;
    let handle = tokio::spawn(async move {
        let mut connection = match client.connect().await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!( %target, %error, "ADB forward connection failed");
                return;
            }
        };
        let destination = format!("tcp:{device_port}\0");
        loop {
            let Ok((local, _peer)) = listener.accept().await else {
                return;
            };
            let channel = match connection.open_channel(destination.as_bytes()).await {
                Ok(channel) => channel,
                Err(error) => {
                    tracing::warn!( %target, %error, "ADB forward destination refused");
                    continue;
                }
            };
            let (mut local_read, mut local_write) = local.into_split();
            let mut channel_buffer = vec![0u8; 16 * 1024];
            let mut socket_buffer = vec![0u8; 16 * 1024];
            loop {
                let done = match connection
                    .select_channel(
                        channel,
                        &mut channel_buffer,
                        tokio::io::AsyncReadExt::read(&mut local_read, &mut socket_buffer),
                    )
                    .await
                {
                    Ok(SelectResult::Data(read)) => {
                        let Some(chunk) = channel_buffer.get(..read) else {
                            break;
                        };
                        tokio::io::AsyncWriteExt::write_all(&mut local_write, chunk)
                            .await
                            .is_err()
                    }
                    Ok(SelectResult::Interrupted(Ok(0))) => true,
                    Ok(SelectResult::Interrupted(Ok(read))) => {
                        let Some(chunk) = socket_buffer.get(..read) else {
                            break;
                        };
                        connection.write_channel(channel, chunk).await.is_err()
                    }
                    Ok(SelectResult::Interrupted(Err(_))) => true,
                    Err(_) => true,
                };
                if done {
                    let _ = connection.close_channel(channel).await;
                    break;
                }
            }
        }
    });
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_parse_tcp_usb_and_wireless() {
        assert_eq!(
            AdbTarget::parse("10.0.0.5:37565").expect("tcp"),
            AdbTarget::Tcp {
                host: "10.0.0.5".to_string(),
                port: 37565
            }
        );
        assert_eq!(
            AdbTarget::parse("usb://serial/ABC123").expect("usb"),
            AdbTarget::Usb {
                selector: "ABC123".to_string()
            }
        );
        assert_eq!(
            AdbTarget::parse("ABC123").expect("usb serial"),
            AdbTarget::Usb {
                selector: "ABC123".to_string()
            }
        );
        assert!(AdbTarget::parse("10.0.0.5").is_err());
        assert!(AdbTarget::parse("10.0.0.5:notaport").is_err());
    }

    #[test]
    fn target_uri_and_label_round_trip() {
        let tcp = AdbTarget::parse("127.0.0.1:5555").expect("tcp");
        assert_eq!(tcp.uri(), "tcp://127.0.0.1:5555");
        assert_eq!(tcp.label(), "127.0.0.1:5555");
        let usb = AdbTarget::parse("usb://").expect("usb");
        assert_eq!(usb.uri(), "usb://");
        assert_eq!(usb.label(), "usb");
    }

    #[test]
    fn key_value_and_package_parsers() {
        let values = parse_key_values("model=RMX3997\ncharacteristics=nosdcard\n");
        assert_eq!(values.get("model").map(String::as_str), Some("RMX3997"));
        assert!(package_installed_in(
            "package:com.impresario.agent\n",
            "com.impresario.agent"
        ));
        assert!(!package_installed_in(
            "package:com.impresario.agent.test\n",
            "com.impresario.agent"
        ));
    }

    #[test]
    fn key_store_generates_and_reuses_an_absolute_path_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let first = load_or_generate_key(dir.path()).expect("first key");
        let second = load_or_generate_key(dir.path()).expect("reused key");
        assert_eq!(first.public_key_wire(), second.public_key_wire());
        assert!(dir.path().join("adbkey").is_file());
        assert!(dir.path().join("adbkey.pub").is_file());
    }

    #[test]
    fn adb_error_summary_respects_utf8_boundaries() {
        let output = AdbOutput {
            stdout: Vec::new(),
            stderr: "ééééé".into(),
            exit_code: 1,
        };
        assert!(output.summary(3).ends_with("..."));
    }
}
