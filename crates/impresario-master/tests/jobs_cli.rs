//! Job, lease and daemon integration tests (TASK-2 3.5).
//!
//! The daemon starts from `current_exe()`, so every test here spawns the real
//! `impresario` binary through `Command::cargo_bin` instead of calling
//! `run_with` in-process. Each test owns a disposable `IMPRESARIO_HOME`, which
//! also isolates the local daemon socket and the job state.

mod common;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use common::{write_workspace, TestAgent, PLAY_BASIC};
use impresario_clients::testing::MockAgentBuilder;
use impresario_core::home::Home;
use impresario_master::jobs::{JobRecord, JobStatus, JobStore};
use serde_json::{json, Value};

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 2;
const EXIT_INFRA: i32 = 3;
const EXIT_INTERRUPTED: i32 = 130;

#[derive(Debug)]
struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn impresario(home: &Path, workspace: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin("impresario"));
    command
        .env("IMPRESARIO_HOME", home)
        .arg("--workspace")
        .arg(workspace)
        .arg("--no-tui")
        .arg("--plain")
        .arg("--no-color");
    command
}

fn run(home: &Path, workspace: &Path, args: &[&str]) -> Output {
    let output = impresario(home, workspace)
        .args(args)
        .output()
        .expect("impresario runs");
    Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn submitted_id(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("job ")
                .and_then(|rest| rest.strip_suffix(" submitted"))
        })
        .unwrap_or_else(|| panic!("no job id in: {stdout}"))
        .to_string()
}

fn wait_until(mut predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    predicate()
}

fn store(home: &Path) -> JobStore {
    JobStore::new(&Home::at(home))
}

#[test]
fn detached_job_runs_wait_report_logs_and_jobs() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(
        workspace.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("basic.play", PLAY_BASIC)],
    );

    let detached = run(home.path(), workspace.path(), &["run", "--detach"]);
    assert_eq!(detached.code, EXIT_OK, "stderr={}", detached.stderr);
    let id = submitted_id(&detached.stdout);
    assert!(
        detached
            .stdout
            .contains(&format!("attach: impresario attach {id}")),
        "{}",
        detached.stdout
    );

    let listed = run(home.path(), workspace.path(), &["jobs", "--all", "--json"]);
    assert_eq!(listed.code, EXIT_OK, "stderr={}", listed.stderr);
    let records: Vec<Value> = serde_json::from_str(&listed.stdout).expect("jobs json");
    assert!(
        records.iter().any(|record| record["id"] == id.as_str()),
        "{listed:?}"
    );

    let waited = run(home.path(), workspace.path(), &["wait", &id]);
    assert_eq!(waited.code, EXIT_OK, "stderr={}", waited.stderr);
    assert!(waited.stderr.contains("passed"), "{}", waited.stderr);

    let report = run(home.path(), workspace.path(), &["report", &id, "--json"]);
    assert_eq!(report.code, EXIT_OK, "stderr={}", report.stderr);
    let report: Value = serde_json::from_str(&report.stdout).expect("report json");
    assert_eq!(report["run_id"], id.as_str());
    assert_eq!(report["status"], "passed");
    assert_eq!(report["exit_code"], 0);

    let console = run(home.path(), workspace.path(), &["report", &id]);
    assert_eq!(console.code, EXIT_OK, "stderr={}", console.stderr);
    assert!(console.stdout.contains("Act: Pass"), "{}", console.stdout);

    let logs = run(home.path(), workspace.path(), &["logs", &id]);
    assert_eq!(logs.code, EXIT_OK, "stderr={}", logs.stderr);
    assert!(
        logs.stdout.contains(&format!("{id}: run {id} started")),
        "{}",
        logs.stdout
    );

    let events = run(home.path(), workspace.path(), &["logs", &id, "--events"]);
    assert_eq!(events.code, EXIT_OK, "stderr={}", events.stderr);
    assert!(
        events.stdout.contains("\"event\":\"run_started\""),
        "{}",
        events.stdout
    );
    assert!(
        events.stdout.contains("\"event\":\"run_finished\""),
        "{}",
        events.stdout
    );

    let active = run(home.path(), workspace.path(), &["jobs"]);
    assert_eq!(active.code, EXIT_OK, "stderr={}", active.stderr);
    assert_eq!(active.stdout.trim(), "no active jobs");

    let attach = run(home.path(), workspace.path(), &["attach", &id]);
    assert_eq!(attach.code, EXIT_OK, "stderr={}", attach.stderr);
    assert!(attach.stdout.contains("Act: Pass"), "{}", attach.stdout);
    assert!(attach.stderr.contains("passed"), "{}", attach.stderr);

    let stop = run(home.path(), workspace.path(), &["stop", &id]);
    assert_eq!(stop.code, EXIT_OK, "stderr={}", stop.stderr);
    assert!(stop.stdout.contains("already finished"), "{}", stop.stdout);

    let rm = run(home.path(), workspace.path(), &["rm", &id]);
    assert_eq!(rm.code, EXIT_OK, "stderr={}", rm.stderr);
    assert!(!store(home.path()).dir(&id).exists());

    let missing = run(home.path(), workspace.path(), &["report", &id]);
    assert_eq!(missing.code, EXIT_USAGE);
    assert!(missing.stderr.contains("unknown job"), "{}", missing.stderr);
}

#[test]
fn on_busy_fail_and_queue_use_the_device_lease() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(
        workspace.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("basic.play", PLAY_BASIC)],
    );
    let store = store(home.path());

    // A live owner holds the device lease. Using this test process as the
    // owner keeps the test deterministic: the lease stays valid until the
    // test removes it, without a second run and its timing.
    std::fs::create_dir_all(store.lease_dir()).expect("lease dir");
    let holder = store.lease_dir().join("web1.json");
    std::fs::write(
        &holder,
        json!({
            "job": "holder",
            "pid": std::process::id(),
            "devices": ["web1"],
            "started_ms": 1
        })
        .to_string(),
    )
    .expect("holder lease");

    // Attached: fail fast, before touching the device.
    let busy = run(
        home.path(),
        workspace.path(),
        &["run", "--on-busy", "fail", "--play", "plays/basic.play"],
    );
    assert_eq!(
        busy.code, EXIT_INFRA,
        "stdout={} stderr={}",
        busy.stdout, busy.stderr
    );
    assert!(busy.stderr.contains("device lease busy"), "{}", busy.stderr);
    assert_eq!(agent.state.sessions(), 0, "the busy device was not touched");

    // Detached: the daemon refuses the submission for the same reason.
    let busy_detached = run(
        home.path(),
        workspace.path(),
        &[
            "run",
            "--detach",
            "--on-busy",
            "fail",
            "--play",
            "plays/basic.play",
        ],
    );
    assert_eq!(busy_detached.code, EXIT_INFRA);
    assert!(
        busy_detached.stderr.contains("device lease busy"),
        "{}",
        busy_detached.stderr
    );

    // Queue returns immediately for a detached run and records its position.
    let queued = run(
        home.path(),
        workspace.path(),
        &[
            "run",
            "--detach",
            "--on-busy",
            "queue",
            "--play",
            "plays/basic.play",
        ],
    );
    assert_eq!(queued.code, EXIT_OK, "stderr={}", queued.stderr);
    let queued_id = submitted_id(&queued.stdout);
    assert!(
        wait_until(
            || {
                store.read(&queued_id).ok().flatten().is_some_and(|record| {
                    record.status == JobStatus::Queued
                        && record.queue_position == Some(1)
                        && record.message.is_none()
                })
            },
            Duration::from_secs(20),
        ),
        "queued job never reported position 1: {:?}",
        store.read(&queued_id).ok().flatten()
    );
    assert_eq!(
        agent.state.sessions(),
        0,
        "a queued run waits for the lease"
    );

    // The queued run stops without ever touching the device.
    let stop_queued = run(home.path(), workspace.path(), &["stop", &queued_id]);
    assert_eq!(stop_queued.code, EXIT_OK, "stderr={}", stop_queued.stderr);
    let waited = run(home.path(), workspace.path(), &["wait", &queued_id]);
    assert_eq!(waited.code, EXIT_INTERRUPTED, "stderr={}", waited.stderr);
    let record = store.read(&queued_id).expect("reads").expect("record");
    assert_eq!(record.status, JobStatus::Stopped);
    assert_eq!(agent.state.sessions(), 0, "the queued run never ran");

    std::fs::remove_file(&holder).expect("release holder lease");
    assert!(
        store
            .lease_dir()
            .read_dir()
            .expect("leases")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with("wait.")),
        "the queued run removed its wait file"
    );
}

#[test]
fn detached_submission_releases_the_callers_pipes() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(
        workspace.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("basic.play", PLAY_BASIC)],
    );

    // `run --detach` returns while the daemon keeps writing the run. If the
    // daemon inherited this process's stdout/stderr pipes, `output()` would
    // block until the daemon idles (30 s) or forever for a queued job.
    let started = Instant::now();
    let detached = run(home.path(), workspace.path(), &["run", "--detach"]);
    let elapsed = started.elapsed();
    assert_eq!(detached.code, EXIT_OK, "stderr={}", detached.stderr);
    assert!(
        elapsed < Duration::from_secs(10),
        "run --detach held the caller's pipes for {elapsed:?}"
    );
    let id = submitted_id(&detached.stdout);
    let waited = run(home.path(), workspace.path(), &["wait", &id]);
    assert_eq!(waited.code, EXIT_OK, "stderr={}", waited.stderr);
}

#[test]
fn detached_job_keeps_running_while_attach_is_not_connected() {
    let agent = TestAgent::start(MockAgentBuilder::new().delay_kind("validate_check", 400));
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(
        workspace.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("basic.play", PLAY_BASIC)],
    );

    let detached = run(home.path(), workspace.path(), &["run", "--detach"]);
    assert_eq!(detached.code, EXIT_OK, "stderr={}", detached.stderr);
    let id = submitted_id(&detached.stdout);

    // The submitting CLI has exited; the daemon still owns the run.
    let waited = run(home.path(), workspace.path(), &["wait", &id]);
    assert_eq!(waited.code, EXIT_OK, "stderr={}", waited.stderr);

    // Attaching after completion prints the recorded console log and the
    // final status without starting a second run.
    let attach = run(home.path(), workspace.path(), &["attach", &id]);
    assert_eq!(attach.code, EXIT_OK, "stderr={}", attach.stderr);
    assert!(attach.stdout.contains("web1"), "{}", attach.stdout);
    assert_eq!(agent.state.sessions(), 1);
}

#[test]
fn interrupted_jobs_and_stale_leases_are_recovered() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path(), "", &[], &[]);
    let home_path = Home::at(home.path());
    let store = JobStore::new(&home_path);
    std::fs::create_dir_all(store.dir("dead")).expect("job dir");
    let record = JobRecord {
        schema_version: 1,
        id: "dead".to_string(),
        status: JobStatus::Running,
        detached: true,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        started_at: Some("2026-01-01T00:00:00Z".to_string()),
        finished_at: None,
        duration_ms: None,
        exit_code: None,
        workspace: workspace.path().display().to_string(),
        devices: vec!["web1".to_string()],
        plays: vec!["Basic".to_string()],
        label: "1 play(s) x 1 device(s)".to_string(),
        pid: Some(0),
        queue_position: None,
        run_dir: workspace.path().join("reports/dead").display().to_string(),
        message: None,
    };
    std::fs::write(
        store.dir("dead").join("job.json"),
        serde_json::to_string_pretty(&record).expect("json"),
    )
    .expect("write record");
    std::fs::create_dir_all(store.lease_dir()).expect("lease dir");
    std::fs::write(
        store.lease_dir().join("web1.json"),
        json!({"job": "dead", "pid": 0, "devices": ["web1"], "started_ms": 1}).to_string(),
    )
    .expect("stale lease");

    let listed = run(home.path(), workspace.path(), &["jobs", "--all", "--json"]);
    assert_eq!(listed.code, EXIT_OK, "stderr={}", listed.stderr);
    let records: Vec<Value> = serde_json::from_str(&listed.stdout).expect("jobs json");
    let dead = records
        .iter()
        .find(|record| record["id"] == "dead")
        .expect("dead job listed");
    assert_eq!(dead["status"], "interrupted");
    assert_eq!(dead["exit_code"], 130);
    assert!(
        dead["message"]
            .as_str()
            .is_some_and(|message| message.contains("gone")),
        "{dead}"
    );
    assert!(store.dir("dead").join("interrupted").is_file());
    assert!(
        !store.lease_dir().join("web1.json").exists(),
        "a dead owner's lease is removed"
    );
}

#[test]
fn daemon_serves_until_idle() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path(), "", &[], &[]);
    let binary = assert_cmd::cargo::cargo_bin("impresario");
    let mut child = std::process::Command::new(binary)
        .env("IMPRESARIO_HOME", home.path())
        .arg("--workspace")
        .arg(workspace.path())
        .arg("--no-tui")
        .arg("--plain")
        .arg("--no-color")
        .args(["daemon", "--serve", "--idle-seconds", "1"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("daemon spawns");
    let started = Instant::now();
    let status: std::process::ExitStatus = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "daemon did not become idle"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status.code(), Some(EXIT_OK));
    assert!(home.path().join("jobs/daemon.token").is_file());
}
