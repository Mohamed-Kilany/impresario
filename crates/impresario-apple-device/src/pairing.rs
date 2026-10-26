//! RemotePairing pairing records and first-time pairing.
//!
//! Pair records are machine-global: they hold the host's Ed25519 private key and
//! are stored under `~/.impresario/apple/pairing/`. They are never printed,
//! uploaded, or committed. The device recognizes the host by the public key;
//! the `identifier` is the stable host identity used in the pairing protocol.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use tokio::time::timeout;
use uuid::Uuid;

use crate::{Error, Result};

/// The mDNS service type advertised while a device shows a pairing code.
pub const MANUAL_PAIRING_SERVICE: &str = "_remotepairing-manual-pairing._tcp.local.";
/// The normal tvOS RemotePairing control service.
pub const REMOTE_PAIRING_SERVICE: &str = "_remotepairing._tcp.local.";
/// The tvOS RemotePairing control port, fixed by the platform.
pub const REMOTE_PAIRING_PORT: u16 = 49152;

/// Machine-global Apple paths inside `$IMPRESARIO_HOME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplePaths {
    root: PathBuf,
}

impl ApplePaths {
    /// `$IMPRESARIO_HOME/apple`.
    #[must_use]
    pub fn new(home: &Path) -> Self {
        Self {
            root: home.join("apple"),
        }
    }

    /// The Apple root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `apple/pairing/`.
    #[must_use]
    pub fn pairing_dir(&self) -> PathBuf {
        self.root.join("pairing")
    }

    /// `apple/ddi/`.
    #[must_use]
    pub fn ddi_dir(&self) -> PathBuf {
        self.root.join("ddi")
    }

    /// `apple/signing/`.
    #[must_use]
    pub fn signing_dir(&self) -> PathBuf {
        self.root.join("signing")
    }

    /// `apple/products/`.
    #[must_use]
    pub fn products_dir(&self) -> PathBuf {
        self.root.join("products")
    }

    /// `apple/products/<name>/`.
    #[must_use]
    pub fn product_set_dir(&self, name: &str) -> PathBuf {
        self.products_dir().join(safe_key(name))
    }

    /// `apple/ddi/<build>/`.
    #[must_use]
    pub fn ddi_build_dir(&self, build: &str) -> PathBuf {
        self.ddi_dir().join(safe_key(build))
    }

    /// `apple/tmp/`.
    #[must_use]
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The record for one device, keyed by UDID or host.
    #[must_use]
    pub fn pairing_path(&self, key: &str) -> PathBuf {
        self.pairing_dir().join(format!("{}.plist", safe_key(key)))
    }

    /// The stable host identity.
    ///
    /// `IMPRESARIO_APPLE_HOST_ID` wins so an existing record created by an
    /// earlier tool can still be used. Otherwise the identifier is the UUIDv3
    /// of the machine name, matching the pymobiledevice3 convention.
    #[must_use]
    pub fn host_identifier(&self) -> String {
        if let Some(value) = std::env::var_os("IMPRESARIO_APPLE_HOST_ID") {
            let value = value.to_string_lossy().trim().to_string();
            if !value.is_empty() {
                return value;
            }
        }
        host_identifier_from(machine_name().as_deref())
    }

    /// Load a saved pairing record for `key`.
    pub fn load_pairing(&self, key: &str) -> Result<RpPairingFile> {
        let path = self.pairing_path(key);
        load_pairing_file(&path)
    }

    /// Load a record by UDID, falling back to the device host or the host
    /// identifier used by an imported legacy record.
    pub fn load_pairing_for(&self, udid: Option<&str>, host: &str) -> Result<RpPairingFile> {
        let mut keys = Vec::new();
        if let Some(udid) = udid {
            keys.push(udid.to_string());
        }
        keys.push(host.to_string());
        keys.push(self.host_identifier());
        let mut errors = Vec::new();
        for key in keys {
            match self.load_pairing(&key) {
                Ok(file) => return Ok(file),
                Err(error) => {
                    errors.push(format!("{}: {error}", self.pairing_path(&key).display()));
                }
            }
        }
        Err(Error::Device(format!(
            "no usable pairing record for {host}; run `impresario apple pair --host {host}` \
             or import an existing record (tried {})",
            errors.join("; ")
        )))
    }

    /// Whether a pairing record exists under `key`.
    #[must_use]
    pub fn has_pairing(&self, key: &str) -> bool {
        self.pairing_path(key).is_file()
    }

    /// Save a pairing record for `key` with private-file permissions.
    pub fn save_pairing(&self, key: &str, file: &RpPairingFile) -> Result<()> {
        let path = self.pairing_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::io(format!("create {}", parent.display()), error))?;
        }
        write_private(&path, &file.to_bytes())
    }

    /// Import a legacy pair plist, assigning `identifier` when the source does
    /// not carry one. Returns the normalized record.
    pub fn import_pairing(&self, source: &Path, identifier: Option<&str>) -> Result<RpPairingFile> {
        let identifier = identifier.map_or_else(|| self.host_identifier(), str::to_string);
        load_pairing_file_with_identifier(source, &identifier)
    }

    /// Import a legacy pair plist and save it under `key` (normally the device
    /// UDID or host).
    pub fn import_pairing_to(
        &self,
        source: &Path,
        key: &str,
        identifier: Option<&str>,
    ) -> Result<RpPairingFile> {
        let file = self.import_pairing(source, identifier)?;
        self.save_pairing(key, &file)?;
        Ok(file)
    }
}

/// Pair with a tvOS device that is showing a code.
///
/// The `pin` future is only called when the device asks the host for a code;
/// tvOS shows the code on screen.
pub async fn pair_device<F, Fut>(
    host: &str,
    port: Option<u16>,
    paths: &ApplePaths,
    pin: F,
) -> Result<PairingSummary>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = String>,
{
    let port = match port {
        Some(port) => port,
        None => discover_manual_pairing_port(host, Duration::from_secs(15)).await?,
    };
    let stream = tokio::net::TcpStream::connect((host, port))
        .await
        .map_err(|error| Error::io(format!("connect to {host}:{port}"), error))?;
    let socket = idevice::remote_pairing::RpPairingSocket::new(stream);
    let mut client = idevice::remote_pairing::RemotePairingClient::new(socket, "Impresario");
    let mut file = RpPairingFile::generate(&paths.host_identifier());
    file.identifier = paths.host_identifier();

    client
        .pair(&mut file, pin)
        .await
        .map_err(|error| Error::Device(format!("pairing failed: {error}")))?;

    let peer = client.paired_peer_device().ok();
    let udid = peer
        .as_ref()
        .map(|peer| peer.remotepairing_udid.clone())
        .filter(|udid| !udid.trim().is_empty());
    // Key by device, not by host: the same host pairs with several devices and
    // a host-id filename would overwrite them.
    let key = udid.as_deref().unwrap_or(&file.identifier).to_string();
    paths.save_pairing(&key, &file)?;
    Ok(PairingSummary {
        identifier: file.identifier.clone(),
        saved_as: key,
        name: peer.as_ref().map(|peer| peer.name.clone()),
        model: peer.as_ref().map(|peer| peer.model.clone()),
        udid,
        port,
    })
}

/// A successful pairing, suitable for a one-line CLI result.
#[derive(Debug, Clone)]
pub struct PairingSummary {
    pub identifier: String,
    /// Filename key under `apple/pairing/` (the UDID when the device reports it).
    pub saved_as: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub udid: Option<String>,
    pub port: u16,
}

/// Discover the manual-pairing port over mDNS, filtering by the device host.
pub async fn discover_manual_pairing_port(host: &str, wait: Duration) -> Result<u16> {
    let daemon = ServiceDaemon::new()
        .map_err(|error| Error::Device(format!("cannot start mDNS: {error}")))?;
    let receiver = daemon
        .browse(MANUAL_PAIRING_SERVICE)
        .map_err(|error| Error::Device(format!("cannot browse mDNS: {error}")))?;
    let wanted = host_to_ips(host).await;
    let found = timeout(wait, async {
        while let Ok(event) = receiver.recv_async().await {
            let ServiceEvent::ServiceResolved(info) = event else {
                continue;
            };
            if wanted.is_empty()
                || info
                    .get_addresses_v4()
                    .iter()
                    .any(|address| wanted.contains(&std::net::IpAddr::V4(*address)))
            {
                return Some(info.port);
            }
        }
        None
    })
    .await;
    let _ = daemon.shutdown();
    match found {
        Ok(Some(port)) => Ok(port),
        Ok(None) => Err(Error::Device(
            "no _remotepairing-manual-pairing._tcp service was found; \
             put the Apple TV into pairing mode or pass --port"
                .into(),
        )),
        Err(_) => Err(Error::Device(
            "timed out waiting for the Apple TV pairing service; \
             put the Apple TV into pairing mode or pass --port"
                .into(),
        )),
    }
}

async fn host_to_ips(host: &str) -> Vec<std::net::IpAddr> {
    if let Ok(ip) = host.parse() {
        return vec![ip];
    }
    tokio::net::lookup_host((host, 0))
        .await
        .map(|addrs| addrs.map(|addr| addr.ip()).collect())
        .unwrap_or_default()
}

/// Load the normalized pairing record from a legacy plist.
pub fn load_pairing_file(path: &Path) -> Result<RpPairingFile> {
    let identifier = read_raw_identifier(path)?;
    match identifier {
        Some(identifier) => load_pairing_file_with_identifier(path, &identifier),
        None => Err(Error::Device(format!(
            "{} has no identifier; import it with --host-id",
            path.display()
        ))),
    }
}

/// Load a pair plist, forcing `identifier` when the source lacks one.
pub fn load_pairing_file_with_identifier(path: &Path, identifier: &str) -> Result<RpPairingFile> {
    let value = plist::Value::from_file(path)
        .map_err(|error| Error::Device(format!("read {}: {error}", path.display())))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| Error::Device(format!("{} is not a plist dictionary", path.display())))?;
    let public_key = dict
        .get("public_key")
        .cloned()
        .ok_or_else(|| Error::Device(format!("{} is missing public_key", path.display())))?;
    let private_key = dict
        .get("private_key")
        .cloned()
        .ok_or_else(|| Error::Device(format!("{} is missing private_key", path.display())))?;
    let mut out = plist::Dictionary::new();
    out.insert("public_key".into(), public_key);
    out.insert("private_key".into(), private_key);
    if let Some(irk) = dict.get("peer_alt_irk").or_else(|| dict.get("alt_irk")) {
        out.insert("alt_irk".into(), irk.clone());
    }
    out.insert(
        "identifier".into(),
        plist::Value::String(identifier.to_string()),
    );
    let mut bytes = Vec::new();
    plist::to_writer_xml(&mut bytes, &plist::Value::Dictionary(out))?;
    RpPairingFile::from_bytes(&bytes)
        .map_err(|error| Error::Device(format!("invalid pairing record: {error}")))
}

fn read_raw_identifier(path: &Path) -> Result<Option<String>> {
    let value = plist::Value::from_file(path)
        .map_err(|error| Error::Device(format!("read {}: {error}", path.display())))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| Error::Device(format!("{} is not a plist dictionary", path.display())))?;
    Ok(dict
        .get("identifier")
        .or_else(|| dict.get("host_identifier"))
        .and_then(plist::Value::as_string)
        .map(str::to_string))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)
        .map_err(|error| Error::io(format!("write {}", tmp.display()), error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| Error::io(format!("chmod {}", tmp.display()), error))?;
    }
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|error| Error::io(format!("replace {}", path.display()), error))?;
    }
    std::fs::rename(&tmp, path)
        .map_err(|error| Error::io(format!("rename {}", path.display()), error))
}

/// A UUIDv3 identifier for a machine name, stable across runs.
#[must_use]
pub fn host_identifier_from(machine: Option<&str>) -> String {
    let machine = machine.unwrap_or("impresario");
    Uuid::new_v3(&Uuid::NAMESPACE_DNS, machine.as_bytes()).to_string()
}

fn machine_name() -> Option<String> {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Some(value) = std::env::var_os(key) {
            let value: OsString = value;
            let value = value.to_string_lossy().trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn safe_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for character in key.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            out.push(character);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        "device".to_string()
    } else {
        out
    }
}

/// The pairing record type, re-exported so callers do not depend on `idevice`.
pub use idevice::remote_pairing::RpPairingFile;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_identifier_is_stable_and_canonical() {
        let first = host_identifier_from(Some("TestCrew-Kilany"));
        let second = host_identifier_from(Some("TestCrew-Kilany"));
        assert_eq!(first, second);
        let parsed = Uuid::parse_str(&first).expect("UUID");
        assert_eq!(parsed.get_version_num(), 3);
    }

    #[test]
    fn pairing_path_rejects_traversal() {
        let paths = ApplePaths::new(Path::new("/home/test/.impresario"));
        let path = paths.pairing_path("../../etc/passwd");
        assert_eq!(path.parent(), Some(paths.pairing_dir().as_path()));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".._.._etc_passwd.plist")
        );
    }

    #[test]
    fn pair_plist_is_normalized_without_leaking_keys() {
        let dir = std::env::temp_dir().join(format!(
            "impresario-apple-pair-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = RpPairingFile::generate("test-host");
        let source = dir.join("legacy.plist");
        std::fs::write(&source, file.to_bytes()).unwrap();
        let loaded = load_pairing_file_with_identifier(&source, "host-identifier").unwrap();
        assert_eq!(loaded.identifier, "host-identifier");
        assert_eq!(loaded.public_key_bytes(), file.public_key_bytes());
        assert!(!format!("{loaded:?}").contains("private"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
