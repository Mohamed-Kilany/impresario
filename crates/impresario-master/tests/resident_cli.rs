//! Resident CLI: schedule/watch editing and the service status surface.
//!
//! These tests do not start a resident process or touch a device. They pin the
//! YAML editing contract and the command-facing schedule arithmetic.

mod common;

use std::path::Path;

use assert_cmd::Command;
use common::write_workspace;
use serde_yaml_ng::Value;

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 2;

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(home: &Path, workspace: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::cargo_bin("impresario").expect("binary");
    command.env("IMPRESARIO_HOME", home);
    if let Some(workspace) = workspace {
        command.arg("--workspace").arg(workspace);
    }
    let output = command.args(args).output().expect("impresario runs");
    Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[test]
fn schedule_and_watch_editing_preserves_the_workspace_file() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path(), "", &[], &[]);
    let config = workspace.path().join("impresario.config");
    let original = std::fs::read_to_string(&config).expect("config");
    let with_comment = format!("{original}# resident sections start here\n");
    std::fs::write(&config, with_comment).expect("write config");

    let added = run(
        home.path(),
        Some(workspace.path()),
        &[
            "schedule",
            "add",
            "nightly",
            "--when",
            "kind: daily, at: '02:30'",
            "--play",
            "smoke.play",
            "--tag",
            "smoke",
        ],
    );
    assert_eq!(added.code, EXIT_OK, "stderr={}", added.stderr);
    assert!(added.stdout.contains("added schedule 'nightly'"));

    let text = std::fs::read_to_string(&config).expect("config after add");
    assert!(text.contains("# resident sections start here"), "{text}");
    let value: Value = serde_yaml_ng::from_str(&text).expect("valid YAML");
    assert_eq!(value["schedules"][0]["name"], "nightly");
    assert_eq!(value["schedules"][0]["when"]["kind"], "daily");
    assert_eq!(value["schedules"][0]["run"]["plays"][0], "smoke.play");

    let listed = run(
        home.path(),
        Some(workspace.path()),
        &["schedule", "list", "--json"],
    );
    assert_eq!(listed.code, EXIT_OK, "stderr={}", listed.stderr);
    let schedules: Vec<Value> = serde_json::from_str(&listed.stdout).expect("schedule json");
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0]["name"], "nightly");

    let next = run(
        home.path(),
        Some(workspace.path()),
        &["schedule", "next", "nightly", "--count", "2"],
    );
    assert_eq!(next.code, EXIT_OK, "stderr={}", next.stderr);
    let fire_lines: Vec<&str> = next
        .stdout
        .lines()
        .filter(|line| line.trim_start().starts_with("20"))
        .collect();
    assert_eq!(fire_lines.len(), 2, "{}", next.stdout);

    let disabled = run(
        home.path(),
        Some(workspace.path()),
        &["schedule", "disable", "nightly"],
    );
    assert_eq!(disabled.code, EXIT_OK, "stderr={}", disabled.stderr);
    let value: Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(&config).expect("config")).expect("yaml");
    assert_eq!(value["schedules"][0]["enabled"], false);
    assert!(std::fs::read_to_string(&config)
        .expect("config")
        .contains("# resident sections start here"));

    let watch = run(
        home.path(),
        Some(workspace.path()),
        &[
            "watch",
            "add",
            "soak",
            "--play",
            "smoke.play",
            "--gap",
            "30s",
        ],
    );
    assert_eq!(watch.code, EXIT_OK, "stderr={}", watch.stderr);
    let text = std::fs::read_to_string(&config).expect("config after watch");
    let value: Value = serde_yaml_ng::from_str(&text).expect("valid YAML");
    assert_eq!(value["watchers"][0]["name"], "soak");
    assert_eq!(value["watchers"][0]["pacing"]["gap"], "30s");

    let status = run(
        home.path(),
        Some(workspace.path()),
        &["watch", "status", "--json"],
    );
    assert_eq!(status.code, EXIT_OK, "stderr={}", status.stderr);
    let status: Value = serde_json::from_str(&status.stdout).expect("watch status json");
    assert_eq!(status["watchers"][0]["name"], "soak");

    let removed = run(
        home.path(),
        Some(workspace.path()),
        &["schedule", "rm", "nightly"],
    );
    assert_eq!(removed.code, EXIT_OK, "stderr={}", removed.stderr);
    let text = std::fs::read_to_string(&config).expect("config after remove");
    let value: Value = serde_yaml_ng::from_str(&text).expect("valid YAML");
    assert!(
        value["schedules"].is_null()
            || value["schedules"]
                .as_sequence()
                .is_some_and(|items| items.is_empty())
    );
}

#[test]
fn a_second_schedule_and_edit_preserve_existing_entries_and_comments() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path(), "", &[], &[]);
    let config = workspace.path().join("impresario.config");

    let first = run(
        home.path(),
        Some(workspace.path()),
        &[
            "schedule",
            "add",
            "nightly",
            "--when",
            "kind: daily, at: '02:30'",
            "--play",
            "smoke.play",
        ],
    );
    assert_eq!(first.code, EXIT_OK, "stderr={}", first.stderr);
    let text = std::fs::read_to_string(&config).expect("config after first add");
    std::fs::write(&config, format!("{text}# keep this comment\n")).expect("comment");

    let second = run(
        home.path(),
        Some(workspace.path()),
        &[
            "schedule",
            "add",
            "weekly",
            "--when",
            "kind: weekly, days: [mon], at: '03:00'",
            "--play",
            "smoke.play",
        ],
    );
    assert_eq!(second.code, EXIT_OK, "stderr={}", second.stderr);
    let text = std::fs::read_to_string(&config).expect("config after second add");
    assert!(text.contains("# keep this comment"), "{text}");
    let value: Value = serde_yaml_ng::from_str(&text).expect("valid YAML");
    let schedules = value["schedules"].as_sequence().expect("schedules");
    assert_eq!(schedules.len(), 2);
    assert_eq!(schedules[0]["name"], "nightly");
    assert_eq!(schedules[1]["name"], "weekly");

    let edited = run(
        home.path(),
        Some(workspace.path()),
        &[
            "schedule",
            "edit",
            "weekly",
            "--yaml",
            "name: weekly\nwhen: { kind: weekly, days: [tue], at: '04:00' }\nrun: { plays: [smoke.play] }\n",
        ],
    );
    assert_eq!(edited.code, EXIT_OK, "stderr={}", edited.stderr);
    let text = std::fs::read_to_string(&config).expect("config after edit");
    assert!(text.contains("# keep this comment"), "{text}");
    let value: Value = serde_yaml_ng::from_str(&text).expect("valid YAML");
    assert_eq!(value["schedules"][1]["name"], "weekly");
    assert_eq!(value["schedules"][1]["when"]["days"][0], "tue");
}

#[test]
fn yaml_add_uses_the_command_name_when_the_document_omits_it() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path(), "", &[], &[]);

    let added = run(
        home.path(),
        Some(workspace.path()),
        &[
            "schedule",
            "add",
            "from-yaml",
            "--yaml",
            "when: { kind: daily, at: '03:00' }\nrun: { plays: [smoke.play] }\n",
        ],
    );
    assert_eq!(added.code, EXIT_OK, "stderr={}", added.stderr);

    let text = std::fs::read_to_string(workspace.path().join("impresario.config"))
        .expect("config after yaml add");
    let value: Value = serde_yaml_ng::from_str(&text).expect("valid YAML");
    assert_eq!(value["schedules"][0]["name"], "from-yaml");
    assert_eq!(value["schedules"][0]["when"]["at"], "03:00");
}

#[test]
fn invalid_schedule_names_and_missing_arguments_fail_as_usage_errors() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path(), "", &[], &[]);

    let bad_name = run(
        home.path(),
        Some(workspace.path()),
        &[
            "schedule",
            "add",
            "two words",
            "--when",
            "kind: daily, at: '02:30'",
        ],
    );
    assert_eq!(bad_name.code, EXIT_USAGE, "stderr={}", bad_name.stderr);
    assert!(
        bad_name.stderr.contains("name must be"),
        "{}",
        bad_name.stderr
    );

    let missing_when = run(
        home.path(),
        Some(workspace.path()),
        &["schedule", "add", "nightly"],
    );
    assert_eq!(missing_when.code, EXIT_USAGE);
    assert!(missing_when.stderr.contains("provide --when"));

    let stopped = run(
        home.path(),
        Some(workspace.path()),
        &["service", "status", "--json"],
    );
    assert_eq!(stopped.code, EXIT_OK, "stderr={}", stopped.stderr);
    let status: Value = serde_json::from_str(&stopped.stdout).expect("status json");
    assert_eq!(status["running"], false);
}

#[test]
fn notify_lists_events_and_resolves_the_recorded_report() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path(), "", &[], &[]);
    let report = workspace
        .path()
        .join("reports")
        .join("job-1")
        .join("report.html");
    std::fs::create_dir_all(report.parent().expect("report parent")).expect("report dir");
    std::fs::write(&report, "<html>report</html>").expect("report");
    let events = home.path().join("service").join("events.jsonl");
    std::fs::create_dir_all(events.parent().expect("events parent")).expect("service dir");
    let event = serde_json::json!({
        "kind": "watch_notify",
        "id": "job-1",
        "watcher": "soak",
        "event": "device_unreachable",
        "status": "failed",
        "channels": ["toast", "webhook:ops"],
        "job": "job-1",
        "report": report.display().to_string(),
        "at": "2026-11-05T10:00:00+00:00"
    });
    std::fs::write(&events, format!("{event}\n")).expect("events");

    let listed = run(
        home.path(),
        Some(workspace.path()),
        &["notify", "list", "--json"],
    );
    assert_eq!(listed.code, EXIT_OK, "stderr={}", listed.stderr);
    let events: Vec<Value> = serde_json::from_str(&listed.stdout).expect("notify json");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["watcher"], "soak");
    assert_eq!(events[0]["id"], "job-1");

    let opened = run(
        home.path(),
        Some(workspace.path()),
        &["notify", "open", "job-1", "--print"],
    );
    assert_eq!(opened.code, EXIT_OK, "stderr={}", opened.stderr);
    assert_eq!(opened.stdout.trim(), report.display().to_string());

    let missing = run(
        home.path(),
        Some(workspace.path()),
        &["notify", "open", "not-a-job", "--print"],
    );
    assert_eq!(missing.code, EXIT_USAGE, "stderr={}", missing.stderr);
    assert!(
        missing.stderr.contains("no report found"),
        "{}",
        missing.stderr
    );
}

#[test]
fn watch_add_writes_and_lists_the_lease_mode_and_rejects_no_gap_low_priority() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path(), "", &[], &[]);
    let config = workspace.path().join("impresario.config");

    let added = run(
        home.path(),
        Some(workspace.path()),
        &[
            "watch",
            "add",
            "hold",
            "--play",
            "smoke.play",
            "--lease",
            "no-gap",
        ],
    );
    assert_eq!(added.code, EXIT_OK, "stderr={}", added.stderr);
    let value: Value =
        serde_yaml_ng::from_str(&std::fs::read_to_string(&config).expect("config")).expect("yaml");
    assert_eq!(value["watchers"][0]["lease"], "no_gap");

    let listed = run(home.path(), Some(workspace.path()), &["watch", "list"]);
    assert_eq!(listed.code, EXIT_OK, "stderr={}", listed.stderr);
    assert!(listed.stdout.contains("lease=no_gap"), "{}", listed.stdout);

    let invalid = run(
        home.path(),
        Some(workspace.path()),
        &[
            "watch",
            "add",
            "starve",
            "--yaml",
            "run: { plays: [smoke.play] }\nlease: no_gap\npriority: low\n",
        ],
    );
    assert_eq!(invalid.code, EXIT_USAGE, "stderr={}", invalid.stderr);
    assert!(
        invalid.stderr.contains("priority low"),
        "{}",
        invalid.stderr
    );
}
