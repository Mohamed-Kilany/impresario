//! CLI-level `run` tests (spec 15): a complete workspace on disk, the real
//! `run_with` entry, reporters written into the run directory, and — through
//! the spawned binary — secret redaction of every artifact.

mod common;

use std::path::Path;

use assert_cmd::Command;
use base64::Engine as _;
use common::{
    argv, read_report_json, run_dir, write_workspace, TestAgent, PLAY_BASIC, PLAY_SECRET, PLAY_SHOT,
};
use impresario_clients::log::{LogLevel, LogRecord};
use impresario_clients::testing::MockAgentBuilder;
use impresario_core::command::WireErrorCode;
use impresario_master::cli::run_with;

const EXIT_OK: i32 = 0;
const EXIT_TEST_FAILED: i32 = 1;
const EXIT_USAGE: i32 = 2;

/// The spawned binary's argv (assert_cmd already knows the program).
fn binary_args(workspace: &Path, rest: &[&str]) -> Vec<String> {
    argv(workspace, rest).split_off(1)
}

#[test]
fn run_defaults_write_console_junit_and_json() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("basic.play", PLAY_BASIC)],
    );

    let output = Command::cargo_bin("impresario")
        .expect("binary")
        .args(binary_args(temp.path(), &["run"]))
        .output()
        .expect("child runs");
    assert_eq!(output.status.code(), Some(EXIT_OK));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("▶ web1 · web"), "console tree: {stdout}");
    assert!(stdout.contains("✔ Act: Pass"), "console tree: {stdout}");

    let dir = run_dir(temp.path());
    assert!(dir.join("junit.xml").is_file(), "default junit reporter");
    assert!(dir.join("report.json").is_file(), "default json reporter");
    assert!(!dir.join("report.html").exists(), "html is opt-in");

    let report = read_report_json(temp.path());
    assert_eq!(report["status"], "passed");
    assert_eq!(report["exit_code"], 0);
    assert_eq!(report["devices"][0]["name"], "web1");
    assert_eq!(report["devices"][0]["platform"], "web");
    let junit = std::fs::read_to_string(dir.join("junit.xml")).expect("junit.xml");
    assert!(junit.contains("name=\"Pass\""), "{junit}");
}

#[test]
fn run_writes_html_with_embedded_screenshot() {
    // The screenshot step passes and writes `shot1.png`; the check then fails
    // so the report references the failure capture (`failure-1.png`), which
    // the html reporter must embed as base64.
    let agent = TestAgent::start(
        MockAgentBuilder::new()
            .screenshot(b"cli-png".to_vec())
            .fail_command(2, WireErrorCode::CheckFailed, "nope"),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("shot.play", PLAY_SHOT)],
    );

    let code = run_with(&argv(temp.path(), &["run", "--reporter", "html"]));
    assert_eq!(code, EXIT_TEST_FAILED);
    let dir = run_dir(temp.path());
    assert!(
        !dir.join("report.json").exists(),
        "--reporter html writes only report.html"
    );
    let png = dir.join("screenshots/web1/shot1.png");
    assert_eq!(std::fs::read(png).expect("shot1.png"), b"cli-png");
    assert_eq!(
        std::fs::read(dir.join("screenshots/web1/failure-1.png")).expect("failure-1.png"),
        b"cli-png"
    );

    let html = std::fs::read_to_string(dir.join("report.html")).expect("report.html");
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"cli-png");
    assert!(
        html.contains(&format!("data:image/png;base64,{encoded}")),
        "the failure screenshot is embedded as base64"
    );
    assert!(html.contains("run-failed"), "run badge: status failed");
}

#[test]
fn secrets_never_reach_any_artifact() {
    const SECRET: &str = "cli-secret-hunter2";
    let agent = TestAgent::start(
        MockAgentBuilder::new()
            .delay_kind("validate_check", 200)
            .fail_command(1, WireErrorCode::CheckFailed, format!("saw {SECRET}"))
            .logs(vec![LogRecord::new(
                "mock",
                LogLevel::Error,
                format!("agent log leaks {SECRET}"),
            )]),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("secret.play", PLAY_SECRET)],
    );

    let output = Command::cargo_bin("impresario")
        .expect("binary")
        .env("IMPRESARIO_TEST_SECRET", SECRET)
        .args(binary_args(
            temp.path(),
            &["run", "--reporter", "console,junit,json,html"],
        ))
        .output()
        .expect("child runs");
    assert_eq!(output.status.code(), Some(EXIT_TEST_FAILED));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("***"), "console shows the marker: {stdout}");
    assert!(
        !stdout.contains(SECRET),
        "console leaks the secret: {stdout}"
    );

    let dir = run_dir(temp.path());
    for name in ["junit.xml", "report.json", "report.html"] {
        let text = std::fs::read_to_string(dir.join(name)).expect(name);
        assert!(!text.contains(SECRET), "{name} leaks the secret");
        assert!(text.contains("***"), "{name} should carry the marker");
    }
}

#[test]
fn run_in_missing_workspace_exits_2() {
    let temp = tempfile::tempdir().expect("tempdir");
    let output = Command::cargo_bin("impresario")
        .expect("binary")
        .args(binary_args(temp.path(), &["run"]))
        .output()
        .expect("child runs");
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
}
