//! Transport behavior that does not need real devices (spec section 9):
//! address handling, connector fast paths and platform explanations.

use impresario_clients::transport::{
    transport_for, AdbTransport, AppleTransport, ConnectorTransport, DeviceTransport,
    TransportError,
};
use impresario_core::config::{DeviceConfig, DeviceOptions};
use impresario_core::home::Home;
use impresario_core::platforms::Platform;

fn device(
    name: &str,
    platform: Platform,
    target_endpoint: &str,
    connector_address: Option<&str>,
) -> DeviceConfig {
    let ip = match platform {
        Platform::Web => "127.0.0.1",
        Platform::WebOs | Platform::Tizen => "192.168.1.50",
        Platform::Android | Platform::AndroidTv => "192.168.1.60",
        Platform::Ios | Platform::Tvos => target_endpoint,
    };
    let mut dev = DeviceConfig::new(name, platform, ip);
    dev.options = DeviceOptions {
        connector_address: connector_address.map(str::to_string),
        ..DeviceOptions::default()
    };
    dev
}

#[tokio::test]
async fn connector_uses_a_non_loopback_address_directly() {
    let mut transport = ConnectorTransport::new();
    let dev = device(
        "living_room_tv",
        Platform::WebOs,
        "ws://192.168.1.50:9998",
        Some("192.168.1.50:50051"),
    );
    let endpoint = transport.prepare(&dev, None).await.expect("prepare");
    assert_eq!(endpoint.address(), "192.168.1.50:50051");
    assert!(!endpoint.is_loopback());
    assert_eq!(transport.log_sources().len(), 1);
    transport.teardown().await.expect("teardown");
}

#[tokio::test]
async fn connector_connects_to_a_listening_loopback_agent() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr").to_string();

    let mut transport = ConnectorTransport::new();
    let dev = device("web_site", Platform::Web, "launch", Some(&address));
    let endpoint = transport.prepare(&dev, None).await.expect("prepare");
    assert_eq!(endpoint.address(), address);
    assert!(endpoint.is_loopback());
}

#[tokio::test]
async fn connector_handles_a_loopback_address_with_nothing_listening() {
    // Either a connector binary exists and gets spawned, or prepare explains
    // that nothing was listening and no connector was found.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);

    let mut transport = ConnectorTransport::new();
    let mut dev = device("web_site", Platform::Web, "launch", None);
    let address = format!("127.0.0.1:{port}");
    dev.options.connector_address = Some(address.clone());
    match transport.prepare(&dev, None).await {
        Ok(endpoint) => {
            assert_eq!(endpoint.address(), address);
            transport.teardown().await.expect("teardown");
        }
        Err(TransportError::Transport(message)) => {
            assert!(message.contains("impresario-connector") || message.contains("did not start"));
        }
        Err(other) => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn apple_transport_requires_a_lan_host() {
    let mut transport = AppleTransport::new();
    let physical = device("test_apple_tv", Platform::Tvos, "", None);
    let error = transport
        .prepare(&physical, None)
        .await
        .expect_err("missing LAN host");
    match error {
        TransportError::DeviceUnavailable(message) => {
            assert!(message.contains("no IP"), "{message}");
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(
        transport.log_sources().first().map(|s| s.name.as_str()),
        Some("xctest")
    );

    let physical = device(
        "test_apple_tv",
        Platform::Tvos,
        "https://example.test",
        None,
    );
    let error = transport
        .prepare(&physical, None)
        .await
        .expect_err("hostname is not an IP");
    match error {
        TransportError::DeviceUnavailable(message) => {
            assert!(message.contains("LAN host"), "{message}");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn adb_transport_reports_a_device_without_an_ip() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut transport = AdbTransport::with_home(Home::at(home.path()));
    let dev = DeviceConfig::new("flagship_android", Platform::Android, "");
    let error = transport.prepare(&dev, None).await.expect_err("no device");
    match &error {
        TransportError::DeviceUnavailable(message) => {
            assert!(message.contains("no IP"), "{message}");
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(error.to_wire().code.as_str(), "DEVICE_UNAVAILABLE");
}

#[tokio::test]
async fn transport_for_matches_the_platform_family() {
    assert_eq!(
        transport_for(Platform::Tizen)
            .log_sources()
            .first()
            .map(|s| s.name.as_str()),
        Some("connector")
    );
    assert_eq!(transport_for(Platform::AndroidTv).log_sources().len(), 3);
    assert_eq!(
        transport_for(Platform::Tvos)
            .log_sources()
            .first()
            .map(|s| s.name.as_str()),
        Some("xctest")
    );
}
