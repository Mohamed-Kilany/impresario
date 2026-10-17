//! `devices check` integration tests (spec 8.1/15): transport prepare,
//! handshake, `DeviceInfo` line, teardown — one session-less probe per
//! device, reporting failures without a run report.

mod common;

use std::path::Path;

use assert_cmd::Command;
use common::{argv, write_workspace, TestAgent};
use impresario_clients::testing::MockAgentBuilder;
use impresario_core::command::WireErrorCode;
use impresario_master::cli::run_with;

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 2;
const EXIT_INFRA: i32 = 3;

fn binary_args(workspace: &Path, rest: &[&str]) -> Vec<String> {
    argv(workspace, rest).split_off(1)
}

#[test]
fn devices_check_reports_device_info() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[],
    );

    let output = Command::cargo_bin("impresario")
        .expect("binary")
        .args(binary_args(temp.path(), &["devices", "check"]))
        .output()
        .expect("child runs");
    assert_eq!(output.status.code(), Some(EXIT_OK));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("web1 (web): Mock Device · MockOS 1.0 · 1080x1920 @1x · MockAgent"),
        "device line: {stdout}"
    );
    assert!(
        stdout.contains("devices check: 1 device(s) ok"),
        "summary: {stdout}"
    );

    // A check handshakes and shuts the session down without a Session stream.
    assert_eq!(agent.state.handshakes(), 1);
    assert_eq!(agent.state.sessions(), 0);
    assert_eq!(agent.state.shutdowns(), 1);
}

#[test]
fn devices_check_selects_by_name() {
    let first = TestAgent::start(MockAgentBuilder::new());
    let second = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[
            ("first".to_string(), first.address.clone()),
            ("second".to_string(), second.address.clone()),
        ],
        &[],
    );

    let code = run_with(&argv(temp.path(), &["devices", "check", "second"]));
    assert_eq!(code, EXIT_OK);
    assert_eq!(first.state.handshakes(), 0, "first is not selected");
    assert_eq!(second.state.handshakes(), 1);
}

#[test]
fn devices_check_unknown_name_exits_2() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[],
    );

    let code = run_with(&argv(temp.path(), &["devices", "check", "nope"]));
    assert_eq!(code, EXIT_USAGE);
    assert_eq!(agent.state.handshakes(), 0);
}

#[test]
fn devices_check_rejected_handshake_exits_3() {
    let agent = TestAgent::start(
        MockAgentBuilder::new().reject_handshake(WireErrorCode::Unauthenticated, "bad token"),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[],
    );

    let code = run_with(&argv(temp.path(), &["devices", "check"]));
    assert_eq!(code, EXIT_INFRA);
    assert_eq!(agent.state.handshakes(), 1);
    assert_eq!(agent.state.sessions(), 0);
}

#[test]
fn devices_check_without_configs_exits_2() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output = Command::cargo_bin("impresario")
        .expect("binary")
        .args(binary_args(temp.path(), &["devices", "check"]))
        .output()
        .expect("child runs");
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
}
