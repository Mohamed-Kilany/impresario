//! INS-R12 old/new handshake matrix: the master must keep talking to every
//! executor it talked to before the `Inspector` service and `LOCATOR`
//! strategy were added.
//!
//! - an existing `1.0.0` executor without `INSPECTOR` runs unchanged;
//! - a future `1.x` executor with unknown capabilities is tolerated;
//! - a `0.x` executor is rejected at the handshake as a device error.

mod common;

use common::{build_test_plan, device, exec_options, test_config, TestAgent, PLAY_TWO_ACTS};
use impresario_clients::testing::{mock_capabilities, MockAgentBuilder, MockScript};
use impresario_master::exec::execute;
use impresario_master::report::{DeviceReport, RunStatus, Status};
use impresario_proto::v1 as pb;

fn act_status(device: &DeviceReport, act: &str) -> Status {
    device
        .plays
        .iter()
        .flat_map(|play| &play.acts)
        .find(|entry| entry.name == act)
        .unwrap_or_else(|| panic!("act {act} missing"))
        .status
}

#[tokio::test]
async fn one_zero_executor_without_inspector_still_runs() {
    let script = MockScript {
        protocol_version: "1.0.0".to_string(),
        // Exactly the capabilities shipped before the inspector work.
        capabilities: mock_capabilities(),
        ..MockScript::default()
    };
    assert!(
        !script
            .capabilities
            .contains(&(pb::Capability::Inspector as i32)),
        "the compatibility fixture must model an executor without INSPECTOR"
    );
    let agent = TestAgent::start(MockAgentBuilder::new().script(script));

    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_TWO_ACTS);
    let options = exec_options(&temp, config, "run-handshake-1-0");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.report.status, RunStatus::Passed);
    let device = outcome
        .report
        .devices
        .iter()
        .find(|entry| entry.name == "web1")
        .expect("device report");
    assert_eq!(act_status(device, "First"), Status::Passed);
    assert_eq!(act_status(device, "Second"), Status::Passed);
    assert_eq!(agent.state.handshakes(), 1);
}

#[tokio::test]
async fn future_minor_protocol_and_unknown_capabilities_are_tolerated() {
    let mut capabilities = mock_capabilities();
    // A future executor may advertise capabilities this master predates.
    capabilities.push(9_999);
    let script = MockScript {
        protocol_version: "1.9.7".to_string(),
        capabilities,
        ..MockScript::default()
    };
    let agent = TestAgent::start(MockAgentBuilder::new().script(script));

    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_TWO_ACTS);
    let options = exec_options(&temp, config, "run-handshake-1-9");
    let outcome = execute(&plan, &options).await;

    assert_eq!(
        outcome.exit_code, 0,
        "a future minor must stay wire-compatible: {:?}",
        outcome.report.status
    );
    assert_eq!(outcome.report.status, RunStatus::Passed);
    assert_eq!(agent.state.handshakes(), 1);
}

#[tokio::test]
async fn major_mismatch_is_a_protocol_device_error() {
    let script = MockScript {
        protocol_version: "0.9.0".to_string(),
        ..MockScript::default()
    };
    let agent = TestAgent::start(MockAgentBuilder::new().script(script));

    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_TWO_ACTS);
    let options = exec_options(&temp, config, "run-handshake-0-9");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 3);
    assert_eq!(outcome.report.status, RunStatus::Error);
    let device = outcome
        .report
        .devices
        .iter()
        .find(|entry| entry.name == "web1")
        .expect("device report");
    let error = device.error.as_ref().expect("device error");
    assert_eq!(error.code, "PROTOCOL");
    assert_eq!(
        error.details.get("phase").map(String::as_str),
        Some("handshake")
    );
    assert!(
        error.message.contains("protocol major mismatch"),
        "unexpected message: {}",
        error.message
    );
    assert_eq!(agent.state.handshakes(), 1);
    assert_eq!(agent.state.sessions(), 0, "no session stream is opened");
}
