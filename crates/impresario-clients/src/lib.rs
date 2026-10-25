#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::todo,
        clippy::unreachable
    )
)]

//! impresario-clients: the master-side gRPC client for the `Agent` service
//! (spec 7.2/7.3), one device transport per platform family (section 9),
//! core<->proto conversions (D-16) and the in-process `MockAgent` used by
//! orchestrator integration tests.

pub mod adb;
pub mod agent_client;
pub mod convert;
pub mod log;
pub mod testing;
pub mod tizen;
pub mod tizen_sign;
pub mod transport;
pub mod webos;
pub mod webos_package;

pub use adb::{
    discover_mdns, discover_usb, pair_wireless, AdbClient, AdbError, AdbTarget, AndroidProbe,
    MdnsAndroidDevice, UsbAndroidDevice,
};
pub use agent_client::{
    AgentClient, AgentSession, Capability, CaptureOutcome, ClientError, DeviceInfo,
    HandshakeOutcome, HandshakeParams, SessionOptions, PROTOCOL_VERSION,
};
pub use log::{LogBuffer, LogLevel, LogRecord, LogStream};
pub use transport::{
    connector_binary, transport_for, AdbTransport, AppleTransport, ConnectorTransport,
    DeviceTransport, Endpoint, LogSource, LogSourceKind, NativeProbe, TransportError,
};
