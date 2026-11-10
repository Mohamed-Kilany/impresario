//! `impresario probe` CLI tests: a mock executor, a disposable workspace and
//! the real `run_with` entry point. They cover the report/evidence artifacts
//! and the probe-specific exit codes without touching hardware.

mod common;

use common::{argv, read_report_json, run_dir, write_workspace, TestAgent};
use impresario_clients::testing::MockAgentBuilder;
use impresario_core::command::WireErrorCode;
use impresario_master::cli::run_with;

const EXIT_OK: i32 = 0;
const EXIT_TEST_FAILED: i32 = 1;
const EXIT_INFRA: i32 = 3;

#[test]
fn probe_showing_writes_report_and_evidence_bundle() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[],
    );

    let code = run_with(&argv(
        temp.path(),
        &[
            "probe",
            "showing",
            "--device",
            "web1",
            "--piece",
            "TestScreen.A",
        ],
    ));
    assert_eq!(code, EXIT_OK);

    let dir = run_dir(temp.path());
    assert!(dir.join("report.json").is_file());
    let evidence: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("evidence.json")).expect("evidence"),
    )
    .expect("evidence parses");
    assert_eq!(evidence["schema_version"], 1);
    assert_eq!(evidence["kind"], "showing");
    assert_eq!(evidence["device"], "web1");
    assert_eq!(evidence["platform"], "web");
    assert_eq!(evidence["selector"]["strategy"], "CSS");
    assert_eq!(evidence["selector"]["value"], "#a");
    assert_eq!(evidence["claims"][0]["id"], "showing=present");
    assert_eq!(
        evidence["result"]["verdicts"][0]["claim"],
        "showing=present"
    );
    assert_eq!(evidence["result"]["verdicts"][0]["verdict"], "pass");

    let report = read_report_json(temp.path());
    assert_eq!(report["schema_version"], 3);
    let step = &report["devices"][0]["plays"][0]["acts"][0]["attempts"][0]["stages"][0]["steps"][0];
    assert_eq!(step["status"], "passed");
    assert_eq!(step["probe"]["verdicts"][0]["claim"], "showing=present");
    assert_eq!(step["probe"]["verdicts"][0]["verdict"], "pass");
}

#[test]
fn probe_player_accepts_a_raw_selector_and_claims() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[],
    );

    let code = run_with(&argv(
        temp.path(),
        &[
            "probe",
            "player",
            "--device",
            "web1",
            "--selector",
            "#a",
            "--claim",
            "state=Playing",
        ],
    ));
    assert_eq!(code, EXIT_OK);
    let report = read_report_json(temp.path());
    assert_eq!(report["status"], "passed");
    assert_eq!(
        report["devices"][0]["plays"][0]["acts"][0]["attempts"][0]["stages"][0]["steps"][0]
            ["probe"]["verdicts"][0]["claim"],
        "state=Playing"
    );
}

#[test]
fn probe_failure_exits_one_and_keeps_the_report() {
    let agent = TestAgent::start(MockAgentBuilder::new().fail_command(
        1,
        WireErrorCode::CheckFailed,
        "probe rejected",
    ));
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(
        temp.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[],
    );

    let code = run_with(&argv(
        temp.path(),
        &["probe", "showing", "--device", "web1", "--selector", "#a"],
    ));
    assert_eq!(code, EXIT_TEST_FAILED);
    let report = read_report_json(temp.path());
    assert_eq!(report["status"], "failed");
    assert_eq!(report["exit_code"], 1);
    assert!(
        !run_dir(temp.path()).join("evidence.json").exists(),
        "a command-level failure has no canonical probe result"
    );
}

#[test]
fn probe_rejects_an_unknown_device_as_infrastructure() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_workspace(temp.path(), "", &[], &[]);

    let code = run_with(&argv(
        temp.path(),
        &["probe", "showing", "--device", "nope", "--selector", "#a"],
    ));
    assert_eq!(code, EXIT_INFRA);
}
