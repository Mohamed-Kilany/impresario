//! Native Samsung Tizen TV transport (TASK-2 6.4): the SDB wire protocol over
//! TCP 26101, sync push, the TV `0 ...` shell verbs, debug launch and CDP port
//! forwarding. No `sdb`, `tizen` or `ssh` process is started (D-19).
//!
//! Protocol facts are from the MIT-licensed `tizen-sdb` implementation
//! (`SdbFrame.cs`, `StreamSdbFrameTransport.cs`, `SdbTcpDevice.cs`,
//! `TizenInstaller.cs`) plus Tizen's public SDB documentation. The exact TV
//! install result remains hardware-unverified (HD-4).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use impresario_core::apps::AppConfig;
use impresario_core::config::DeviceConfig;
use impresario_core::home::Home;
use impresario_core::platforms::Platform;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPrivateKey};
use sha1::Sha1;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::agent_client::DeviceInfo;
use crate::log::{unix_ms_now, LogBuffer, LogLevel, LogRecord};
use crate::transport::{
    ConnectorTransport, DeviceTransport, Endpoint, LogSource, LogSourceKind, NativeProbe,
    ResetOutcome, TransportError,
};

const CMD_CNXN: u32 = 0x4e58_4e43;
const CMD_AUTH: u32 = 0x4854_5541;
const CMD_OPEN: u32 = 0x4e45_504f;
const CMD_OKAY: u32 = 0x5941_4b4f;
const CMD_WRTE: u32 = 0x4554_5257;
const CMD_CLSE: u32 = 0x4553_4c43;
const AUTH_TOKEN: u32 = 1;
const AUTH_SIGNATURE: u32 = 2;
const AUTH_PUBLICKEY: u32 = 3;
const CLIENT_VERSION: u32 = 0x0100_0000;
const DEFAULT_SDB_PORT: u16 = 26101;
const MAX_DATA: u32 = 4096;
/// Staging fallback used when the TV does not report `sdk_toolpath`; the
/// reference installer uses the same directory.
const REMOTE_STAGING_FALLBACK: &str = "/home/owner/share/tmp/sdk_tools/tmp";
const OPERATION_TIMEOUT: Duration = Duration::from_secs(20);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);

/// SDB/Tizen failures.
#[derive(Debug, thiserror::Error)]
pub enum TizenError {
    #[error("Tizen host is not configured (connection.host)")]
    NoHost,
    #[error("Tizen app is required for a native run")]
    NoApp,
    #[error("Tizen app has no id for this platform")]
    NoAppId,
    #[error("Tizen app has no package; configure platforms.tizen.package for install")]
    NoPackage,
    #[error("SDB protocol: {0}")]
    Protocol(String),
    #[error("SDB I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("SDB authentication failed: {0}")]
    Auth(String),
    #[error("Tizen TV rejected '{command}': {reply}")]
    Command { command: String, reply: String },
    #[error("Tizen install did not make '{0}' appear in 0 vd_applist")]
    InstallUnverified(String),
    #[error("Tizen debug launch returned no inspector port: {0}")]
    NoInspectorPort(String),
    #[error("Tizen DUID lookup failed after 3 attempts: {0}")]
    Duid(String),
    #[error("Tizen signing/profile: {0}")]
    Signing(String),
    #[error("Tizen {0} did not finish within {1:?}")]
    Timeout(&'static str, Duration),
}

impl From<TizenError> for TransportError {
    fn from(value: TizenError) -> Self {
        match value {
            TizenError::NoHost
            | TizenError::NoApp
            | TizenError::NoAppId
            | TizenError::NoPackage
            | TizenError::Auth(_)
            | TizenError::Duid(_)
            | TizenError::Signing(_) => TransportError::DeviceUnavailable(value.to_string()),
            other => TransportError::Transport(other.to_string()),
        }
    }
}

fn le_u32(bytes: &[u8]) -> u32 {
    let mut word = [0u8; 4];
    if let Some(front) = bytes.get(..4) {
        word.copy_from_slice(front);
    }
    u32::from_le_bytes(word)
}

/// One 24-byte SDB frame plus payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdbFrame {
    pub command: u32,
    pub arg0: u32,
    pub arg1: u32,
    pub payload: Vec<u8>,
}

impl SdbFrame {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let length = self.payload.len() as u32;
        let checksum = self
            .payload
            .iter()
            .fold(0u32, |sum, byte| sum.wrapping_add(u32::from(*byte)));
        let mut output = Vec::with_capacity(24 + self.payload.len());
        for value in [
            self.command,
            self.arg0,
            self.arg1,
            length,
            checksum,
            self.command ^ 0xffff_ffff,
        ] {
            output.extend_from_slice(&value.to_le_bytes());
        }
        output.extend_from_slice(&self.payload);
        output
    }

    /// Decode a header and validate the magic, declared length and checksum.
    pub fn decode(header: &[u8; 24], payload: Vec<u8>) -> Result<Self, TizenError> {
        let word = |offset: usize| -> u32 { le_u32(header.get(offset..).unwrap_or_default()) };
        let command = word(0);
        let length = word(12) as usize;
        let checksum = word(16);
        let magic = word(20);
        if magic != command ^ 0xffff_ffff {
            return Err(TizenError::Protocol(format!(
                "bad magic 0x{magic:08x} for command 0x{command:08x}"
            )));
        }
        if length != payload.len() {
            return Err(TizenError::Protocol(format!(
                "declared payload {length}, got {}",
                payload.len()
            )));
        }
        let actual = payload
            .iter()
            .fold(0u32, |sum, byte| sum.wrapping_add(u32::from(*byte)));
        if actual != checksum {
            return Err(TizenError::Protocol(format!(
                "bad checksum 0x{checksum:08x}, computed 0x{actual:08x}"
            )));
        }
        Ok(Self {
            command,
            arg0: word(4),
            arg1: word(8),
            payload,
        })
    }
}

/// Values reported by the TV's `capability:` service.
///
/// The wire payload is a two-byte length prefix followed by `key:value` lines.
/// Unknown keys are retained so newer TVs do not break the parser.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TizenCapabilities {
    values: BTreeMap<String, String>,
}

impl TizenCapabilities {
    /// Parse the payload returned by `capability:`. The reference installer
    /// strips the first two bytes; tolerate an unprefixed fixture as well.
    #[must_use]
    pub fn parse(payload: &[u8]) -> Self {
        let body = payload.get(2..).unwrap_or_default();
        let body = if body.contains(&b':') {
            body
        } else if payload.contains(&b':') {
            payload
        } else {
            body
        };
        let mut values = BTreeMap::new();
        for line in String::from_utf8_lossy(body).lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            if !key.is_empty() {
                values.insert(key.to_string(), value.trim().to_string());
            }
        }
        Self { values }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    #[must_use]
    pub fn platform_version(&self) -> &str {
        self.get("platform_version").unwrap_or("9.0")
    }

    /// Remote directory used for pushed packages. The documented capability is
    /// `sdk_toolpath`; older TVs fall back to the reference installer's path.
    #[must_use]
    pub fn sdk_toolpath(&self) -> &str {
        self.get("sdk_toolpath")
            .filter(|value| !value.trim().is_empty())
            .map_or(REMOTE_STAGING_FALLBACK, |value| value)
    }

    /// Tizen 7.0 and newer require re-signing with a certificate that covers
    /// the device DUID. Older TVs install the original package.
    #[must_use]
    pub fn requires_signing(&self) -> bool {
        platform_requires_signing(self.platform_version())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Whether a Tizen `platform_version` is at least 7.0. Missing or malformed
/// versions are treated as modern so we never silently install unsigned.
#[must_use]
pub fn platform_requires_signing(version: &str) -> bool {
    let major = version
        .trim()
        .split(['.', '-', ' '])
        .next()
        .and_then(|value| value.parse::<u32>().ok());
    major.is_none_or(|major| major >= 7)
}

/// One authenticated SDB connection, used for one channel at a time. Forwarded
/// TCP streams and commands each use a fresh connection, matching the TV's
/// daemon behaviour without a host-side `sdb` server.
pub struct SdbConnection {
    stream: TcpStream,
    max_data: u32,
    banner: String,
    key_path: PathBuf,
}

impl SdbConnection {
    pub async fn connect(host: &str, port: u16, home: &Home) -> Result<Self, TizenError> {
        let stream = timeout(Duration::from_secs(10), TcpStream::connect((host, port)))
            .await
            .map_err(|_| TizenError::Timeout("SDB connect", Duration::from_secs(10)))??;
        stream.set_nodelay(true)?;
        let (key, key_path) = load_or_create_key(home)?;
        let mut connection = Self {
            stream,
            max_data: MAX_DATA,
            banner: String::new(),
            key_path,
        };
        connection
            .handshake(&key, host)
            .await
            .map_err(|error| match error {
                TizenError::Protocol(_) | TizenError::Io(_) => TizenError::Auth(error.to_string()),
                other => other,
            })?;
        Ok(connection)
    }

    async fn handshake(&mut self, key: &RsaPrivateKey, host: &str) -> Result<(), TizenError> {
        let banner = format!("host::impresario-sdb:{}", std::env::consts::OS);
        self.write_frame(&SdbFrame {
            command: CMD_CNXN,
            arg0: CLIENT_VERSION,
            arg1: MAX_DATA,
            payload: banner.into_bytes(),
        })
        .await?;
        let mut sent_public_key = false;
        for _ in 0..8 {
            let frame = timeout(OPERATION_TIMEOUT, self.read_frame())
                .await
                .map_err(|_| TizenError::Timeout("SDB handshake", OPERATION_TIMEOUT))??;
            match frame.command {
                CMD_CNXN => {
                    if frame.arg1 > 0 {
                        self.max_data = self.max_data.min(frame.arg1);
                    }
                    self.banner = String::from_utf8_lossy(&frame.payload).into_owned();
                    tracing::debug!(banner = %self.banner, "SDB handshake complete");
                    return Ok(());
                }
                CMD_AUTH if frame.arg0 == AUTH_TOKEN => {
                    tracing::debug!("SDB AUTH token requested");
                    let signature = sign_token(key, &frame.payload)?;
                    self.write_frame(&SdbFrame {
                        command: CMD_AUTH,
                        arg0: AUTH_SIGNATURE,
                        arg1: 0,
                        payload: signature,
                    })
                    .await?;
                }
                CMD_AUTH if frame.arg0 == AUTH_PUBLICKEY => {
                    tracing::debug!("SDB AUTH public-key requested");
                    if sent_public_key {
                        return Err(TizenError::Auth(
                            "the TV keeps requesting the public key; accept the prompt on the TV"
                                .into(),
                        ));
                    }
                    sent_public_key = true;
                    let blob = android_public_key(key)?;
                    let user = std::env::var("USERNAME")
                        .or_else(|_| std::env::var("USER"))
                        .unwrap_or_else(|_| "impresario".to_string());
                    let mut payload = base64_encode(&blob);
                    payload.push_str(&format!(" {user}@{host}\0"));
                    self.write_frame(&SdbFrame {
                        command: CMD_AUTH,
                        arg0: AUTH_PUBLICKEY,
                        arg1: 0,
                        payload: payload.into_bytes(),
                    })
                    .await?;
                }
                CMD_AUTH => {
                    return Err(TizenError::Auth(format!(
                        "unknown AUTH type {}",
                        frame.arg0
                    )))
                }
                other => {
                    return Err(TizenError::Protocol(format!(
                        "expected CNXN/AUTH during handshake, got {other:#x}"
                    )))
                }
            }
        }
        Err(TizenError::Auth(
            "TV did not complete the SDB handshake".to_string(),
        ))
    }

    #[must_use]
    pub fn banner(&self) -> &str {
        &self.banner
    }

    #[must_use]
    pub fn key_path(&self) -> &Path {
        &self.key_path
    }

    async fn write_frame(&mut self, frame: &SdbFrame) -> Result<(), TizenError> {
        self.stream.write_all(&frame.encode()).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn read_frame(&mut self) -> Result<SdbFrame, TizenError> {
        let mut header = [0u8; 24];
        self.stream.read_exact(&mut header).await?;
        let length = le_u32(header.get(12..).unwrap_or_default()) as usize;
        if length > 16 * 1024 * 1024 {
            return Err(TizenError::Protocol(format!(
                "payload length {length} exceeds 16 MiB"
            )));
        }
        let mut payload = vec![0u8; length];
        self.stream.read_exact(&mut payload).await?;
        SdbFrame::decode(&header, payload)
    }

    /// Open one SDB service. `Ok(None)` means the channel closed before OKAY.
    pub async fn open(mut self, service: &str) -> Result<SdbChannel, TizenError> {
        let local = next_local_id();
        let mut payload = service.as_bytes().to_vec();
        if !payload.ends_with(&[0]) {
            payload.push(0);
        }
        self.write_frame(&SdbFrame {
            command: CMD_OPEN,
            arg0: local,
            arg1: 0,
            payload,
        })
        .await?;
        loop {
            let frame = timeout(OPERATION_TIMEOUT, self.read_frame())
                .await
                .map_err(|_| TizenError::Timeout("SDB open", OPERATION_TIMEOUT))??;
            match frame.command {
                CMD_OKAY if frame.arg1 == local => {
                    return Ok(SdbChannel {
                        connection: self,
                        local,
                        remote: frame.arg0,
                        buffered: Vec::new(),
                        closed: false,
                    });
                }
                CMD_CLSE if frame.arg1 == local => {
                    return Err(TizenError::Protocol(format!(
                        "device closed '{service}' before OKAY"
                    )));
                }
                _ => {}
            }
        }
    }

    /// Run a TV shell verb and return all output.
    pub async fn shell(self, command: &str) -> Result<String, TizenError> {
        let channel = self.open(&format!("shell:{command}")).await?;
        channel.read_to_end().await
    }

    /// Read the TV capability dictionary (`capability:` service).
    pub async fn capability(self) -> Result<TizenCapabilities, TizenError> {
        let channel = self.open("capability:").await?;
        let bytes = channel.read_bytes_to_end().await?;
        Ok(TizenCapabilities::parse(&bytes))
    }

    /// Push one local file through the sync service.
    ///
    /// Every WRTE is acknowledged by the TV, and each frame must fit the
    /// negotiated `max_data`. Sending unacknowledged or oversized sync frames
    /// makes sdbd drop the connection (observed on a Tizen 9.0 TV), so the
    /// SEND/DATA/DONE stream goes through `send_wrte`.
    pub async fn push(self, local: &Path, remote: &str) -> Result<(), TizenError> {
        let bytes = std::fs::read(local)?;
        let mut channel = self.open("sync:").await?;
        let mut header = Vec::with_capacity(8 + remote.len());
        header.extend_from_slice(b"SEND");
        header.extend_from_slice(&(remote.len() as u32).to_le_bytes());
        header.extend_from_slice(remote.as_bytes());
        channel.send_wrte(&header).await?;
        for chunk in bytes.chunks(64 * 1024) {
            let mut payload = Vec::with_capacity(8 + chunk.len());
            payload.extend_from_slice(b"DATA");
            payload.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
            payload.extend_from_slice(chunk);
            channel.send_wrte(&payload).await?;
        }
        let modified = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as u32)
            .unwrap_or(0);
        let mut done = Vec::with_capacity(8);
        done.extend_from_slice(b"DONE");
        done.extend_from_slice(&modified.to_le_bytes());
        channel.send_wrte(&done).await?;
        let response = channel.read_sync_response().await?;
        match response.0.as_str() {
            "OKAY" => Ok(()),
            "FAIL" => Err(TizenError::Protocol(format!(
                "sync push failed: {}",
                String::from_utf8_lossy(&response.1)
            ))),
            other => Err(TizenError::Protocol(format!(
                "unexpected sync response {other}"
            ))),
        }
    }
}

pub struct SdbChannel {
    connection: SdbConnection,
    local: u32,
    remote: u32,
    buffered: Vec<u8>,
    closed: bool,
}

impl SdbChannel {
    async fn send_okay(&mut self, remote: u32) -> Result<(), TizenError> {
        let frame = SdbFrame {
            command: CMD_OKAY,
            arg0: self.local,
            arg1: remote,
            payload: Vec::new(),
        };
        self.connection.write_frame(&frame).await
    }

    async fn send_wrte(&mut self, payload: &[u8]) -> Result<(), TizenError> {
        // Honor the negotiated maxData; each WRTE is acknowledged by OKAY.
        for chunk in payload.chunks(self.connection.max_data.max(1) as usize) {
            let frame = SdbFrame {
                command: CMD_WRTE,
                arg0: self.local,
                arg1: self.remote,
                payload: chunk.to_vec(),
            };
            self.connection.write_frame(&frame).await?;
            loop {
                let frame = timeout(OPERATION_TIMEOUT, self.connection.read_frame())
                    .await
                    .map_err(|_| TizenError::Timeout("SDB write", OPERATION_TIMEOUT))??;
                match frame.command {
                    CMD_OKAY if frame.arg1 == self.local => break,
                    CMD_WRTE if frame.arg1 == self.local => {
                        self.buffered.extend_from_slice(&frame.payload);
                        self.send_okay(frame.arg0).await?;
                    }
                    CMD_CLSE if frame.arg1 == self.local => {
                        self.closed = true;
                        return Err(TizenError::Protocol("channel closed while writing".into()));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    async fn read_frame(&mut self) -> Result<SdbFrame, TizenError> {
        timeout(OPERATION_TIMEOUT, self.connection.read_frame())
            .await
            .map_err(|_| TizenError::Timeout("SDB read", OPERATION_TIMEOUT))?
    }

    async fn read_sync_response(&mut self) -> Result<(String, Vec<u8>), TizenError> {
        let mut buffered = std::mem::take(&mut self.buffered);
        loop {
            if let Some(header) = buffered.get(..8) {
                let id = String::from_utf8_lossy(header.get(..4).unwrap_or_default()).into_owned();
                let length = u32::from_le_bytes(
                    header
                        .get(4..8)
                        .and_then(|value| value.try_into().ok())
                        .unwrap_or([0u8; 4]),
                ) as usize;
                let end = 8usize.saturating_add(length);
                if let Some(payload) = buffered.get(8..end) {
                    let payload = payload.to_vec();
                    self.buffered = buffered.split_off(end);
                    return Ok((id, payload));
                }
            }
            if self.closed {
                return Err(TizenError::Protocol(
                    "sync channel closed before a response".into(),
                ));
            }
            let frame = self.read_frame().await?;
            match frame.command {
                CMD_WRTE if frame.arg1 == self.local => {
                    buffered.extend_from_slice(&frame.payload);
                    self.send_okay(frame.arg0).await?;
                }
                CMD_OKAY if frame.arg1 == self.local => {}
                CMD_CLSE if frame.arg1 == self.local => {
                    self.closed = true;
                    self.send_okay(frame.arg0).await?;
                }
                _ => {}
            }
        }
    }

    async fn read_to_end(self) -> Result<String, TizenError> {
        let output = self.read_bytes_to_end().await?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    async fn read_bytes_to_end(mut self) -> Result<Vec<u8>, TizenError> {
        let mut output = std::mem::take(&mut self.buffered);
        loop {
            let frame = self.read_frame().await?;
            match frame.command {
                CMD_WRTE if frame.arg1 == self.local => {
                    output.extend_from_slice(&frame.payload);
                    self.send_okay(frame.arg0).await?;
                }
                CMD_CLSE if frame.arg1 == self.local => {
                    let acknowledgement = SdbFrame {
                        command: CMD_CLSE,
                        arg0: self.local,
                        arg1: frame.arg0,
                        payload: Vec::new(),
                    };
                    let _ = self.connection.write_frame(&acknowledgement).await;
                    break;
                }
                _ => {}
            }
        }
        Ok(output)
    }

    /// Pump bytes in both directions for one forwarded TCP stream.
    async fn proxy(mut self, mut socket: TcpStream) -> Result<(), TizenError> {
        let mut buffered = std::mem::take(&mut self.buffered);
        if !buffered.is_empty() {
            socket.write_all(&buffered).await?;
            buffered.clear();
        }
        let mut socket_buf = vec![0u8; 16 * 1024];
        loop {
            tokio::select! {
                read = socket.read(&mut socket_buf) => {
                    let read = read?;
                    if read == 0 {
                        let _ = self.connection.write_frame(&SdbFrame {
                            command: CMD_CLSE,
                            arg0: self.local,
                            arg1: self.remote,
                            payload: Vec::new(),
                        }).await;
                        return Ok(());
                    }
                    self.send_wrte(socket_buf.get(..read).unwrap_or_default()).await?;
                }
                frame = self.connection.read_frame() => {
                    let frame = frame?;
                    match frame.command {
                        CMD_WRTE if frame.arg1 == self.local => {
                            socket.write_all(&frame.payload).await?;
                            self.send_okay(frame.arg0).await?;
                        }
                        CMD_CLSE if frame.arg1 == self.local => {
                            let _ = self.connection.write_frame(&SdbFrame {
                                command: CMD_CLSE,
                                arg0: self.local,
                                arg1: frame.arg0,
                                payload: Vec::new(),
                            }).await;
                            return Ok(());
                        }
                        CMD_OKAY if frame.arg1 == self.local => {}
                        _ => {}
                    }
                }
            }
        }
    }
}

fn next_local_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed).max(1)
}

fn load_or_create_key(home: &Home) -> Result<(RsaPrivateKey, PathBuf), TizenError> {
    let path = home.keys_dir().join("tizen_adbkey");
    if path.is_file() {
        let bytes = std::fs::read(&path)?;
        let key = RsaPrivateKey::from_pkcs8_der(&bytes)
            .map_err(|error| TizenError::Auth(format!("parse {}: {error}", path.display())))?;
        return Ok((key, path));
    }
    let mut rng = rsa::rand_core::OsRng;
    let key = RsaPrivateKey::new(&mut rng, 2048)
        .map_err(|error| TizenError::Auth(format!("generate RSA key: {error}")))?;
    let der = key
        .to_pkcs8_der()
        .map_err(|error| TizenError::Auth(format!("encode RSA key: {error}")))?;
    std::fs::write(&path, der.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(metadata) = std::fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            let _ = std::fs::set_permissions(&path, permissions);
        }
    }
    Ok((key, path))
}

fn sign_token(key: &RsaPrivateKey, token: &[u8]) -> Result<Vec<u8>, TizenError> {
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};
    let signing_key = SigningKey::<Sha1>::new(key.clone());
    Ok(signing_key.sign(token).to_vec())
}

/// Android's custom RSAPublicKey blob (little-endian modulus, n0inv, rr and
/// exponent), as SDB expects in an AUTH publickey reply.
fn android_public_key(key: &RsaPrivateKey) -> Result<Vec<u8>, TizenError> {
    const MODULUS: usize = 256;
    let n = key.n();
    let modulus_be = n.to_bytes_be();
    if modulus_be.len() > MODULUS {
        return Err(TizenError::Auth("RSA key is larger than 2048 bits".into()));
    }
    let mut modulus_le = vec![0u8; MODULUS];
    for (index, byte) in modulus_be.iter().rev().enumerate() {
        if let Some(slot) = modulus_le.get_mut(index) {
            *slot = *byte;
        }
    }
    let mut least = [0u8; 4];
    if let Some(front) = modulus_le.get(..4) {
        least.copy_from_slice(front);
    }
    let n0 = u32::from_le_bytes(least);
    let n0inv = 0u32.wrapping_sub(inverse_mod_2_32(n0));
    let exponent = key.e().to_bytes_be();
    let exponent = exponent
        .iter()
        .fold(0u32, |value, byte| (value << 8) | u32::from(*byte));
    let rr = (BigUint::from(1u8) << (MODULUS * 16)) % n;
    let mut rr_le = vec![0u8; MODULUS];
    for (index, byte) in rr.to_bytes_le().iter().enumerate().take(MODULUS) {
        if let Some(slot) = rr_le.get_mut(index) {
            *slot = *byte;
        }
    }
    let mut output = Vec::with_capacity(4 + 4 + MODULUS + MODULUS + 4);
    output.extend_from_slice(&((MODULUS / 4) as u32).to_le_bytes());
    output.extend_from_slice(&n0inv.to_le_bytes());
    output.extend_from_slice(&modulus_le);
    output.extend_from_slice(&rr_le);
    output.extend_from_slice(&exponent.to_le_bytes());
    Ok(output)
}

fn inverse_mod_2_32(value: u32) -> u32 {
    let mut inverse = 1u32;
    for _ in 0..5 {
        inverse = inverse.wrapping_mul(2u32.wrapping_sub(value.wrapping_mul(inverse)));
    }
    inverse
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---------------------------------------------------------------------------
// transport
// ---------------------------------------------------------------------------

pub struct TizenTransport {
    connector: ConnectorTransport,
    host: Option<String>,
    port: u16,
    forward: Option<Forward>,
    target: Option<String>,
    app_id: Option<String>,
    home: Option<Home>,
    explicit_endpoint: Option<String>,
    logs: LogBuffer,
}

impl Default for TizenTransport {
    fn default() -> Self {
        Self::new()
    }
}

struct Forward {
    task: JoinHandle<()>,
}

impl Drop for Forward {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TizenTransport {
    #[must_use]
    pub fn new() -> Self {
        Self {
            connector: ConnectorTransport::new(),
            host: None,
            port: DEFAULT_SDB_PORT,
            forward: None,
            target: None,
            app_id: None,
            home: None,
            explicit_endpoint: None,
            logs: LogBuffer::new(1000),
        }
    }

    fn push_log(&self, level: LogLevel, message: impl Into<String>) {
        self.logs.push(LogRecord {
            unix_ms: unix_ms_now(),
            level,
            source: "tizen".to_string(),
            message: message.into(),
        });
    }

    async fn shell(&self, command: &str) -> Result<String, TizenError> {
        let host = self.host.as_deref().ok_or(TizenError::NoHost)?;
        let home = self
            .home
            .as_ref()
            .ok_or_else(|| TizenError::Protocol("home not initialised".into()))?;
        let connection = SdbConnection::connect(host, self.port, home).await?;
        connection.shell(command).await
    }

    /// Read the TV capability dictionary. This is also the cheapest
    /// connectivity/authentication probe (no app is installed or launched).
    pub async fn capabilities(&self) -> Result<TizenCapabilities, TizenError> {
        let host = self.host.as_deref().ok_or(TizenError::NoHost)?;
        let home = self
            .home
            .as_ref()
            .ok_or_else(|| TizenError::Protocol("home not initialised".into()))?;
        let connection = SdbConnection::connect(host, self.port, home).await?;
        connection.capability().await
    }

    /// Query the TV DUID with the documented retry/validation.
    pub async fn duid(&self) -> Result<String, TizenError> {
        let mut last = String::new();
        for _ in 0..3 {
            match self.shell("0 getduid").await {
                Ok(reply) => {
                    let value = reply.trim();
                    if value.len() >= 10
                        && value.len() <= 64
                        && value
                            .chars()
                            .all(|character| character.is_ascii_alphanumeric())
                    {
                        return Ok(value.to_string());
                    }
                    last = reply;
                }
                Err(error) => last = error.to_string(),
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        Err(TizenError::Duid(last))
    }

    async fn app_list(&self) -> Result<String, TizenError> {
        self.shell("0 vd_applist").await
    }

    async fn install(
        &self,
        package_id: &str,
        app_id: &str,
        package: &Path,
        staging: &str,
    ) -> Result<(), TizenError> {
        let bytes = std::fs::read(package)?;
        // `0 vd_appinstall` refuses to overwrite a staged file and reports
        // `download failed[16]`; a fresh, shell-safe name per attempt avoids
        // that. The reference client does not delete its staged package, and
        // `0 rmfile` is not a verified verb here.
        let remote = format!(
            "{}/impresario-{}-{}.wgt",
            staging.trim_end_matches('/'),
            std::process::id(),
            unix_ms_now()
        );
        let host = self.host.as_deref().ok_or(TizenError::NoHost)?;
        let home = self
            .home
            .as_ref()
            .ok_or_else(|| TizenError::Protocol("home not initialised".into()))?;
        let connection = SdbConnection::connect(host, self.port, home).await?;
        tracing::debug!(remote = %remote, bytes = bytes.len(), "Tizen install: pushing");
        connection.push(package, &remote).await?;
        tracing::debug!("Tizen install: push acknowledged");
        self.push_log(
            LogLevel::Info,
            format!("pushed {} bytes to {remote}", bytes.len()),
        );
        // `vd_appinstall` takes the package id, not the dotted app id.
        let reply = self
            .shell(&format!("0 vd_appinstall {package_id} {remote}"))
            .await?;
        self.push_log(LogLevel::Info, format!("vd_appinstall: {}", reply.trim()));
        // The TV has no reliable exit status. Treat explicit failure text as a
        // failure and, otherwise, require the app to appear in the app list
        // before reporting success.
        if reply_reports_failure(&reply) {
            return Err(TizenError::Command {
                command: "0 vd_appinstall".to_string(),
                reply: reply.trim().to_string(),
            });
        }
        let deadline = tokio::time::Instant::now() + INSTALL_TIMEOUT;
        loop {
            let list = self.app_list().await.unwrap_or_default();
            if app_list_contains(&list, app_id, package_id) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(TizenError::InstallUnverified(app_id.to_string()));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn debug_launch(&self, app_id: &str) -> Result<u16, TizenError> {
        let reply = self.shell(&format!("0 debug {app_id}")).await?;
        parse_debug_port(&reply)
            .ok_or_else(|| TizenError::NoInspectorPort(reply.trim().to_string()))
    }

    /// Build `NativeProbe` data from SDB without installing or launching.
    pub async fn probe_device(&mut self, device: &DeviceConfig) -> Result<NativeProbe, TizenError> {
        if device.platform != Platform::Tizen {
            return Err(TizenError::Protocol(format!(
                "TizenTransport cannot probe {}",
                device.platform
            )));
        }
        let host = device
            .connection
            .host
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(TizenError::NoHost)?
            .to_string();
        let home = Home::resolve().map_err(|error| {
            TizenError::Protocol(format!("cannot resolve IMPRESARIO home: {error}"))
        })?;
        home.ensure_dirs()
            .map_err(|error| TizenError::Protocol(format!("cannot initialise home: {error}")))?;
        self.host = Some(host);
        self.port = device
            .connection
            .port_or_default(Platform::Tizen)
            .unwrap_or(DEFAULT_SDB_PORT);
        self.home = Some(home.clone());
        self.explicit_endpoint = device.connection.endpoint.clone();

        let capabilities = self.capabilities().await?;
        let version = capabilities.platform_version().to_string();
        let mut details = vec![
            format!("Tizen {version}"),
            format!("SDK staging {}", capabilities.sdk_toolpath()),
            format!(
                "signing {}",
                if capabilities.requires_signing() {
                    "required (Tizen 7.0+)"
                } else {
                    "not required (Tizen < 7.0)"
                }
            ),
        ];
        let mut warnings = Vec::new();
        let duid = match self.duid().await {
            Ok(duid) => {
                details.push(format!("DUID {duid}"));
                Some(duid)
            }
            Err(error) => {
                warnings.push(error.to_string());
                None
            }
        };
        match self.app_list().await {
            Ok(list) => {
                let count = list.lines().filter(|line| !line.trim().is_empty()).count();
                details.push(format!("{count} app(s) reported by 0 vd_applist"));
            }
            Err(error) => warnings.push(format!("app list unavailable: {error}")),
        }
        if let Some(profile_name) = device
            .auth
            .profile
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            match crate::tizen_sign::show_profile(&home, profile_name) {
                Ok(record) => {
                    details.push(format!(
                        "profile '{profile_name}' valid until {}",
                        record.distributor_not_after
                    ));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_secs() as i64)
                        .unwrap_or(0);
                    if record.author_not_after_unix <= now
                        || record.distributor_not_after_unix <= now
                    {
                        warnings.push(format!(
                            "profile '{profile_name}' has an expired certificate"
                        ));
                    }
                    if let Some(duid) = duid.as_deref() {
                        if !record.duids.iter().any(|candidate| candidate == duid) {
                            warnings.push(format!(
                                "profile '{profile_name}' does not cover DUID {duid}"
                            ));
                        } else {
                            details.push(format!("profile covers DUID {duid}"));
                        }
                    }
                }
                Err(error) => warnings.push(error.to_string()),
            }
        } else if capabilities.requires_signing() {
            warnings.push(
                "Tizen 7.0+ needs a signing profile (device auth.profile is empty)".to_string(),
            );
        }

        let info = DeviceInfo {
            model: capabilities
                .get("model_name")
                .or_else(|| capabilities.get("model"))
                .filter(|value| !value.is_empty())
                .unwrap_or("Tizen TV")
                .to_string(),
            os_name: "Tizen".to_string(),
            os_version: version,
            screen_width_px: capabilities
                .get("screen_width")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            screen_height_px: capabilities
                .get("screen_height")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            pixel_ratio: 1.0,
            runtime: "SDB (native)".to_string(),
        };
        Ok(NativeProbe {
            info,
            details,
            warnings,
        })
    }

    async fn create_forward(&self, remote_port: u16) -> Result<(u16, Forward), TizenError> {
        let host = self.host.clone().ok_or(TizenError::NoHost)?;
        let port = self.port;
        let home = self
            .home
            .clone()
            .ok_or_else(|| TizenError::Protocol("home not initialised".into()))?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let local_port = listener.local_addr()?.port();
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let host = host.clone();
                let home = home.clone();
                tokio::spawn(async move {
                    // The TV's sdbd occasionally drops a handshake when several
                    // channels are opened in quick succession (page discovery
                    // plus the WebSocket). Retry before closing the local socket
                    // so the connector sees a transient failure, not a dead
                    // inspector endpoint.
                    let mut last_error = None;
                    for attempt in 0..3_u64 {
                        let connection = match SdbConnection::connect(&host, port, &home).await {
                            Ok(connection) => connection,
                            Err(error) => {
                                last_error = Some(error);
                                tokio::time::sleep(Duration::from_millis(150 * (attempt + 1)))
                                    .await;
                                continue;
                            }
                        };
                        match connection.open(&format!("tcp:{remote_port}")).await {
                            Ok(channel) => {
                                if let Err(error) = channel.proxy(socket).await {
                                    tracing::debug!(%error, remote_port, "Tizen forward ended");
                                }
                                return;
                            }
                            Err(error) => {
                                last_error = Some(error);
                                tokio::time::sleep(Duration::from_millis(150 * (attempt + 1)))
                                    .await;
                            }
                        }
                    }
                    if let Some(error) = last_error {
                        tracing::warn!(%error, remote_port, "Tizen forward connection failed");
                    }
                });
            }
        });
        Ok((local_port, Forward { task }))
    }
}

#[async_trait]
impl DeviceTransport for TizenTransport {
    async fn prepare(
        &mut self,
        device: &DeviceConfig,
        app: Option<&AppConfig>,
    ) -> Result<Endpoint, TransportError> {
        if device.platform != Platform::Tizen {
            return Err(TransportError::Transport(format!(
                "TizenTransport cannot prepare {}",
                device.platform
            )));
        }
        let app = app.ok_or_else(|| TransportError::from(TizenError::NoApp))?;
        let entry = app
            .platform(Platform::Tizen)
            .ok_or_else(|| TransportError::from(TizenError::NoApp))?;
        let app_id = entry
            .id
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| TransportError::from(TizenError::NoAppId))?
            .to_string();
        let host = device
            .connection
            .host
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| TransportError::from(TizenError::NoHost))?
            .to_string();
        let home = Home::resolve().map_err(|error| {
            TransportError::Transport(format!("cannot resolve IMPRESARIO home: {error}"))
        })?;
        home.ensure_dirs().map_err(|error| {
            TransportError::Transport(format!("cannot initialise IMPRESARIO home: {error}"))
        })?;
        self.host = Some(host.clone());
        self.port = device
            .connection
            .port_or_default(Platform::Tizen)
            .unwrap_or(DEFAULT_SDB_PORT);
        self.home = Some(home.clone());
        self.app_id = Some(app_id.clone());
        self.explicit_endpoint = device.connection.endpoint.clone();

        // Read capabilities before spawning the executor: Tizen < 7 installs
        // the original package and must not require a signing profile.
        let capabilities = self.capabilities().await.map_err(|error| {
            self.push_log(LogLevel::Error, error.to_string());
            TransportError::from(error)
        })?;
        let platform_version = capabilities.platform_version().to_string();
        let staging = capabilities.sdk_toolpath().to_string();
        let requires_signing = capabilities.requires_signing();
        self.push_log(
            LogLevel::Info,
            format!(
                "Tizen {platform_version}; signing {}; staging {staging}",
                if requires_signing {
                    "required"
                } else {
                    "not required"
                }
            ),
        );
        tracing::debug!("Tizen prepare: capabilities read");

        let endpoint = self.connector.prepare(device, Some(app)).await?;
        tracing::debug!("Tizen prepare: connector ready");

        if let Some(package) = entry.package.as_deref().filter(|value| !value.is_empty()) {
            let package = Path::new(package);
            if !package.is_file() {
                return Err(TransportError::Transport(format!(
                    "Tizen package {} is not a file",
                    package.display()
                )));
            }
            let package_id = crate::tizen_sign::inspect_wgt(package)
                .map_err(|error| TransportError::Transport(error.to_string()))?
                .package;
            self.push_log(LogLevel::Info, format!("package id {package_id}"));
            if requires_signing {
                let profile = device
                    .auth
                    .profile
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        TransportError::from(TizenError::Signing(format!(
                            "device '{}' runs Tizen {platform_version} and needs a signing profile; \
                             set device auth.profile and import one with \
                             `impresario sign tizen profile add`",
                            device.name
                        )))
                    })?;
                // The profile check needs the TV's DUID; failure here is a
                // real device/signing failure, not a reason to install an
                // unsigned artifact.
                let duid = self.duid().await.map_err(|error| {
                    self.push_log(LogLevel::Error, error.to_string());
                    TransportError::from(error)
                })?;
                self.push_log(LogLevel::Info, format!("DUID {duid}"));
                let signed =
                    crate::tizen_sign::sign_package_for_device(&home, profile, package, &duid)
                        .map_err(|error| {
                            self.push_log(LogLevel::Error, error.to_string());
                            TransportError::Transport(error.to_string())
                        })?;
                self.push_log(
                    LogLevel::Info,
                    format!("signed package cached at {}", signed.display()),
                );
                tracing::debug!(package = %signed.display(), "Tizen prepare: installing signed package");
                self.install(&package_id, &app_id, &signed, &staging)
                    .await
                    .map_err(|error| {
                        self.push_log(LogLevel::Error, error.to_string());
                        TransportError::from(error)
                    })?;
            } else {
                self.push_log(
                    LogLevel::Info,
                    format!(
                        "Tizen {platform_version} does not require re-signing; installing {}",
                        package.display()
                    ),
                );
                self.install(&package_id, &app_id, package, &staging)
                    .await
                    .map_err(|error| {
                        self.push_log(LogLevel::Error, error.to_string());
                        TransportError::from(error)
                    })?;
            }
            self.push_log(LogLevel::Info, format!("installed {app_id}"));
            tracing::debug!("Tizen prepare: install verified");
        }

        // A normal launch is enough for CDP on TVs where the inspector port is
        // always available; `0 debug` is the documented way to obtain it. Use
        // debug first, then fall back to the explicit endpoint/known launch.
        let inspector = match self.debug_launch(&app_id).await {
            Ok(port) => port,
            Err(error) => {
                self.push_log(
                    LogLevel::Warn,
                    format!("{error}; launching without a debug port query"),
                );
                let reply = self
                    .shell(&format!("0 was_execute {app_id}"))
                    .await
                    .map_err(|error| {
                        self.push_log(LogLevel::Error, error.to_string());
                        TransportError::from(error)
                    })?;
                self.push_log(LogLevel::Info, format!("was_execute: {}", reply.trim()));
                if reply_reports_failure(&reply) {
                    self.push_log(
                        LogLevel::Error,
                        format!("0 was_execute failed: {}", reply.trim()),
                    );
                    return Err(TransportError::from(TizenError::Command {
                        command: "0 was_execute".to_string(),
                        reply: reply.trim().to_string(),
                    }));
                }
                match explicit_inspector_port(device) {
                    Some(port) => port,
                    None => {
                        return Err(TransportError::from(error));
                    }
                }
            }
        };
        let (local_port, forward) = self.create_forward(inspector).await?;
        self.target = Some(format!("ws://127.0.0.1:{local_port}"));
        self.forward = Some(forward);
        self.push_log(
            LogLevel::Info,
            format!("inspector {inspector} forwarded to 127.0.0.1:{local_port}"),
        );
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
        Ok(ResetOutcome::NotRequested)
    }

    fn target_endpoint(&self, device: &DeviceConfig) -> String {
        self.target
            .clone()
            .or_else(|| self.explicit_endpoint.clone())
            .unwrap_or_else(|| device.target_endpoint())
    }

    async fn teardown(&mut self) -> Result<(), TransportError> {
        if let Some(forward) = self.forward.take() {
            forward.task.abort();
        }
        if let Some(app_id) = self.app_id.take() {
            match self.shell(&format!("0 was_kill {app_id}")).await {
                Ok(_) => self.push_log(LogLevel::Debug, format!("stopped {app_id}")),
                Err(error) => self.push_log(
                    LogLevel::Warn,
                    format!("could not stop {app_id} during teardown: {error}"),
                ),
            }
        }
        self.target = None;
        self.connector.teardown().await?;
        Ok(())
    }

    fn log_sources(&self) -> Vec<LogSource> {
        let mut sources = self.connector.log_sources();
        sources.push(LogSource::new("tizen", LogSourceKind::Host));
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

fn explicit_inspector_port(device: &DeviceConfig) -> Option<u16> {
    let endpoint = device.connection.endpoint.as_deref()?;
    if let Some(port) = endpoint
        .rsplit_once(':')
        .and_then(|(_, value)| value.trim_end_matches('/').parse::<u16>().ok())
    {
        return Some(port);
    }
    None
}

/// Parse `port: 48105` / `port:48105` from `0 debug`.
#[must_use]
fn reply_reports_failure(reply: &str) -> bool {
    let lowered = reply.to_ascii_lowercase();
    lowered.contains("fail") || lowered.contains("error") || lowered.contains("not exist")
}

/// Match an installed app in the TV's `0 vd_applist` output.
///
/// A Tizen 9.0 TV prints labelled fields such as
/// `--------------app_tizen_id =Impresario.TV-------------`; fake/replay
/// devices return one id per line. Match either shape exactly.
fn app_list_contains(list: &str, app_id: &str, package_id: &str) -> bool {
    let field = |line: &str, key: &str| -> Option<String> {
        let (name, value) = line.split_once('=')?;
        (name.trim_matches('-').trim() == key).then(|| {
            value
                .trim_matches(|character: char| {
                    character == '-'
                        || character == '"'
                        || character == '\''
                        || character.is_whitespace()
                })
                .to_string()
        })
    };
    let mut structured = false;
    for line in list.lines() {
        for (key, wanted) in [("app_tizen_id", app_id), ("app_package_name", package_id)] {
            if let Some(value) = field(line, key) {
                structured = true;
                if value == wanted {
                    return true;
                }
            }
        }
    }
    if structured {
        return false;
    }
    list.lines().any(|line| {
        line.split_whitespace()
            .any(|token| token.trim_matches(['"', '\'']) == app_id)
    })
}

pub fn parse_debug_port(reply: &str) -> Option<u16> {
    let lowered = reply.to_ascii_lowercase();
    let index = lowered.find("port")? + 4;
    let tail = reply.get(index..)?;
    let tail = tail.trim_start_matches(|character: char| {
        character == ':' || character.is_whitespace() || character == '='
    });
    let digits: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_have_adb_style_header_and_checksum() {
        let frame = SdbFrame {
            command: CMD_CNXN,
            arg0: 1,
            arg1: 2,
            payload: b"abc".to_vec(),
        };
        let encoded = frame.encode();
        assert_eq!(encoded.len(), 27);
        assert_eq!(&encoded[..4], &CMD_CNXN.to_le_bytes());
        assert_eq!(&encoded[12..16], &3u32.to_le_bytes());
        assert_eq!(&encoded[16..20], &(97 + 98 + 99u32).to_le_bytes());
        assert_eq!(&encoded[20..24], &(CMD_CNXN ^ 0xffff_ffff).to_le_bytes());
        let mut header = [0u8; 24];
        header.copy_from_slice(&encoded[..24]);
        let decoded = SdbFrame::decode(&header, encoded[24..].to_vec());
        assert_eq!(decoded.ok(), Some(frame));
    }

    #[test]
    fn frame_validation_rejects_bad_magic_and_checksum() {
        let frame = SdbFrame {
            command: CMD_OKAY,
            arg0: 1,
            arg1: 2,
            payload: b"x".to_vec(),
        };
        let encoded = frame.encode();
        let mut header = [0u8; 24];
        header.copy_from_slice(&encoded[..24]);
        header[20] ^= 1;
        assert!(SdbFrame::decode(&header, b"x".to_vec()).is_err());
        let mut header = [0u8; 24];
        header.copy_from_slice(&encoded[..24]);
        header[16] ^= 1;
        assert!(SdbFrame::decode(&header, b"x".to_vec()).is_err());
    }

    #[test]
    fn debug_port_is_parsed() {
        assert_eq!(
            parse_debug_port("successfully launched pid = 1257 with debug 1 port: 48105"),
            Some(48105)
        );
        assert_eq!(parse_debug_port("port=39819"), Some(39819));
        assert_eq!(parse_debug_port("nothing"), None);
    }

    #[test]
    fn app_list_matches_both_tv_and_replay_shapes() {
        let real = "--------------app_tizen_id                      =Impresario.TV-------------\n\
                    --------------app_package_name                  =Impresario-------------";
        assert!(app_list_contains(real, "Impresario.TV", "Impresario"));
        assert!(app_list_contains(real, "Other.TV", "Impresario"));
        assert!(!app_list_contains(real, "Other.TV", "Other"));

        assert!(app_list_contains(
            "com.example.test\ncom.example.other\n",
            "com.example.test",
            "com.example"
        ));
        assert!(!app_list_contains(
            "com.example.test\n",
            "com.example.tes",
            "com.example"
        ));
    }

    #[test]
    fn n0_inverse_is_correct_for_odd_values() {
        for value in [1u32, 3, 17, 0xffff_ffff, 0xdead_beef] {
            assert_eq!(
                value.wrapping_mul(inverse_mod_2_32(value)),
                1,
                "value {value:#x}"
            );
        }
    }

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct FakeSdbState {
        services: Mutex<Vec<String>>,
        uploads: Mutex<std::collections::HashMap<String, Vec<u8>>>,
        apps: Mutex<Vec<String>>,
        fail_install: AtomicBool,
        auths: AtomicUsize,
    }

    struct FakeSdbRunning {
        addr: std::net::SocketAddr,
        state: std::sync::Arc<FakeSdbState>,
        task: JoinHandle<()>,
    }

    impl Drop for FakeSdbRunning {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn read_test_frame(stream: &mut TcpStream) -> std::io::Result<SdbFrame> {
        let mut header = [0u8; 24];
        stream.read_exact(&mut header).await?;
        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(&header[12..16]);
        let length = u32::from_le_bytes(length_bytes) as usize;
        if length > 16 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "fixture payload too large",
            ));
        }
        let mut payload = vec![0u8; length];
        stream.read_exact(&mut payload).await?;
        SdbFrame::decode(&header, payload)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    async fn write_test_frame(stream: &mut TcpStream, frame: &SdbFrame) -> std::io::Result<()> {
        stream.write_all(&frame.encode()).await?;
        stream.flush().await
    }

    async fn send_test_channel(
        stream: &mut TcpStream,
        local: u32,
        remote: u32,
        payload: Vec<u8>,
    ) -> std::io::Result<()> {
        write_test_frame(
            stream,
            &SdbFrame {
                command: CMD_WRTE,
                arg0: remote,
                arg1: local,
                payload,
            },
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        write_test_frame(
            stream,
            &SdbFrame {
                command: CMD_CLSE,
                arg0: remote,
                arg1: local,
                payload: Vec::new(),
            },
        )
        .await?;
        // Drain the client's WRTE acknowledgement and CLSE reply before the
        // fixture socket closes. Closing with unread data turns into an RST on
        // Windows and can discard the CLSE frame the client is about to read.
        let _ = timeout(Duration::from_millis(500), async {
            loop {
                match read_test_frame(stream).await {
                    Ok(frame) if frame.command == CMD_CLSE => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        })
        .await;
        Ok(())
    }

    async fn fake_shell_reply(state: &FakeSdbState, command: &str) -> String {
        if command.contains("0 getduid") {
            return "DUID0123456789\n".to_string();
        }
        if command.contains("0 vd_applist") {
            return state.apps.lock().await.join("\n");
        }
        if command.contains("0 vd_appinstall ") {
            if state.fail_install.load(Ordering::SeqCst) {
                return "installation failed: invalid package\n".to_string();
            }
            let Some(app_id) = command.split_whitespace().nth(2) else {
                return "installation failed: missing app id\n".to_string();
            };
            let app_id = app_id.trim_matches(['"', '\'']).to_string();
            let mut apps = state.apps.lock().await;
            if !apps.contains(&app_id) {
                apps.push(app_id);
            }
            return "[100] install completed\n".to_string();
        }
        if command.contains("0 debug ") {
            return "successfully launched pid = 1257 with debug 1 port: 48105\n".to_string();
        }
        if command.contains("0 was_kill ") {
            return "killed\n".to_string();
        }
        if command.contains("0 vd_appinstall") {
            return "installation failed: malformed command\n".to_string();
        }
        String::new()
    }

    async fn handle_fake_sdb_connection(
        mut stream: TcpStream,
        state: std::sync::Arc<FakeSdbState>,
    ) -> std::io::Result<()> {
        let hello = read_test_frame(&mut stream).await?;
        if hello.command != CMD_CNXN {
            return Ok(());
        }
        state.auths.fetch_add(1, Ordering::SeqCst);
        write_test_frame(
            &mut stream,
            &SdbFrame {
                command: CMD_AUTH,
                arg0: AUTH_TOKEN,
                arg1: 0,
                payload: b"fake-token".to_vec(),
            },
        )
        .await?;
        let auth = read_test_frame(&mut stream).await?;
        if auth.command != CMD_AUTH || auth.arg0 != AUTH_SIGNATURE || auth.payload.len() != 256 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "fixture expected an RSA-SHA1 signature",
            ));
        }
        write_test_frame(
            &mut stream,
            &SdbFrame {
                command: CMD_CNXN,
                arg0: 1,
                arg1: MAX_DATA,
                payload: b"device::fake-tizen\0".to_vec(),
            },
        )
        .await?;

        let mut sync_path: Option<String> = None;
        let mut sync_data = Vec::new();
        loop {
            let frame = read_test_frame(&mut stream).await?;
            match frame.command {
                CMD_OPEN => {
                    let local = frame.arg0;
                    let remote = 41;
                    let service = String::from_utf8_lossy(&frame.payload)
                        .trim_end_matches('\0')
                        .to_string();
                    state.services.lock().await.push(service.clone());
                    write_test_frame(
                        &mut stream,
                        &SdbFrame {
                            command: CMD_OKAY,
                            arg0: remote,
                            arg1: local,
                            payload: Vec::new(),
                        },
                    )
                    .await?;
                    if service == "capability:" {
                        let body = b"platform_version:6.0\nmodel:QA55\nsdk_toolpath:/home/owner/share/tmp/sdk_tools/tmp\n";
                        let mut payload = u16::try_from(body.len())
                            .unwrap_or(0)
                            .to_be_bytes()
                            .to_vec();
                        payload.extend_from_slice(body);
                        send_test_channel(&mut stream, local, remote, payload).await?;
                        return Ok(());
                    }
                    if let Some(command) = service.strip_prefix("shell:") {
                        let reply = fake_shell_reply(&state, command).await;
                        send_test_channel(&mut stream, local, remote, reply.into_bytes()).await?;
                        return Ok(());
                    }
                    if service.starts_with("tcp:") {
                        send_test_channel(&mut stream, local, remote, b"tizen-remote".to_vec())
                            .await?;
                        return Ok(());
                    }
                    if service == "sync:" {
                        continue;
                    }
                    send_test_channel(&mut stream, local, remote, Vec::new()).await?;
                    return Ok(());
                }
                CMD_WRTE => {
                    let payload = &frame.payload;
                    let local = frame.arg0;
                    let remote = frame.arg1;
                    if payload.starts_with(b"SEND") && payload.len() >= 8 {
                        let mut length = [0u8; 4];
                        length.copy_from_slice(&payload[4..8]);
                        let length = u32::from_le_bytes(length) as usize;
                        if let Some(path) = payload.get(8..8 + length) {
                            sync_path = Some(String::from_utf8_lossy(path).into_owned());
                        }
                    } else if payload.starts_with(b"DATA") && payload.len() >= 8 {
                        let mut length = [0u8; 4];
                        length.copy_from_slice(&payload[4..8]);
                        let length = u32::from_le_bytes(length) as usize;
                        if let Some(data) = payload.get(8..8 + length) {
                            sync_data.extend_from_slice(data);
                        }
                    }
                    // Every WRTE is acknowledged, including SEND, DATA and
                    // DONE; a real TV drops the connection otherwise.
                    write_test_frame(
                        &mut stream,
                        &SdbFrame {
                            command: CMD_OKAY,
                            arg0: remote,
                            arg1: local,
                            payload: Vec::new(),
                        },
                    )
                    .await?;
                    if payload.starts_with(b"DONE") {
                        if let Some(path) = sync_path.take() {
                            state
                                .uploads
                                .lock()
                                .await
                                .insert(path, std::mem::take(&mut sync_data));
                        }
                        let mut response = b"OKAY".to_vec();
                        response.extend_from_slice(&0u32.to_le_bytes());
                        send_test_channel(&mut stream, local, remote, response).await?;
                        return Ok(());
                    }
                }
                CMD_CLSE => return Ok(()),
                _ => {}
            }
        }
    }

    impl FakeSdbRunning {
        async fn start() -> std::io::Result<Self> {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
            let addr = listener.local_addr()?;
            let state = std::sync::Arc::new(FakeSdbState::default());
            let task_state = std::sync::Arc::clone(&state);
            let task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let state = std::sync::Arc::clone(&task_state);
                    tokio::spawn(async move {
                        let _ = handle_fake_sdb_connection(stream, state).await;
                    });
                }
            });
            Ok(Self { addr, state, task })
        }
    }

    #[tokio::test]
    async fn fake_sdb_replays_auth_capability_push_install_launch_and_forwarding(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::AsyncReadExt as _;

        let server = FakeSdbRunning::start().await?;
        let temporary = tempfile::tempdir()?;
        let home = Home::at(temporary.path().join("home"));
        home.ensure_dirs()?;
        let package = temporary.path().join("app.wgt");
        std::fs::write(&package, b"fake-wgt")?;

        let mut transport = TizenTransport::new();
        transport.host = Some("127.0.0.1".to_string());
        transport.port = server.addr.port();
        transport.home = Some(home);

        let capabilities = transport.capabilities().await?;
        assert_eq!(capabilities.platform_version(), "6.0");
        assert_eq!(
            capabilities.sdk_toolpath(),
            "/home/owner/share/tmp/sdk_tools/tmp"
        );
        assert!(!capabilities.requires_signing());

        assert_eq!(transport.duid().await?, "DUID0123456789");
        assert_eq!(
            transport.shell("0 vd_applist").await?.trim(),
            "",
            "the fixture starts without the app"
        );
        transport
            .install(
                "com.example.test",
                "com.example.test",
                &package,
                capabilities.sdk_toolpath(),
            )
            .await?;
        assert!(transport
            .shell("0 vd_applist")
            .await?
            .contains("com.example.test"));
        assert_eq!(transport.debug_launch("com.example.test").await?, 48105);
        assert_eq!(
            transport.shell("0 was_kill com.example.test").await?.trim(),
            "killed"
        );
        let uploads = server.state.uploads.lock().await;
        assert_eq!(uploads.len(), 1);
        let (remote, uploaded) = uploads.iter().next().expect("one upload");
        assert!(
            remote.starts_with("/home/owner/share/tmp/sdk_tools/tmp/impresario-"),
            "unexpected staging path {remote}"
        );
        assert!(remote.ends_with(".wgt"));
        assert!(
            !remote.contains(' '),
            "Tizen rejects staging names with spaces"
        );
        assert_eq!(uploaded.as_slice(), b"fake-wgt");
        drop(uploads);

        let (local_port, forward) = transport.create_forward(48105).await?;
        let mut stream = TcpStream::connect(("127.0.0.1", local_port)).await?;
        let mut reply = [0u8; 12];
        timeout(Duration::from_secs(2), stream.read_exact(&mut reply)).await??;
        assert_eq!(&reply, b"tizen-remote");
        drop(forward);

        let services = server.state.services.lock().await.clone();
        assert!(services.iter().any(|service| service == "shell:0 getduid"));
        assert!(services.iter().any(|service| {
            service.starts_with(
                "shell:0 vd_appinstall com.example.test /home/owner/share/tmp/sdk_tools/tmp/impresario-",
            ) && service.ends_with(".wgt")
        }));
        assert!(services.iter().any(|service| service == "tcp:48105"));
        assert!(server.state.auths.load(Ordering::SeqCst) >= 4);

        server.state.fail_install.store(true, Ordering::SeqCst);
        let error = transport
            .install(
                "com.example.fail",
                "com.example.fail",
                &package,
                capabilities.sdk_toolpath(),
            )
            .await
            .expect_err("a failed install reply must not be reported as success");
        assert!(matches!(error, TizenError::Command { .. }));
        assert!(!transport
            .shell("0 vd_applist")
            .await?
            .contains("com.example.fail"));
        Ok(())
    }
}
