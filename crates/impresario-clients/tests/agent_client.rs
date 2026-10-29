//! End-to-end tests of `AgentClient`/`AgentSession` against the in-process
//! `MockAgent` (spec 7.2 protocol rules).

use std::sync::Arc;
use std::time::{Duration, Instant};

use impresario_clients::agent_client::{
    AgentClient, Capability, ClientError, HandshakeParams, SessionOptions,
};
use impresario_clients::log::{LogLevel, LogRecord};
use impresario_clients::testing::{command_kind, MockAgent};
use impresario_core::command::{
    CheckKind, Command, KeyPress, RunAction, ValidateCheck, WireErrorCode,
};
use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;
use impresario_core::selectors::{parse as parse_selector, Selector};

fn click() -> Command {
    Command::RunAction(RunAction::click(selector("id:login_btn")))
}

fn selector(value: &str) -> Selector {
    parse_selector(Platform::Android, value, 0, None).expect("selector parses")
}

fn params() -> HandshakeParams {
    let mut params = HandshakeParams::new(
        "flagship_android",
        Platform::Android,
        "emulator-5554",
        "1.0.0-test",
    );
    params.app_id = Some("com.example.shop".to_string());
    params
}

async fn connected(mock: &MockAgent) -> (AgentClient, impresario_clients::HandshakeOutcome) {
    let client = AgentClient::connect(&mock.address())
        .await
        .expect("connect");
    let outcome = client.handshake(&params()).await.expect("handshake");
    (client, outcome)
}

#[tokio::test]
async fn handshake_then_ordered_commands() {
    let mock = MockAgent::builder().start().await.expect("mock starts");
    let (client, outcome) = connected(&mock).await;
    assert!(outcome.accepted);
    assert_eq!(outcome.session_id, "mock-session-1");
    assert_eq!(outcome.protocol_version, "1.0.0");
    assert!(outcome.has_capability(Capability::Screenshot));
    assert!(!outcome.has_capability(Capability::MultiSession));
    let device = outcome.device.as_ref().expect("device info");
    assert_eq!(device.model, "Mock Device");
    assert_eq!(device.screen_width_px, 1080);

    let session = client
        .open_session(&outcome.session_id, SessionOptions::default())
        .await
        .expect("session opens");

    let result = session.send(&click(), 2000, 500).await.expect("click");
    assert!(result.ok);
    assert!(result.error.is_none());

    let check = Command::ValidateCheck(ValidateCheck {
        selector: selector("id:welcome_banner"),
        kind: CheckKind::TextEquals,
        expected_text: "Welcome".to_string(),
        expected_count: 0,
    });
    let result = session.send(&check, 2000, 500).await.expect("check");
    assert!(result.ok);
    assert_eq!(result.observed_text, "Welcome");

    let key = Command::KeyPress(KeyPress::new(RemoteKey::Down, 2));
    assert!(session.send(&key, 1000, 500).await.expect("key").ok);

    session.shutdown(false).await.expect("shutdown");
    assert_eq!(mock.state().shutdowns(), 1);

    assert_eq!(mock.state().handshakes(), 1);
    assert_eq!(mock.state().sessions(), 1);
    assert_eq!(
        mock.state().command_kinds(),
        vec!["run_action", "validate_check", "key_press"]
    );
    let commands = mock.state().commands();
    assert_eq!(commands.first().map(|c| c.timeout_ms), Some(2000));
    assert_eq!(
        commands
            .first()
            .map(|c| c.command_id.as_str())
            .map(str::is_empty),
        Some(false)
    );
}

#[tokio::test]
async fn second_handshake_is_already_in_use() {
    let mock = MockAgent::builder()
        .single_session()
        .start()
        .await
        .expect("mock starts");
    let first = AgentClient::connect(&mock.address())
        .await
        .expect("connect");
    first.handshake(&params()).await.expect("first handshake");

    let second = AgentClient::connect(&mock.address())
        .await
        .expect("connect");
    let error = second.handshake(&params()).await.expect_err("rejected");
    assert!(
        matches!(&error, ClientError::HandshakeRejected(wire) if wire.code == WireErrorCode::AlreadyInUse),
        "unexpected error: {error}"
    );
    assert_eq!(error.code(), WireErrorCode::AlreadyInUse);
}

#[tokio::test]
async fn cancel_of_unknown_id_is_a_protocol_result() {
    let mock = MockAgent::builder().start().await.expect("mock starts");
    let (client, outcome) = connected(&mock).await;
    let session = client
        .open_session(&outcome.session_id, SessionOptions::default())
        .await
        .expect("session opens");

    let result = session
        .cancel("no-such-command")
        .await
        .expect("cancel result");
    assert!(!result.ok);
    let error = result.error.expect("error info");
    assert_eq!(error.code, WireErrorCode::Protocol);
    assert!(error.message.contains("no-such-command"));
}

#[tokio::test]
async fn scripted_failure_reaches_the_caller() {
    let mock = MockAgent::builder()
        .fail_command(1, WireErrorCode::ElementNotFound, "no such element")
        .start()
        .await
        .expect("mock starts");
    let (client, outcome) = connected(&mock).await;
    let session = client
        .open_session(&outcome.session_id, SessionOptions::default())
        .await
        .expect("session opens");

    let result = session.send(&click(), 2000, 500).await.expect("result");
    assert!(!result.ok);
    assert_eq!(
        result.error.expect("error info").code,
        WireErrorCode::ElementNotFound
    );

    let result = session.send(&click(), 2000, 500).await.expect("second");
    assert!(result.ok, "only the first command was scripted to fail");
}

#[tokio::test]
async fn master_deadline_cancels_a_hung_command() {
    let mock = MockAgent::builder()
        .hang_kind("run_action")
        .start()
        .await
        .expect("mock starts");
    let (client, outcome) = connected(&mock).await;
    let options = SessionOptions {
        cancel_wait: Duration::from_millis(300),
        ..SessionOptions::default()
    };
    let session = client
        .open_session(&outcome.session_id, options)
        .await
        .expect("session opens");

    let started = Instant::now();
    let error = session
        .send(&click(), 100, 100)
        .await
        .expect_err("must time out");
    assert!(matches!(error, ClientError::Timeout { .. }), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the deadline path must not drag on"
    );
    assert!(
        session.is_closed(),
        "the session is torn down after overrun"
    );

    let commands = mock.wait_for_commands(2, Duration::from_secs(2)).await;
    assert_eq!(commands.len(), 2, "click then Cancel");
    assert_eq!(command_kind(&commands[0]), "run_action");
    assert_eq!(command_kind(&commands[1]), "cancel");

    let error = session
        .send(&click(), 100, 100)
        .await
        .expect_err("refused after close");
    assert_eq!(error.code(), WireErrorCode::Timeout);
}

#[tokio::test]
async fn missed_heartbeats_declare_a_transport_failure() {
    let mock = MockAgent::builder()
        .heartbeat_interval(Duration::from_millis(25))
        .stop_heartbeats_after(2)
        .start()
        .await
        .expect("mock starts");
    let (client, outcome) = connected(&mock).await;
    let options = SessionOptions {
        heartbeat_timeout: Duration::from_millis(300),
        watchdog_poll: Duration::from_millis(25),
        cancel_wait: Duration::from_millis(50),
    };
    let session = client
        .open_session(&outcome.session_id, options)
        .await
        .expect("session opens");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !session.is_closed() {
        assert!(Instant::now() < deadline, "session never failed");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let reason = session.close_reason().expect("close reason");
    assert_eq!(reason.code(), WireErrorCode::Transport);

    let error = session
        .send(&click(), 1000, 100)
        .await
        .expect_err("refused");
    assert_eq!(error.code(), WireErrorCode::Transport);
}

#[tokio::test]
async fn token_is_enforced_on_every_rpc() {
    let mock = MockAgent::builder()
        .token("s3cret")
        .start()
        .await
        .expect("mock starts");

    let client = AgentClient::connect(&mock.address())
        .await
        .expect("connect");
    let error = client.handshake(&params()).await.expect_err("rejected");
    assert_eq!(error.code(), WireErrorCode::Unauthenticated);

    let client = AgentClient::connect(&mock.address())
        .await
        .expect("connect")
        .with_token(Some("s3cret".to_string()));
    let mut with_token = params();
    with_token.auth_token = Some("s3cret".to_string());
    let outcome = client.handshake(&with_token).await.expect("accepted");
    let session = client
        .open_session(&outcome.session_id, SessionOptions::default())
        .await
        .expect("session opens");
    assert!(session.send(&click(), 1000, 500).await.expect("click").ok);
}

#[tokio::test]
async fn capture_screenshot_overlaps_a_hung_command() {
    let mock = MockAgent::builder()
        .hang_kind("run_action")
        .start()
        .await
        .expect("mock starts");
    let (client, outcome) = connected(&mock).await;
    let session = Arc::new(
        client
            .open_session(
                &outcome.session_id,
                SessionOptions {
                    cancel_wait: Duration::from_millis(300),
                    ..SessionOptions::default()
                },
            )
            .await
            .expect("session opens"),
    );

    let hung = {
        let session = session.clone();
        tokio::spawn(async move { session.send(&click(), 30_000, 0).await })
    };
    let commands = mock.wait_for_commands(1, Duration::from_secs(2)).await;
    assert_eq!(commands.len(), 1);
    let command_id = commands.first().map(|c| c.command_id.clone()).expect("id");

    let capture = session.capture_screenshot().await.expect("capture");
    assert_eq!(capture.png, b"mock-png");
    assert_eq!(capture.width, 1080);
    assert_eq!(capture.height, 1920);
    assert!(capture.error.is_none());
    assert_eq!(mock.state().captures(), 1);

    let cancel_result = session.cancel(&command_id).await.expect("cancel");
    assert!(cancel_result.ok);
    let outcome = hung.await.expect("join").expect("the hung send resolves");
    assert!(!outcome.ok);
    assert_eq!(
        outcome.error.expect("error info").code,
        WireErrorCode::Cancelled
    );
}

#[tokio::test]
async fn dropped_stream_fails_pending_and_future_commands() {
    let mock = MockAgent::builder()
        .drop_stream_after_results(1)
        .start()
        .await
        .expect("mock starts");
    let (client, outcome) = connected(&mock).await;
    let session = client
        .open_session(&outcome.session_id, SessionOptions::default())
        .await
        .expect("session opens");

    let first = session
        .send(&click(), 2000, 500)
        .await
        .expect("first result");
    assert!(first.ok);

    tokio::time::sleep(Duration::from_millis(50)).await;
    let error = session
        .send(&click(), 2000, 500)
        .await
        .expect_err("dropped");
    assert_eq!(error.code(), WireErrorCode::Transport, "{error}");
}

#[tokio::test]
async fn stream_logs_yields_records() {
    let logs = vec![
        LogRecord::new("agent", LogLevel::Info, "hello"),
        LogRecord::new("agent", LogLevel::Warn, "careful"),
    ];
    let mock = MockAgent::builder()
        .logs(logs)
        .start()
        .await
        .expect("mock starts");
    let (client, outcome) = connected(&mock).await;
    let session = client
        .open_session(&outcome.session_id, SessionOptions::default())
        .await
        .expect("session opens");

    let mut stream = session.stream_logs(LogLevel::Debug).await.expect("logs");
    let first = stream.next().await.expect("entry").expect("ok");
    assert_eq!(first.message, "hello");
    assert_eq!(first.level, LogLevel::Info);
    assert_eq!(first.source, "agent");
    let second = stream.next().await.expect("entry").expect("ok");
    assert_eq!(second.level, LogLevel::Warn);
    assert_eq!(mock.state().log_streams(), 1);
}

#[tokio::test]
async fn connect_retry_gives_up_with_a_transport_error() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);

    let error =
        AgentClient::connect_retry(&format!("127.0.0.1:{port}"), Duration::from_millis(300))
            .await
            .expect_err("nothing listens there");
    assert_eq!(error.code(), WireErrorCode::Transport);
}
