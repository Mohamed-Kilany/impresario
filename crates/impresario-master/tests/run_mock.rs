//! Executor integration tests against `MockAgent` (spec 15): exit codes,
//! failure capture, timeouts/cancellation, reconnects, retries, fail-fast,
//! parallelism, app-reset policies, selection filters, capability gaps,
//! redaction and Ctrl-C.

mod common;

use std::time::{Duration, Instant};

use common::{
    build_test_plan, build_test_plan_with, device, exec_options, test_config, with_app_reset,
    TestAgent, PLAY_BASIC, PLAY_HANG, PLAY_RECONNECT, PLAY_SHOT, PLAY_TAGGED, PLAY_TWO_ACTS,
};
use impresario_clients::log::{LogLevel, LogRecord};
use impresario_clients::testing::{MockAgentBuilder, MockScript};
use impresario_core::command::WireErrorCode;
use impresario_core::config::AppReset;
use impresario_core::platforms::Platform;
use impresario_master::exec::{execute, ExecOutcome};
use impresario_master::plan::Selection;
use impresario_master::report::{
    ActReport, AttemptReport, DeviceReport, RunReport, RunStatus, Status,
};
use impresario_proto::v1 as pb;
use tempfile::TempDir;

const SECRET_PLAY: &str = r#"Play: Secret

Act: Leak
    Perform: Leak
"#;

/// A play that hangs until cancelled; the 30 s step timeout keeps the command
/// in flight while the test cancels the run.
const PLAY_LONG_HANG: &str = r#"Play: Long

Act: Hang
    Perform: LongHang
"#;

fn device_of<'a>(report: &'a RunReport, name: &str) -> &'a DeviceReport {
    report
        .devices
        .iter()
        .find(|device| device.name == name)
        .unwrap_or_else(|| panic!("device {name} missing"))
}

fn act_of<'a>(device: &'a DeviceReport, name: &str) -> &'a ActReport {
    device
        .plays
        .iter()
        .flat_map(|play| &play.acts)
        .find(|act| act.name == name)
        .unwrap_or_else(|| panic!("act {name} missing"))
}

fn attempt_of(act: &ActReport, n: u32) -> &AttemptReport {
    act.attempts
        .iter()
        .find(|attempt| attempt.n == n)
        .unwrap_or_else(|| panic!("attempt {n} missing"))
}

fn step_statuses(attempt: &AttemptReport) -> Vec<Status> {
    attempt
        .stages
        .iter()
        .flat_map(|stage| &stage.steps)
        .map(|step| step.status)
        .collect()
}

fn kinds(agent: &TestAgent) -> String {
    agent.state.command_kinds().join(",")
}

fn app_control(command: &pb::Command) -> (i32, bool) {
    match command.body.as_ref() {
        Some(pb::command::Body::AppControl(control)) => (control.op, control.clear_data),
        other => panic!("expected app_control, got {other:?}"),
    }
}

async fn run_play_in(
    agent: &TestAgent,
    policy: AppReset,
    temp: &TempDir,
    run_id: &str,
    play_source: &str,
) -> ExecOutcome {
    let config = with_app_reset(test_config(""), policy);
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, play_source);
    let options = exec_options(temp, config, run_id);
    execute(&plan, &options).await
}

#[tokio::test]
async fn passing_play_exits_zero() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let options = exec_options(&temp, config, "run-pass");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.report.status, RunStatus::Passed);
    let device = device_of(&outcome.report, "web1");
    assert_eq!(device.platform, "web");
    assert!(device.error.is_none());
    assert_eq!(
        device.device_info.as_ref().expect("device info").model,
        "Mock Device"
    );
    let act = act_of(device, "Pass");
    assert_eq!(act.status, Status::Passed);
    assert!(!act.flaky);
    assert_eq!(act.attempts.len(), 1);
    assert!(attempt_of(act, 1).failure.is_none());
    assert_eq!(step_statuses(attempt_of(act, 1)), vec![Status::Passed; 3]);

    let summary = outcome.report.summary();
    assert_eq!(summary.acts_passed, 1);
    assert_eq!(summary.acts_failed, 0);
    assert_eq!(summary.steps_total, 3);
    assert_eq!(summary.steps_failed, 0);

    assert_eq!(kinds(&agent), "validate_check,run_action,run_action");
    assert_eq!(agent.state.handshakes(), 1);
    assert_eq!(agent.state.sessions(), 1);
    assert_eq!(agent.state.shutdowns(), 1);
    assert!(!options.run_dir().join("screenshots").exists());
}

#[tokio::test]
async fn failing_step_exits_one_with_failure_capture() {
    let agent = TestAgent::start(MockAgentBuilder::new().fail_command(
        2,
        WireErrorCode::CheckFailed,
        "boom",
    ));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let options = exec_options(&temp, config, "run-fail");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 1);
    assert_eq!(outcome.report.status, RunStatus::Failed);
    let device = device_of(&outcome.report, "web1");
    assert!(
        device.error.is_none(),
        "a step failure is not a device error"
    );
    let act = act_of(device, "Pass");
    assert_eq!(act.status, Status::Failed);
    assert!(!act.flaky);
    let attempt = attempt_of(act, 1);
    let failure = attempt.failure.as_ref().expect("failure detail");
    assert_eq!(failure.step_index, 1);
    assert_eq!(failure.code, "CHECK_FAILED");
    assert_eq!(failure.message, "boom");
    assert_eq!(
        failure.screenshot.as_deref(),
        Some("screenshots/web1/failure-1.png")
    );
    assert_eq!(
        step_statuses(attempt),
        vec![Status::Passed, Status::Failed, Status::Skipped]
    );
    let on_disk = temp
        .path()
        .join("reports/run-fail/screenshots/web1/failure-1.png");
    assert_eq!(
        std::fs::read(on_disk).expect("screenshot on disk"),
        b"mock-png"
    );

    // 3 = the in-session failure capture (not the out-of-band RPC).
    assert_eq!(kinds(&agent), "validate_check,run_action,screenshot");
    assert_eq!(agent.state.captures(), 0);
    assert_eq!(agent.state.shutdowns(), 1);
}

#[tokio::test]
async fn hung_command_times_out_and_is_cancelled() {
    let agent = TestAgent::start(MockAgentBuilder::new().hang_kind("run_action"));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_HANG);
    let options = exec_options(&temp, config, "run-hang");

    let started = Instant::now();
    let outcome = execute(&plan, &options).await;
    let elapsed = started.elapsed();

    assert_eq!(
        outcome.exit_code, 1,
        "TIMEOUT is a test failure, not exit 3"
    );
    assert_eq!(outcome.report.status, RunStatus::Failed);
    let act = act_of(device_of(&outcome.report, "web1"), "Hang");
    assert_eq!(act.status, Status::Failed);
    let attempt = attempt_of(act, 1);
    let failure = attempt.failure.as_ref().expect("failure detail");
    assert_eq!(failure.code, "TIMEOUT");
    assert!(
        elapsed >= Duration::from_millis(500) && elapsed < Duration::from_secs(5),
        "overrun should take about the 400 ms deadline, took {elapsed:?}"
    );

    // Spec 7.2.7: the overrun path cancels the in-flight command by id.
    let commands = agent.state.commands();
    assert_eq!(kinds(&agent), "run_action,cancel");
    match commands.get(1).and_then(|command| command.body.as_ref()) {
        Some(pb::command::Body::Cancel(cancel)) => {
            assert_eq!(cancel.target_command_id, commands[0].command_id);
        }
        other => panic!("expected cancel, got {other:?}"),
    }

    // The failure capture falls back to the out-of-band RPC once the
    // session closed with the deadline.
    assert_eq!(agent.state.captures(), 1);
    assert_eq!(
        failure.screenshot.as_deref(),
        Some("screenshots/web1/failure-1.png")
    );
    assert_eq!(agent.state.shutdowns(), 1);
}

#[tokio::test]
async fn dropped_stream_reconnects_before_the_next_act() {
    let agent = TestAgent::start(MockAgentBuilder::new().drop_stream_after_results(1));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_RECONNECT);
    let options = exec_options(&temp, config, "run-reconnect");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.report.status, RunStatus::Passed);
    let device = device_of(&outcome.report, "web1");
    assert!(device.error.is_none());
    let first = act_of(device, "First");
    assert_eq!(first.status, Status::Passed);
    assert_eq!(
        step_statuses(attempt_of(first, 1)),
        vec![Status::Passed, Status::Passed]
    );
    let second = act_of(device, "Second");
    assert_eq!(second.status, Status::Passed);
    // The reconnected session carries exactly the second act's one step.
    assert_eq!(step_statuses(attempt_of(second, 1)), vec![Status::Passed]);

    assert_eq!(agent.state.handshakes(), 2);
    assert_eq!(agent.state.sessions(), 2);
    assert_eq!(kinds(&agent), "validate_check,validate_check");
    assert_eq!(agent.state.shutdowns(), 1);
}

#[tokio::test]
async fn retries_restart_the_app_and_mark_flaky() {
    let agent = TestAgent::start(MockAgentBuilder::new().fail_command(
        3,
        WireErrorCode::ElementNotFound,
        "click miss",
    ));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = with_app_reset(test_config(""), AppReset::PerAct);
    let devices = vec![device("web1", &agent.address)];
    let mut plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let mut platforms = std::collections::BTreeMap::new();
    platforms.insert(
        Platform::Web,
        impresario_core::apps::AppPlatform {
            url: Some("http://localhost/index.html".to_string()),
            ..Default::default()
        },
    );
    plan.devices[0].app = Some(impresario_core::apps::AppConfig {
        name: "web_app".to_string(),
        description: None,
        platforms,
        options: impresario_core::apps::AppOptions {
            clear_data: true,
            launch_params: std::collections::BTreeMap::new(),
        },
    });
    let mut options = exec_options(&temp, config, "run-flaky");
    options.retries = 1;
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.report.status, RunStatus::Passed);
    let act = act_of(device_of(&outcome.report, "web1"), "Pass");
    assert_eq!(act.status, Status::Passed);
    assert!(act.flaky, "a pass after a failure is flaky");
    assert_eq!(act.attempts.len(), 2);
    assert_eq!(attempt_of(act, 1).status, Status::Failed);
    assert_eq!(
        attempt_of(act, 1)
            .failure
            .as_ref()
            .expect("first-attempt failure")
            .code,
        "ELEMENT_NOT_FOUND"
    );
    assert_eq!(attempt_of(act, 2).status, Status::Passed);
    assert_eq!(step_statuses(attempt_of(act, 2)), vec![Status::Passed; 3]);

    // Attempt 1: LAUNCH; the retry: RESTART — clear_data follows the device.
    assert_eq!(
        kinds(&agent),
        "app_control,validate_check,run_action,screenshot,app_control,validate_check,run_action,run_action"
    );
    let commands = agent.state.commands();
    assert_eq!(app_control(&commands[0]), (pb::AppOp::Launch as i32, true));
    assert_eq!(app_control(&commands[4]), (pb::AppOp::Restart as i32, true));
    assert_eq!(agent.state.handshakes(), 1, "no reconnect needed");
    assert_eq!(outcome.report.summary().steps_total, 6);
    assert_eq!(outcome.report.summary().steps_failed, 1);
}

#[tokio::test]
async fn fail_fast_aborts_the_other_device() {
    let failing = TestAgent::start(
        MockAgentBuilder::new()
            .delay_kind("validate_check", 100)
            .fail_command(1, WireErrorCode::CheckFailed, "first check fails"),
    );
    let slow = TestAgent::start(MockAgentBuilder::new().delay_kind("validate_check", 1000));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("parallel_devices: 2\n");
    let devices = vec![
        device("failing", &failing.address),
        device("slow", &slow.address),
    ];
    let plan = build_test_plan(&config, &devices, PLAY_TWO_ACTS);
    let mut options = exec_options(&temp, config, "run-fail-fast");
    options.fail_fast = true;
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 1);
    assert_eq!(outcome.report.status, RunStatus::Failed);

    let first = device_of(&outcome.report, "failing");
    assert!(first.error.is_none());
    assert_eq!(act_of(first, "First").status, Status::Failed);
    assert_eq!(act_of(first, "Second").status, Status::Skipped);

    let second = device_of(&outcome.report, "slow");
    assert!(second.error.is_none());
    let act = act_of(second, "First");
    assert_eq!(act.status, Status::Aborted);
    let attempt = attempt_of(act, 1);
    assert_eq!(attempt.status, Status::Aborted);
    // The in-flight check finished; the next step never reached the wire.
    assert_eq!(
        step_statuses(attempt),
        vec![Status::Passed, Status::Skipped]
    );
    assert!(attempt.failure.is_none());
    assert_eq!(act_of(second, "Second").status, Status::Skipped);
    assert_eq!(kinds(&slow), "validate_check");
}

#[tokio::test]
async fn parallel_devices_run_concurrently() {
    let agents: Vec<TestAgent> = (0..3)
        .map(|_| TestAgent::start(MockAgentBuilder::new().delay_kind("validate_check", 600)))
        .collect();
    let devices: Vec<_> = agents
        .iter()
        .enumerate()
        .map(|(index, agent)| device(&format!("web{index}"), &agent.address))
        .collect();

    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("parallel_devices: 3\n");
    let plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let options = exec_options(&temp, config, "run-parallel");
    let started = Instant::now();
    let outcome = execute(&plan, &options).await;
    let parallel = started.elapsed();
    assert_eq!(outcome.exit_code, 0);
    assert!(
        parallel < Duration::from_millis(1500),
        "3 x 600 ms in parallel took {parallel:?}"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("parallel_devices: 1\n");
    let plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let options = exec_options(&temp, config, "run-serial");
    let started = Instant::now();
    let outcome = execute(&plan, &options).await;
    let serial = started.elapsed();
    assert_eq!(outcome.exit_code, 0);
    assert!(
        serial >= Duration::from_millis(1800),
        "3 x 600 ms serialized took {serial:?}"
    );
}

#[tokio::test]
async fn app_reset_policies_drive_app_control() {
    let temp = tempfile::tempdir().expect("tempdir");

    let never = TestAgent::start(MockAgentBuilder::new());
    let outcome = run_play_in(&never, AppReset::Never, &temp, "run-never", PLAY_TWO_ACTS).await;
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(
        kinds(&never),
        "validate_check,run_action,validate_check",
        "app_reset: never sends no app control at all"
    );

    let per_act = TestAgent::start(MockAgentBuilder::new());
    let outcome = run_play_in(
        &per_act,
        AppReset::PerAct,
        &temp,
        "run-per-act",
        PLAY_TWO_ACTS,
    )
    .await;
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(
        kinds(&per_act),
        "app_control,validate_check,run_action,app_control,validate_check"
    );
    let commands = per_act.state.commands();
    assert_eq!(app_control(&commands[0]), (pb::AppOp::Launch as i32, false));
    assert_eq!(
        app_control(&commands[3]),
        (pb::AppOp::Restart as i32, false)
    );

    let per_play = TestAgent::start(MockAgentBuilder::new());
    let outcome = run_play_in(
        &per_play,
        AppReset::PerPlay,
        &temp,
        "run-per-play",
        PLAY_TWO_ACTS,
    )
    .await;
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(
        kinds(&per_play),
        "app_control,validate_check,run_action,validate_check",
        "app_reset: per_play resets once per play"
    );
    assert_eq!(
        app_control(&per_play.state.commands()[0]),
        (pb::AppOp::Launch as i32, false)
    );
}

#[tokio::test]
async fn act_selection_filters_the_plan() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let selection = |tags: &[&str], exclude_tags: &[&str], acts: &[&str]| Selection {
        tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
        exclude_tags: exclude_tags.iter().map(|tag| (*tag).to_string()).collect(),
        acts: acts.iter().map(|act| (*act).to_string()).collect(),
    };

    let tagged = TestAgent::start(MockAgentBuilder::new());
    let devices = vec![device("web1", &tagged.address)];
    let plan = build_test_plan_with(
        &config,
        &devices,
        PLAY_TAGGED,
        &selection(&["smoke"], &[], &[]),
    );
    let options = exec_options(&temp, config.clone(), "run-tag");
    let outcome = execute(&plan, &options).await;
    assert_eq!(outcome.exit_code, 0);
    let names: Vec<&str> = device_of(&outcome.report, "web1")
        .plays
        .iter()
        .flat_map(|play| play.acts.iter())
        .map(|act| act.name.as_str())
        .collect();
    assert_eq!(names, ["Smoky"]);
    assert_eq!(kinds(&tagged), "validate_check");

    let named = TestAgent::start(MockAgentBuilder::new());
    let devices = vec![device("web1", &named.address)];
    let plan = build_test_plan_with(
        &config,
        &devices,
        PLAY_TAGGED,
        &selection(&[], &[], &["Plain"]),
    );
    let options = exec_options(&temp, config.clone(), "run-act");
    let outcome = execute(&plan, &options).await;
    assert_eq!(outcome.exit_code, 0);
    let names: Vec<&str> = device_of(&outcome.report, "web1")
        .plays
        .iter()
        .flat_map(|play| play.acts.iter())
        .map(|act| act.name.as_str())
        .collect();
    assert_eq!(names, ["Plain"]);
    assert_eq!(kinds(&named), "validate_check,run_action");

    // `--exclude-tag` wins over `--tag`; a fully excluded play runs nothing.
    let excluded = TestAgent::start(MockAgentBuilder::new());
    let devices = vec![device("web1", &excluded.address)];
    let plan = build_test_plan_with(
        &config,
        &devices,
        PLAY_TAGGED,
        &selection(&["smoke"], &["smoke"], &[]),
    );
    let options = exec_options(&temp, config, "run-excluded");
    let outcome = execute(&plan, &options).await;
    assert_eq!(outcome.exit_code, 0);
    assert!(device_of(&outcome.report, "web1").plays.is_empty());
    assert_eq!(excluded.state.command_count(), 0);
}

#[tokio::test]
async fn screenshot_steps_write_pngs() {
    let agent = TestAgent::start(MockAgentBuilder::new().screenshot(b"png-one".to_vec()));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_SHOT);
    let options = exec_options(&temp, config, "run-shot");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 0);
    let act = act_of(device_of(&outcome.report, "web1"), "Capture");
    assert_eq!(
        step_statuses(attempt_of(act, 1)),
        vec![Status::Passed, Status::Passed]
    );
    let png = temp
        .path()
        .join("reports/run-shot/screenshots/web1/shot1.png");
    assert_eq!(std::fs::read(png).expect("shot1.png"), b"png-one");
    assert_eq!(kinds(&agent), "screenshot,validate_check");

    // `ok` with empty bytes passes without writing a file (decision I-20).
    let empty = TestAgent::start(MockAgentBuilder::new().screenshot(Vec::new()));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &empty.address)];
    let plan = build_test_plan(&config, &devices, PLAY_SHOT);
    let options = exec_options(&temp, config, "run-shot-empty");
    let outcome = execute(&plan, &options).await;
    assert_eq!(outcome.exit_code, 0);
    assert!(!options.run_dir().join("screenshots").exists());
}

#[tokio::test]
async fn handshake_rejection_is_a_device_error() {
    let agent = TestAgent::start(
        MockAgentBuilder::new().reject_handshake(WireErrorCode::Unauthenticated, "bad token"),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let options = exec_options(&temp, config, "run-reject");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 3);
    assert_eq!(outcome.report.status, RunStatus::Error);
    let device = device_of(&outcome.report, "web1");
    let error = device.error.as_ref().expect("device error");
    assert_eq!(error.code, "UNAUTHENTICATED");
    assert_eq!(
        error.details.get("phase").map(String::as_str),
        Some("handshake")
    );
    assert_eq!(act_of(device, "Pass").status, Status::Aborted);
    assert_eq!(agent.state.handshakes(), 1);
    assert_eq!(agent.state.sessions(), 0);
    assert_eq!(agent.state.shutdowns(), 0);
}

#[tokio::test]
async fn token_is_sent_on_every_rpc() {
    let agent = TestAgent::start(MockAgentBuilder::new().token("s3cr3t"));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let mut options = exec_options(&temp, config.clone(), "run-token");
    options.token = Some("s3cr3t".to_string());
    let outcome = execute(&plan, &options).await;
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(agent.state.log_streams(), 1, "StreamLogs carried the token");

    // A wrong token is rejected at the handshake: device error, exit 3.
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let mut options = exec_options(&temp, config, "run-token-bad");
    options.token = Some("wrong".to_string());
    let outcome = execute(&plan, &options).await;
    assert_eq!(outcome.exit_code, 3);
    assert_eq!(
        device_of(&outcome.report, "web1")
            .error
            .as_ref()
            .expect("device error")
            .code,
        "UNAUTHENTICATED"
    );
}

/// A lazy `StreamLogs` (response headers only with the first message, like
/// grpc-java) must not wedge the run: the master stops waiting after
/// `LOG_STREAM_OPEN_TIMEOUT` and continues without agent logs (decision I-74).
#[tokio::test]
async fn lazy_log_stream_does_not_wedge_the_run() {
    let agent = TestAgent::start(
        MockAgentBuilder::new()
            // Exec tests use a 5 s heartbeat timeout: beat well under it so
            // the log-stream stall cannot race the watchdog into a reconnect.
            .heartbeat_interval(Duration::from_secs(1))
            .delay_log_stream(Duration::from_secs(8)),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let options = exec_options(&temp, config, "run-lazy-logs");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.report.status, RunStatus::Passed);
    assert_eq!(
        agent.state.log_streams(),
        1,
        "the master attempted StreamLogs before giving up"
    );
}

#[tokio::test]
async fn missing_app_control_is_unsupported_at_device_level() {
    let mut script = MockScript::default();
    script
        .capabilities
        .retain(|capability| *capability != pb::Capability::AppControl as i32);
    let agent = TestAgent::start(MockAgentBuilder::new().script(script));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = with_app_reset(test_config(""), AppReset::PerAct);
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_BASIC);
    let options = exec_options(&temp, config, "run-no-app-control");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 3);
    let device = device_of(&outcome.report, "web1");
    let error = device.error.as_ref().expect("device error");
    assert_eq!(error.code, "UNSUPPORTED_ACTION");
    assert_eq!(
        error.details.get("phase").map(String::as_str),
        Some("capabilities")
    );
    assert_eq!(act_of(device, "Pass").status, Status::Aborted);
    assert_eq!(agent.state.command_count(), 0, "nothing crosses the wire");
    assert_eq!(agent.state.sessions(), 1);
}

#[tokio::test]
async fn missing_screenshot_capability_fails_the_step() {
    let mut script = MockScript::default();
    script
        .capabilities
        .retain(|capability| *capability != pb::Capability::Screenshot as i32);
    let agent = TestAgent::start(MockAgentBuilder::new().script(script));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_SHOT);
    let options = exec_options(&temp, config, "run-no-screenshot");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 1);
    let act = act_of(device_of(&outcome.report, "web1"), "Capture");
    assert_eq!(act.status, Status::Failed);
    let failure = attempt_of(act, 1).failure.as_ref().expect("failure detail");
    assert_eq!(failure.code, "UNSUPPORTED_ACTION");
    assert_eq!(failure.screenshot, None, "the capture is silently skipped");
    assert_eq!(agent.state.command_count(), 0);
    assert_eq!(agent.state.captures(), 0);
}

#[tokio::test]
async fn secrets_are_redacted_in_messages_and_logs() {
    const SECRET: &str = "hunter2-very-secret";
    std::env::set_var("IMPRESARIO_TEST_SECRET", SECRET);

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
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, SECRET_PLAY);
    let options = exec_options(&temp, config, "run-secret");
    let outcome = execute(&plan, &options).await;

    assert_eq!(outcome.exit_code, 1);
    let act = act_of(device_of(&outcome.report, "web1"), "Leak");
    let failure = attempt_of(act, 1).failure.as_ref().expect("failure detail");
    assert_eq!(failure.code, "CHECK_FAILED");
    assert!(!failure.message.contains(SECRET), "{}", failure.message);
    assert!(failure.message.contains("***"), "{}", failure.message);
    let logs = failure.log_tail.join("\n");
    assert!(logs.contains("***"), "log tail: {logs}");
    assert!(!logs.contains(SECRET), "log tail: {logs}");
    // The step source keeps the unresolved placeholder, never the value.
    let source = &attempt_of(act, 1).stages[0].steps[0].source;
    assert!(source.contains("${env.IMPRESARIO_TEST_SECRET}"));
}

#[tokio::test]
async fn ctrl_c_aborts_the_run_with_exit_130() {
    let agent = TestAgent::start(MockAgentBuilder::new().hang_kind("run_action"));
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config("");
    let devices = vec![device("web1", &agent.address)];
    let plan = build_test_plan(&config, &devices, PLAY_LONG_HANG);
    let options = exec_options(&temp, config, "run-interrupt");
    let shutdown = options.shutdown.clone();

    let task = tokio::spawn({
        let plan = plan.clone();
        let options = options.clone();
        async move { execute(&plan, &options).await }
    });
    agent
        .state
        .wait_for_commands(1, Duration::from_secs(10))
        .await;
    shutdown.cancel();
    let outcome = task.await.expect("executor task joins");

    assert_eq!(outcome.exit_code, 130);
    assert_eq!(outcome.report.status, RunStatus::Interrupted);
    let act = act_of(device_of(&outcome.report, "web1"), "Hang");
    assert_eq!(act.status, Status::Aborted);
    assert_eq!(step_statuses(attempt_of(act, 1)), vec![Status::Aborted]);
    // The in-flight command was cancelled by id before teardown.
    assert_eq!(kinds(&agent), "run_action,cancel");
    assert_eq!(agent.state.shutdowns(), 1);
}
