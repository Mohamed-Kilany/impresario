//! Transport behavior that does not need real devices (spec section 9):
//! address handling, connector fast paths and platform explanations.

use impresario_clients::transport::{
    transport_for, AppleTransport, ConnectorTransport, DeviceTransport, TransportError,
};
use impresario_core::config::{DeviceConfig, DeviceConnection, DeviceOptions};
use impresario_core::platforms::Platform;

fn device(
    name: &str,
    platform: Platform,
    target_endpoint: &str,
    connector_address: Option<&str>,
) -> DeviceConfig {
    let mut connection = DeviceConnection::default();
    match platform {
        Platform::Web => {
            connection.endpoint = Some(target_endpoint.to_string());
        }
        Platform::WebOs | Platform::Tizen => {
            connection.host = Some("192.168.1.50".to_string());
            connection.endpoint = Some(target_endpoint.to_string());
        }
        Platform::Android | Platform::AndroidTv => {
            connection.serial = Some(target_endpoint.to_string());
        }
        Platform::Ios | Platform::Tvos => {
            connection.udid = Some(target_endpoint.to_string());
        }
    }
    DeviceConfig {
        name: name.to_string(),
        platform,
        connection,
        auth: impresario_core::config::DeviceAuth::default(),
        labels: Vec::new(),
        options: DeviceOptions {
            connector_address: connector_address.map(str::to_string),
            ..DeviceOptions::default()
        },
    }
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
async fn apple_transport_explains_on_non_macos_hosts() {
    if cfg!(target_os = "macos") {
        return;
    }
    let mut transport = AppleTransport::new();
    let dev = device(
        "test_iphone",
        Platform::Ios,
        "00008110-001A2345021E001E",
        None,
    );
    let error = transport.prepare(&dev, None).await.expect_err("not macOS");
    match error {
        TransportError::DeviceUnavailable(message) => {
            assert!(message.contains("macOS"), "{message}");
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(
        transport.log_sources().first().map(|s| s.name.as_str()),
        Some("xcodebuild")
    );
}

#[tokio::test]
async fn adb_transport_reports_a_missing_adb_binary() {
    std::env::set_var("IMPRESARIO_ADB", "definitely-not-adb-impresario");
    let mut transport = transport_for(Platform::Android);
    let dev = device("flagship_android", Platform::Android, "emulator-5554", None);
    let error = transport.prepare(&dev, None).await.expect_err("no adb");
    match &error {
        TransportError::Transport(message) => assert!(message.contains("adb"), "{message}"),
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(error.to_wire().code.as_str(), "TRANSPORT");
    std::env::remove_var("IMPRESARIO_ADB");
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
        Some("xcodebuild")
    );
}
