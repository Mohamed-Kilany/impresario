//! RemotePairing pairing records and first-time pairing.
//!
//! Pair records are machine-global: they hold the host's Ed25519 private key and
//! are stored under `~/.impresario/apple/pairing/`. They are never printed,
//! uploaded, or committed. The device recognizes the host by the public key;
//! the `identifier` is the stable host identity used in the pairing protocol.

use std::collections::HashMap;
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

use idevice::remote_pairing::{
    PairableHost, PairableHostInfo, RemotePairingLockdownService, RpPairingSocket,
    PAIRABLE_HOST_SERVICE_TYPE,
};
use idevice::services::lockdown::LockdownClient;
use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::time::timeout;
use uuid::Uuid;

use crate::{Error, Result};

/// The mDNS service type advertised while a device shows a pairing code.
pub const MANUAL_PAIRING_SERVICE: &str = "_remotepairing-manual-pairing._tcp.local.";
/// The stable host name Impresario presents to Apple devices.
///
/// This is the name shown by the device during pairing (and registered with
/// the Apple Developer portal); it is deliberately not the machine name or a
/// leftover `pyatv` identifier.
pub const IMPRESARIO_DEVICE_NAME: &str = "impresario";

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

    /// The host's own 16-byte mDNS identity key, created on first use.
    ///
    /// A device that pairs through the iOS 27 "Paired Macs" flow remembers this
    /// key and uses it to recognize Impresario on later visits, so it must stay
    /// stable across runs. It is not a pairing record and is never advertised
    /// on its own.
    pub fn host_alt_irk(&self) -> Result<[u8; 16]> {
        let path = self.root.join("pairing-host-alt-irk");
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(irk) = <[u8; 16]>::try_from(bytes.as_slice()) {
                return Ok(irk);
            }
        }
        let irk: [u8; 16] = rand::random();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::io(format!("create {}", parent.display()), error))?;
        }
        write_private(&path, &irk)?;
        Ok(irk)
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
        // Older pairings are keyed by UDID, but the public device schema has
        // only the host/IP. If exactly one pairing is stored, use it; with
        // several, the host must be named explicitly so we never guess.
        if udid.is_none() {
            let stored = self.stored_pairing_paths()?;
            match stored.as_slice() {
                [path] => match load_pairing_file(path) {
                    Ok(file) => return Ok(file),
                    Err(error) => errors.push(format!("{}: {error}", path.display())),
                },
                [] => {}
                paths => {
                    let names = paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    errors.push(format!(
                        "several pairing records are stored ({names}); import the intended one \
                         with `impresario apple import-pair <plist> --save {host}` or remove the \
                         stale records"
                    ));
                }
            }
        }
        Err(Error::Device(format!(
            "no usable pairing record for {host}; run `impresario apple pair --host {host}` \
             or import an existing record (tried {})",
            errors.join("; ")
        )))
    }

    /// Pairing files currently stored in the machine-global pairing directory.
    fn stored_pairing_paths(&self) -> Result<Vec<PathBuf>> {
        let dir = self.pairing_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(Error::io(format!("read {}", dir.display()), error)),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|error| Error::io(format!("read {}", dir.display()), error))?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("plist") {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
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
    let mut client =
        idevice::remote_pairing::RemotePairingClient::new(socket, IMPRESARIO_DEVICE_NAME);
    let mut file = RpPairingFile::generate(&paths.host_identifier());
    file.identifier = paths.host_identifier();

    // The device's manual-pairing service expects the RPPairing handshake and a
    // pair-verify attempt before pair-setup. Without them tvOS closes the
    // socket as soon as it receives `setupManualPairing`.
    client
        .connect(&mut file, pin)
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

/// Serve a device-initiated pairing (the iOS 27 "Paired Macs" flow).
///
/// Unlike [`pair_device`], the host advertises
/// `_remotepairing-pairable-host._tcp` and the device connects. The device
/// displays the host under "Other Devices"; `pin` receives the six-digit code
/// the user must enter there.
pub async fn pair_host<F, Fut>(
    paths: &ApplePaths,
    name: &str,
    model: &str,
    save: Option<&str>,
    pin: F,
) -> Result<PairingSummary>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|error| Error::io("bind the pairable-host listener", error))?;
    let port = listener
        .local_addr()
        .map_err(|error| Error::io("read the pairable-host listener address", error))?
        .port();
    let local_ip = lan_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let host_id = paths.host_identifier();
    let mut info = PairableHostInfo::generate(name, model);
    info.identifier = host_id.clone();
    info.alt_irk = paths.host_alt_irk()?;

    let daemon = ServiceDaemon::new()
        .map_err(|error| Error::Device(format!("cannot start mDNS advertiser: {error}")))?;
    // mdns-sd's default 15-byte cap covers ordinary `_http._tcp`; Apple's
    // pairable-host service type is longer, so use the daemon's documented
    // maximum instead of silently failing to register.
    daemon
        .set_service_name_len_max(30)
        .map_err(|error| Error::Device(format!("cannot advertise the pairable host: {error}")))?;
    let properties: HashMap<String, String> = info.mdns_txt_records(&host_id).into_iter().collect();
    let host_name = format!(
        "{}.local.",
        machine_name().unwrap_or_else(|| "impresario".to_string())
    );
    let service = ServiceInfo::new(
        PAIRABLE_HOST_SERVICE_TYPE,
        &host_id,
        &host_name,
        local_ip,
        port,
        properties,
    )
    .map_err(|error| Error::Device(format!("cannot advertise the pairable host: {error}")))?;
    daemon
        .register(service)
        .map_err(|error| Error::Device(format!("cannot register the pairable host: {error}")))?;

    let accepted = listener.accept().await;
    let (stream, peer_addr) =
        accepted.map_err(|error| Error::io("accept the device pairing connection", error))?;
    tracing::info!(%peer_addr, "device connected for pairing");

    let mut file = RpPairingFile::generate(&host_id);
    file.identifier = host_id.clone();
    let mut host = PairableHost::new(RpPairingSocket::new_device(stream), info);
    let peer = match host.accept(&mut file, pin).await {
        Ok(peer) => peer,
        Err(error) => {
            let _ = daemon.shutdown();
            return Err(Error::Device(format!("pairing failed: {error}")));
        }
    };
    let _ = daemon.shutdown();

    let udid = peer.remotepairing_udid.trim().to_string();
    let key = save.map(str::to_string).or_else(|| {
        if udid.is_empty() {
            None
        } else {
            Some(udid.clone())
        }
    });
    let key = key.ok_or_else(|| {
        Error::Device(
            "the device did not report a UDID; pass --save to choose the pairing key".to_string(),
        )
    })?;
    paths.save_pairing(&key, &file)?;
    Ok(PairingSummary {
        identifier: host_id,
        saved_as: key,
        name: Some(peer.name),
        model: Some(peer.model),
        udid: (!udid.is_empty()).then_some(udid),
        port,
    })
}

/// The lockdown service that fronts RemotePairing on iOS/iPadOS.
///
/// Opening it over an already-trusted USB lockdown session and running
/// RPPairing pair-setup there is promptless: the device has accepted this
/// computer for lockdown, so it does not show the Trust dialog again
/// (pymobiledevice3 calls the same flow `lockdown remotepairing --pair`).
pub const REMOTE_PAIRING_LOCKDOWN_SERVICE: &str = "com.apple.dt.remotepairingdeviced.lockdown";

/// Pair an iPhone/iPad over USB through Apple's usbmuxd.
///
/// The lockdown step trusts this computer (the device shows its Trust dialog
/// only when no record exists), then the
/// [`REMOTE_PAIRING_LOCKDOWN_SERVICE`] control channel creates the
/// RemotePairing record that the Wi-Fi transport on [`REMOTE_PAIRING_PORT`]
/// uses. The record is saved under the device UDID, so the device works over
/// Wi-Fi after the cable is removed.
pub async fn pair_usb(
    paths: &ApplePaths,
    udid: Option<&str>,
    host_name: &str,
    wait: Duration,
) -> Result<PairingSummary> {
    let device = {
        let mut mux = connect_usbmuxd().await?;
        let devices = mux
            .get_devices()
            .await
            .map_err(|error| Error::Device(format!("list USB devices: {error}")))?;
        select_usb_device(&devices, udid)?.clone()
    };

    // A cached lockdown record can be stale (for example after an iOS restore or
    // "Reset Location & Privacy"); the device then answers `InvalidHostID`. Drop
    // the cached record and ask for Trust once more instead of failing.
    let (mut lockdown, lockdown_pair, legacy) = {
        let mut repaired = false;
        loop {
            let pair = lockdown_pair_record(&device, paths, host_name, wait).await?;
            let stream = {
                let mut mux = connect_usbmuxd().await?;
                mux.connect_to_device(
                    device.device_id,
                    LockdownClient::LOCKDOWND_PORT,
                    IMPRESARIO_DEVICE_NAME,
                )
                .await
                .map_err(|error| {
                    Error::Device(format!("connect to lockdown on {}: {error}", device.udid))
                })?
            };
            let mut lockdown = LockdownClient::new(stream);
            match lockdown.start_session(&pair).await {
                Ok(legacy) => break (lockdown, pair, legacy),
                Err(idevice::IdeviceError::InvalidHostID) if !repaired => {
                    repaired = true;
                    let mut mux = connect_usbmuxd().await?;
                    mux.delete_pair_record(&device.udid)
                        .await
                        .map_err(|error| {
                            Error::Device(format!(
                                "forget the stale lockdown record for {}: {error}",
                                device.udid
                            ))
                        })?;
                }
                Err(error) => {
                    return Err(Error::Device(format!(
                        "start a lockdown session with {}: {error}",
                        device.udid
                    )))
                }
            }
        }
    };

    // Start the RemotePairing control service on the trusted lockdown session.
    // Keep `lockdown` alive until the handshake finishes: the device ties the
    // started service to the session. Read the identifying values first so a
    // record that verifies (no pair-setup) still reports a name and model.
    let device_name = lockdown
        .get_value(Some("DeviceName"), None)
        .await
        .ok()
        .and_then(|value| value.as_string().map(str::to_string));
    let device_model = lockdown
        .get_value(Some("ProductType"), None)
        .await
        .ok()
        .and_then(|value| value.as_string().map(str::to_string));
    let (port, ssl) = lockdown
        .start_service(REMOTE_PAIRING_LOCKDOWN_SERVICE)
        .await
        .map_err(|error| {
            Error::Device(format!(
                "start {REMOTE_PAIRING_LOCKDOWN_SERVICE} on {}: {error}; update iOS if the \
                 service is unavailable",
                device.udid
            ))
        })?;
    let mut stream = {
        let mut mux = connect_usbmuxd().await?;
        mux.connect_to_device(device.device_id, port, IMPRESARIO_DEVICE_NAME)
            .await
            .map_err(|error| {
                Error::Device(format!(
                    "connect to the RemotePairing control channel on {}: {error}",
                    device.udid
                ))
            })?
    };
    if ssl {
        stream
            .start_session(&lockdown_pair, legacy)
            .await
            .map_err(|error| {
                Error::Device(format!(
                    "secure the RemotePairing control channel on {}: {error}",
                    device.udid
                ))
            })?;
    }
    let service = RemotePairingLockdownService::new(stream);
    let mut client = service.into_client(host_name).map_err(|error| {
        Error::Device(format!(
            "prepare the RemotePairing handshake on {}: {error}",
            device.udid
        ))
    })?;

    // The lockdown transport is already trusted, so the device returns the SRP
    // salt immediately and never asks for a code. Reuse a stored record when it
    // still verifies; `connect` falls back to pair-setup otherwise. iOS uses the
    // fixed "000000" PIN for this promptless flow.
    let mut file = match paths.load_pairing(&device.udid) {
        Ok(file) => file,
        Err(_) => {
            let mut file = RpPairingFile::generate(&paths.host_identifier());
            file.identifier = paths.host_identifier();
            file
        }
    };
    timeout(
        wait,
        client.connect(&mut file, || async { "000000".to_string() }),
    )
    .await
    .map_err(|_| {
        Error::Device(
            "timed out pairing the RemotePairing control channel; unlock the device and retry"
                .to_string(),
        )
    })?
    .map_err(|error| match error {
        idevice::IdeviceError::UserDeniedPairing => Error::Device(
            "the device denied the pairing request; unlock it, tap Trust, and retry".to_string(),
        ),
        idevice::IdeviceError::PasswordProtected => {
            Error::Device("the device is locked; unlock it, keep it awake, and retry".to_string())
        }
        other => Error::Device(format!(
            "RemotePairing pair-setup failed: {other}; update iOS and retry"
        )),
    })?;

    let peer = client.paired_peer_device().ok();
    let remote_udid = peer
        .as_ref()
        .map(|peer| peer.remotepairing_udid.trim().to_string())
        .filter(|value| !value.is_empty());
    let key = remote_udid.clone().unwrap_or_else(|| device.udid.clone());
    paths.save_pairing(&key, &file)?;
    Ok(PairingSummary {
        identifier: file.identifier.clone(),
        saved_as: key,
        name: peer.as_ref().map(|peer| peer.name.clone()).or(device_name),
        model: peer
            .as_ref()
            .map(|peer| peer.model.clone())
            .or(device_model),
        udid: remote_udid.or(Some(device.udid.clone())),
        port: REMOTE_PAIRING_PORT,
    })
}

/// Connect to Apple's usbmuxd service (Apple Mobile Device Support on Windows).
async fn connect_usbmuxd() -> Result<UsbmuxdConnection> {
    let address = UsbmuxdAddr::from_env_var().map_err(|error| {
        Error::Device(format!(
            "USBMUXD_SOCKET_ADDRESS is not a valid address: {error}"
        ))
    })?;
    address.connect(0).await.map_err(|error| {
        Error::Device(format!(
            "cannot reach Apple's usbmuxd service ({error}); install and start Apple Mobile \
             Device Support (it ships with iTunes) or set USBMUXD_SOCKET_ADDRESS"
        ))
    })
}

fn select_usb_device<'a>(
    devices: &'a [UsbmuxdDevice],
    udid: Option<&str>,
) -> Result<&'a UsbmuxdDevice> {
    let wanted = udid.map(str::trim).filter(|value| !value.is_empty());
    if let Some(wanted) = wanted {
        let matches = |device: &&UsbmuxdDevice| device.udid.eq_ignore_ascii_case(wanted);
        return devices
            .iter()
            .filter(matches)
            .find(|device| matches!(&device.connection_type, Connection::Usb))
            .ok_or_else(|| {
                Error::Device(format!(
                    "no Apple device with UDID {wanted} is attached over USB through usbmuxd \
                     (attached: {}); connect the cable and retry",
                    describe_devices(devices)
                ))
            });
    }
    let usb: Vec<_> = devices
        .iter()
        .filter(|device| matches!(&device.connection_type, Connection::Usb))
        .collect();
    match usb.as_slice() {
        [device] => Ok(*device),
        [] if devices.is_empty() => Err(Error::Device(
            "no Apple device is attached over USB; connect the iPhone/iPad, unlock it, and retry"
                .to_string(),
        )),
        [] => Err(Error::Device(format!(
            "no Apple device is attached over USB (usbmuxd sees {}); connect the iPhone/iPad, \
             unlock it, and retry",
            describe_devices(devices)
        ))),
        many => {
            let choices = many
                .iter()
                .map(|device| device.udid.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(Error::Device(format!(
                "several Apple devices are attached over USB ({choices}); pass --udid"
            )))
        }
    }
}

fn describe_devices(devices: &[UsbmuxdDevice]) -> String {
    if devices.is_empty() {
        return "none".to_string();
    }
    devices
        .iter()
        .map(|device| {
            let connection = match &device.connection_type {
                Connection::Usb => "USB",
                Connection::Network(_) => "Network",
                Connection::Unknown(_) => "unknown",
            };
            format!("{} ({connection})", device.udid)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The lockdown pair record for `device`, creating and caching one when the
/// device has never trusted this computer.
async fn lockdown_pair_record(
    device: &UsbmuxdDevice,
    paths: &ApplePaths,
    host_name: &str,
    wait: Duration,
) -> Result<idevice::pairing_file::PairingFile> {
    let mut mux = connect_usbmuxd().await?;
    if let Ok(record) = mux.get_pair_record(&device.udid).await {
        return Ok(record);
    }

    let buid = connect_usbmuxd()
        .await?
        .get_buid()
        .await
        .map_err(|error| Error::Device(format!("read the usbmuxd BUID: {error}")))?;
    let stream = {
        let mut mux = connect_usbmuxd().await?;
        mux.connect_to_device(
            device.device_id,
            LockdownClient::LOCKDOWND_PORT,
            IMPRESARIO_DEVICE_NAME,
        )
        .await
        .map_err(|error| {
            Error::Device(format!("connect to lockdown on {}: {error}", device.udid))
        })?
    };
    let mut lockdown = LockdownClient::new(stream);
    let host_id = paths.host_identifier();
    let paired = timeout(
        wait,
        lockdown.pair(host_id.as_str(), buid.as_str(), Some(host_name)),
    )
    .await
    .map_err(|_| {
        Error::Device(
            "timed out waiting for the device to trust this computer; unlock the iPhone/iPad, \
             tap Trust, keep it awake, and retry"
                .to_string(),
        )
    })?
    .map_err(|error| match error {
        idevice::IdeviceError::UserDeniedPairing => Error::Device(
            "the device denied the trust request; unlock it, tap Trust, and retry".to_string(),
        ),
        idevice::IdeviceError::PasswordProtected => {
            Error::Device("the device is locked; unlock it, keep it awake, and retry".to_string())
        }
        other => Error::Device(format!("lockdown trust failed: {other}")),
    })?;

    let bytes = paired
        .clone()
        .serialize()
        .map_err(|error| Error::Device(format!("serialize the lockdown pair record: {error}")))?;
    let mut mux = connect_usbmuxd().await?;
    mux.save_pair_record(&device.udid, bytes)
        .await
        .map_err(|error| {
            Error::Device(format!(
                "cache the lockdown pair record for {} in usbmuxd: {error}",
                device.udid
            ))
        })?;
    Ok(paired)
}

/// The LAN address used for mDNS adverts, picked by the default route.
fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // Connecting a UDP socket only selects a route; no packet is sent.
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_unspecified() && !address.is_loopback()).then_some(address)
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
    fn load_pairing_for_uses_a_single_stored_record() {
        let dir = std::env::temp_dir().join(format!(
            "impresario-apple-pair-lookup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let paths = ApplePaths::new(&dir);
        let file = RpPairingFile::generate("test-host");
        paths
            .save_pairing("00008110-001A2345021E001E", &file)
            .unwrap();
        let loaded = paths
            .load_pairing_for(None, "192.168.1.70")
            .expect("single stored pairing");
        assert_eq!(loaded.public_key_bytes(), file.public_key_bytes());
        std::fs::remove_dir_all(dir).unwrap();
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

    fn usb_device(udid: &str) -> UsbmuxdDevice {
        UsbmuxdDevice {
            connection_type: Connection::Usb,
            udid: udid.to_string(),
            device_id: 1,
        }
    }

    #[test]
    fn usb_pairing_prefers_the_usb_entry_for_a_named_udid() {
        let devices = [
            UsbmuxdDevice {
                connection_type: Connection::Network("10.0.0.9".parse().unwrap()),
                udid: "DEVICE-1".to_string(),
                device_id: 1,
            },
            usb_device("DEVICE-1"),
        ];
        let selected = select_usb_device(&devices, Some("device-1")).unwrap();
        assert!(matches!(&selected.connection_type, Connection::Usb));
    }

    #[test]
    fn usb_pairing_refuses_to_guess_between_two_devices() {
        let devices = [usb_device("DEVICE-1"), usb_device("DEVICE-2")];
        let error = select_usb_device(&devices, None).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("several Apple devices"), "{message}");
        assert!(message.contains("DEVICE-1, DEVICE-2"), "{message}");
    }

    #[test]
    fn usb_pairing_rejects_a_named_device_that_is_only_on_the_network() {
        let devices = [UsbmuxdDevice {
            connection_type: Connection::Network("10.0.0.9".parse().unwrap()),
            udid: "DEVICE-1".to_string(),
            device_id: 1,
        }];
        let error = select_usb_device(&devices, Some("DEVICE-1")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("attached over USB"), "{message}");
        assert!(message.contains("DEVICE-1 (Network)"), "{message}");
    }

    #[test]
    fn usb_pairing_explains_that_a_network_only_device_needs_a_cable() {
        let devices = [UsbmuxdDevice {
            connection_type: Connection::Network("10.0.0.9".parse().unwrap()),
            udid: "DEVICE-1".to_string(),
            device_id: 1,
        }];
        let error = select_usb_device(&devices, None).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("no Apple device is attached over USB"),
            "{message}"
        );
        assert!(message.contains("DEVICE-1 (Network)"), "{message}");
    }
}
