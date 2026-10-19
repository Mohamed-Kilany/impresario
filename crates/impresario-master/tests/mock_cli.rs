//! The README quick start, end to end and hardware-free, exactly as written:
//! `impresario workspace new` + `validate`, `impresario mock-agent` (decision I-77)
//! serving the scripted MockAgent, the mock device appended to
//! `impresario.devices.config`, and a real `impresario run` child of the
//! workspace-new example play exiting 0 with a passing `report.json`.

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::{read_report_json, run_dir};

/// Kills the mock-agent child when the test ends, even on assertion failure.
struct MockGuard(Child);

impl Drop for MockGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn impresario() -> Command {
    Command::new(env!("CARGO_BIN_EXE_impresario"))
}

/// Spawn a mock-agent and return its printed address.
fn start_mock_agent(port: &str) -> (MockGuard, String) {
    let child = impresario()
        .args(["mock-agent", "--port", port])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mock-agent");
    let mut mock = MockGuard(child);
    let stdout = mock.0.stdout.take().expect("stdout pipe");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let address = rx
        .recv_timeout(Duration::from_secs(30))
        .ok()
        .and_then(|line| {
            line.strip_prefix("impresario: mock device listening on ")
                .map(str::trim)
                .map(str::to_string)
        })
        .expect("mock-agent printed its listening address");
    (mock, address)
}

/// `impresario <args>` in `dir`; returns (exit code, stdout, stderr).
fn run_in(
    dir: &std::path::Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> (Option<i32>, String, String) {
    let mut command = impresario();
    command.current_dir(dir).args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("child runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn readme_mock_quick_start_runs_green() {
    let temp = tempfile::tempdir().expect("tempdir");

    // Terminal A: `impresario mock-agent --port 50071` (port 0 here so the
    // test never races another process for the fixed port).
    let (_mock, address) = start_mock_agent("0");

    // README step 2: create and validate the workspace.
    let (code, stdout, stderr) = run_in(temp.path(), &["workspace", "new", "--example"], &[]);
    assert_eq!(code, Some(0), "workspace new failed\n{stdout}\n{stderr}");
    let (code, stdout, stderr) = run_in(temp.path(), &["validate"], &[]);
    assert_eq!(code, Some(0), "validate failed\n{stdout}\n{stderr}");

    // README step 3: append the mock device (the play's act targets webos;
    // webos falls back to the web selectors).
    let devices_path = temp.path().join("impresario.devices.config");
    let mut devices = std::fs::read_to_string(&devices_path).expect("read devices");
    devices.push_str(&format!(
        "  - name: mock_device\n    platform: webos\n    target_endpoint: ws://127.0.0.1:9999\n    connector: {address}\n"
    ));
    std::fs::write(&devices_path, devices).expect("write devices");

    // Terminal B: `TEST_PASSWORD=… impresario run --play plays/login.play --device mock_device`.
    let (code, stdout, stderr) = run_in(
        temp.path(),
        &[
            "run",
            "--play",
            "plays/login.play",
            "--device",
            "mock_device",
        ],
        &[("TEST_PASSWORD", "impresario")],
    );
    assert_eq!(code, Some(0), "run failed\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("Act: Successful Login Flow"),
        "console tree: {stdout}"
    );

    let report = read_report_json(temp.path());
    assert_eq!(report["status"], "passed", "report: {report}");
    assert_eq!(report["exit_code"], 0);
    assert!(
        run_dir(temp.path()).join("report.json").is_file(),
        "json report written"
    );
}

#[test]
fn mock_agent_fails_loudly_when_the_port_is_taken() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (_mock, address) = start_mock_agent("0");
    let port = address.rsplit(':').next().expect("host:port").to_string();

    // Second bind on the live port: exit 3 (infrastructure), message on stderr.
    let (code, _stdout, stderr) = run_in(temp.path(), &["mock-agent", "--port", &port], &[]);
    assert_eq!(code, Some(3), "stderr: {stderr}");
    assert!(stderr.contains("cannot listen on"), "stderr: {stderr}");
}
