//! The RemotePairing control connection and its RSD tunnel.
//!
//! A tvOS tunnel has two connections: the pair-verified control connection on
//! `_remotepairing._tcp`, then the TLS-PSK listener that carries the RSD/DTX
//! traffic. The control connection must stay alive for as long as the tunnel.

use std::time::Duration;

use idevice::provider::RsdProvider as _;
use idevice::remote_pairing::{
    connect_tls_psk_tunnel_native, RemotePairingClient, RpPairingSocket,
};
use idevice::services::rsd::RsdHandshake;
use idevice::tcp::adapter::Adapter;
use idevice::tcp::handle::AdapterHandle;

use crate::pairing::{RpPairingFile as PairingFile, IMPRESARIO_DEVICE_NAME, REMOTE_PAIRING_PORT};
use crate::{Error, Result};

/// A live RSD tunnel to an Apple device.
pub struct AppleTunnel {
    /// Dropping this closes the control connection and the TLS-PSK tunnel.
    _control: RemotePairingClient<RpPairingSocket<tokio::net::TcpStream>>,
    /// Service-port connections on the tunnel.
    pub handle: AdapterHandle,
    /// RSD service table and device properties.
    pub handshake: RsdHandshake,
    /// The device host used for the control connection.
    pub host: String,
}

impl std::fmt::Debug for AppleTunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleTunnel")
            .field("host", &self.host)
            .field("services", &self.handshake.services.len())
            .finish_non_exhaustive()
    }
}

impl AppleTunnel {
    /// Pair-verify `pairing` and open the RSD tunnel on `host`.
    pub async fn connect(host: &str, pairing: &PairingFile) -> Result<Self> {
        Self::connect_on(host, REMOTE_PAIRING_PORT, pairing).await
    }

    /// Open the tunnel on an explicit RemotePairing control port.
    pub async fn connect_on(host: &str, port: u16, pairing: &PairingFile) -> Result<Self> {
        let stream = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::net::TcpStream::connect((host, port)),
        )
        .await
        .map_err(|_| Error::Device(format!("timed out connecting to {host}:{port}")))?
        .map_err(|error| Error::io(format!("connect to {host}:{port}"), error))?;
        let mut control =
            RemotePairingClient::new(RpPairingSocket::new(stream), IMPRESARIO_DEVICE_NAME);
        let mut verify = pairing.clone();
        control
            .attempt_pair_verify()
            .await
            .map_err(|error| Error::Device(format!("pair-verify with {host} failed: {error}")))?;
        control
            .validate_pairing(&mut verify)
            .await
            .map_err(|error| {
                Error::Device(format!(
                    "{host} rejected the saved pairing record: {error}; re-pair with \
                 `impresario apple pair --host {host}`"
                ))
            })?;

        let listener_port = control
            .create_tcp_listener()
            .await
            .map_err(|error| Error::Device(format!("create tunnel listener: {error}")))?;
        let stream = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::TcpStream::connect((host, listener_port)),
        )
        .await
        .map_err(|_| {
            Error::Device(format!(
                "timed out connecting to tunnel {host}:{listener_port}"
            ))
        })?
        .map_err(|error| Error::io(format!("connect to tunnel {host}:{listener_port}"), error))?;
        let tunnel = connect_tls_psk_tunnel_native(stream, control.encryption_key())
            .await
            .map_err(|error| Error::Device(format!("TLS-PSK tunnel failed: {error}")))?;
        let info = tunnel.info.clone();
        let raw = tunnel.into_inner();
        let stream: Box<dyn idevice::ReadWrite> = Box::new(raw);
        let mut adapter = Adapter::new(
            Box::new(stream),
            info.client_address.parse().map_err(|error| {
                Error::Device(format!("invalid tunnel client address: {error}"))
            })?,
            info.server_address.parse().map_err(|error| {
                Error::Device(format!("invalid tunnel server address: {error}"))
            })?,
        );
        adapter.set_mss(usize::from(info.mtu.saturating_sub(60)));
        let mut handle = adapter.to_async_handle();
        let rsd_stream = handle
            .connect_to_service_port(info.server_rsd_port)
            .await
            .map_err(|error| Error::Device(format!("connect to RSD port: {error}")))?;
        let handshake = RsdHandshake::new(rsd_stream)
            .await
            .map_err(|error| Error::Device(format!("RSD handshake: {error}")))?;

        Ok(Self {
            _control: control,
            handle,
            handshake,
            host: host.to_string(),
        })
    }

    /// Restart the device through the diagnostics relay. This is the clean
    /// way to clear a wedged XCUITest/testmanagerd session; the tunnel drops
    /// when the device reboots.
    pub async fn restart(&mut self) -> Result<()> {
        use idevice::services::diagnostics_relay::DiagnosticsRelayClient;
        use idevice::RsdService as _;

        let mut relay = DiagnosticsRelayClient::connect_rsd(&mut self.handle, &mut self.handshake)
            .await
            .map_err(|error| Error::Device(format!("diagnostics relay: {error}")))?;
        relay
            .restart()
            .await
            .map_err(|error| Error::Device(format!("restart: {error}")))
    }

    /// A string property from the RSD handshake.
    #[must_use]
    pub fn property(&self, key: &str) -> Option<&str> {
        self.handshake
            .properties
            .get(key)
            .and_then(plist::Value::as_string)
    }

    /// A u64 property from the RSD handshake.
    #[must_use]
    pub fn property_u64(&self, key: &str) -> Option<u64> {
        self.handshake
            .properties
            .get(key)
            .and_then(plist::Value::as_unsigned_integer)
    }

    /// The device UDID as reported by RSD.
    #[must_use]
    pub fn udid(&self) -> Option<&str> {
        self.property("UniqueDeviceID")
    }

    /// The hardware model (`AppleTV5,3`).
    #[must_use]
    pub fn product_type(&self) -> Option<&str> {
        self.property("ProductType")
    }

    /// The OS build (`23J582`).
    #[must_use]
    pub fn build_version(&self) -> Option<&str> {
        self.property("BuildVersion")
    }

    /// The user-facing OS version (`26.1`).
    #[must_use]
    pub fn product_version(&self) -> Option<&str> {
        self.property("ProductVersion")
            .or_else(|| self.property("HumanReadableProductVersionString"))
    }

    /// The major OS version used by the DTX handshake.
    #[must_use]
    pub fn os_major_version(&self) -> u8 {
        self.product_version()
            .and_then(|version| version.split('.').next())
            .and_then(|major| major.parse().ok())
            .unwrap_or(18)
    }

    /// The device ECID (`UniqueChipID`).
    #[must_use]
    pub fn unique_chip_id(&self) -> Option<u64> {
        self.property_u64("UniqueChipID")
    }

    /// The device name used in logs.
    #[must_use]
    pub fn device_name(&self) -> &str {
        self.property("UserAssignedDeviceName")
            .or_else(|| self.property("DeviceName"))
            .unwrap_or(&self.host)
    }

    /// A one-line identity for diagnostics.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} ({}, {} {}) at {}",
            self.device_name(),
            self.product_type().unwrap_or("unknown"),
            self.product_version().unwrap_or("unknown"),
            self.build_version().unwrap_or("unknown"),
            self.host
        )
    }
}
