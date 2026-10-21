//! Saved run profiles (TASK-2 3.4) against a real global `config.yaml`.
//!
//! The writer must preserve every unrelated byte of the file: comments,
//! ordering and unknown sections are user data, not something the CLI owns.

mod common;

use std::path::Path;

use assert_cmd::Command;
use common::{write_workspace, TestAgent, PLAY_BASIC};
use impresario_clients::testing::MockAgentBuilder;
use serde_yaml_ng::Value;

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 2;
const EXIT_INFRA: i32 = 3;

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

fn config(home: &Path) -> std::path::PathBuf {
    home.join("config.yaml")
}

#[test]
fn profile_save_list_show_delete_preserves_unrelated_yaml() {
    let home = tempfile::tempdir().expect("home");
    let original = "# keep the top comment\nproject: global-demo\nui:\n  theme: dark  # inline comment\nprofiles:\n  old:\n    device:\n      - web1\n";
    std::fs::write(config(home.path()), original).expect("write config");

    let saved = run(
        home.path(),
        None,
        &[
            "profile",
            "save",
            "ci",
            "--device",
            "web2",
            "--play",
            "smoke.play",
            "--tag",
            "smoke",
            "--reporter",
            "json",
            "--fail-fast",
        ],
    );
    assert_eq!(saved.code, EXIT_OK, "stderr={}", saved.stderr);
    assert!(
        saved.stdout.contains("saved profile 'ci'"),
        "{}",
        saved.stdout
    );

    let text = std::fs::read_to_string(config(home.path())).expect("config");
    assert!(text.contains("# keep the top comment"), "{text}");
    assert!(text.contains("# inline comment"), "{text}");
    assert!(text.contains("project: global-demo"), "{text}");
    let value: Value = serde_yaml_ng::from_str(&text).expect("still valid YAML");
    assert_eq!(value["ui"]["theme"], "dark");
    assert_eq!(value["profiles"]["ci"]["device"][0], "web2");
    assert_eq!(value["profiles"]["ci"]["play"][0], "smoke.play");
    assert_eq!(value["profiles"]["ci"]["tag"][0], "smoke");
    assert_eq!(value["profiles"]["ci"]["reporter"], "json");
    assert_eq!(value["profiles"]["ci"]["fail_fast"], true);
    assert_eq!(value["profiles"]["old"]["device"][0], "web1");

    let list = run(home.path(), None, &["profile", "list"]);
    assert_eq!(list.code, EXIT_OK, "stderr={}", list.stderr);
    assert_eq!(list.stdout.lines().collect::<Vec<_>>(), vec!["ci", "old"]);

    let json = run(home.path(), None, &["profile", "list", "--json"]);
    assert_eq!(json.code, EXIT_OK, "stderr={}", json.stderr);
    let names: Vec<String> = serde_json::from_str(&json.stdout).expect("names json");
    assert_eq!(names, vec!["ci", "old"]);

    let show = run(home.path(), None, &["profile", "show", "ci"]);
    assert_eq!(show.code, EXIT_OK, "stderr={}", show.stderr);
    let shown: Value = serde_yaml_ng::from_str(&show.stdout).expect("profile yaml");
    assert_eq!(shown["device"][0], "web2");
    assert_eq!(shown["fail_fast"], true);

    let removed = run(home.path(), None, &["profile", "rm", "ci"]);
    assert_eq!(removed.code, EXIT_OK, "stderr={}", removed.stderr);
    let text = std::fs::read_to_string(config(home.path())).expect("config");
    assert!(text.contains("# keep the top comment"), "{text}");
    let value: Value = serde_yaml_ng::from_str(&text).expect("still valid YAML");
    assert!(value["profiles"].get("ci").is_none());
    assert_eq!(value["profiles"]["old"]["device"][0], "web1");

    let removed = run(home.path(), None, &["profile", "delete", "old"]);
    assert_eq!(removed.code, EXIT_OK, "stderr={}", removed.stderr);
    let text = std::fs::read_to_string(config(home.path())).expect("config");
    let value: Value = serde_yaml_ng::from_str(&text).expect("still valid YAML");
    assert!(value.get("profiles").is_none());
    assert!(text.contains("# keep the top comment"), "{text}");

    let unknown = run(home.path(), None, &["profile", "delete", "missing"]);
    assert_eq!(unknown.code, EXIT_USAGE);
    assert!(
        unknown.stderr.contains("unknown profile"),
        "{}",
        unknown.stderr
    );

    let bad_name = run(home.path(), None, &["profile", "save", "two words"]);
    assert_eq!(bad_name.code, EXIT_USAGE);
    assert!(
        bad_name.stderr.contains("names may contain"),
        "{}",
        bad_name.stderr
    );
}

#[test]
fn invalid_profiles_and_yaml_fail_loudly_without_writing() {
    let home = tempfile::tempdir().expect("home");
    std::fs::write(
        config(home.path()),
        "project: demo\nprofiles:\n  - not-a-mapping\n",
    )
    .expect("write config");
    let list = run(home.path(), None, &["profile", "list"]);
    assert_eq!(
        list.code, EXIT_INFRA,
        "stdout={} stderr={}",
        list.stdout, list.stderr
    );
    assert!(
        list.stderr.contains("`profiles` must be a mapping"),
        "{}",
        list.stderr
    );

    let unknown_field = "profiles:\n  ci:\n    device: [web1]\n    secret_extra: nope\n";
    std::fs::write(config(home.path()), unknown_field).expect("write config");
    let list = run(home.path(), None, &["profile", "list"]);
    assert_eq!(list.code, EXIT_INFRA);
    assert!(
        list.stderr.contains("profile 'ci' is invalid"),
        "{}",
        list.stderr
    );
    assert_eq!(
        std::fs::read_to_string(config(home.path())).expect("unchanged"),
        unknown_field
    );

    std::fs::write(config(home.path()), "profiles: [\n").expect("write config");
    let list = run(home.path(), None, &["profile", "list"]);
    assert_eq!(list.code, EXIT_INFRA);
    assert!(list.stderr.contains("is not valid YAML"), "{}", list.stderr);
}

#[test]
fn run_profile_fills_only_flags_the_command_line_left_empty() {
    let agent = TestAgent::start(MockAgentBuilder::new());
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(
        workspace.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("basic.play", PLAY_BASIC)],
    );
    std::fs::write(
        config(home.path()),
        "project: global-demo\nprofiles:\n  ci:\n    device: [web1]\n    reporter: json\n",
    )
    .expect("write config");

    let output = run(
        home.path(),
        Some(workspace.path()),
        &["run", "--profile", "ci"],
    );
    assert_eq!(
        output.code, EXIT_OK,
        "stdout={} stderr={}",
        output.stdout, output.stderr
    );
    assert!(
        output.stderr.contains("using profile 'ci'"),
        "{}",
        output.stderr
    );
    let report = workspace.path().join("reports");
    let run_dir = std::fs::read_dir(&report)
        .expect("reports")
        .next()
        .expect("one run")
        .expect("entry")
        .path();
    assert!(run_dir.join("report.json").is_file());

    // An explicit command-line device still wins; the profile does not
    // replace (and therefore does not hide) it.
    let overridden = run(
        home.path(),
        Some(workspace.path()),
        &["run", "--profile", "ci", "--device", "missing"],
    );
    assert_eq!(overridden.code, EXIT_USAGE);
    assert!(
        overridden.stderr.contains("unknown device"),
        "{}",
        overridden.stderr
    );

    let unknown = run(
        home.path(),
        Some(workspace.path()),
        &["run", "--profile", "nope"],
    );
    assert_eq!(unknown.code, EXIT_USAGE);
    assert!(
        unknown.stderr.contains("unknown profile"),
        "{}",
        unknown.stderr
    );
}
