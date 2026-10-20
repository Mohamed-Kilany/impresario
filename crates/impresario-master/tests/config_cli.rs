//! `config` and `workspace` integration tests (TASK-2 sections 3.3 and 4).
//!
//! Each test gets its own global home via `IMPRESARIO_HOME`, so the real
//! `~/.impresario` never participates.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 2;

fn command(home: &Path, workspace: Option<&Path>) -> Command {
    let mut command = Command::cargo_bin("impresario").expect("binary");
    command
        .env("IMPRESARIO_HOME", home)
        .env_remove("IMPRESARIO_WORKSPACE");
    if let Some(workspace) = workspace {
        command.arg("--workspace").arg(workspace);
    }
    command
}

fn run(home: &Path, workspace: &Path, args: &[&str]) -> (i32, String, String) {
    let output = command(home, Some(workspace))
        .args(args)
        .output()
        .expect("run impresario");
    (
        output.status.code().expect("exit code"),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn run_in(home: &Path, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let output = command(home, None)
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run impresario");
    (
        output.status.code().expect("exit code"),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn temp(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    std::fs::create_dir_all(&path).expect("create dir");
    (dir, path)
}

#[test]
fn set_is_lossless_and_unset_restores_the_bytes() {
    let home = tempfile::tempdir().expect("home");
    let (_guard, workspace) = temp("ws");
    let config = workspace.join("impresario.config");
    let original = "# kept header\nproject: demo   # keep this comment\nreport_output: ./out\n";
    std::fs::write(&config, original).expect("write config");

    let (code, stdout, stderr) = run(
        home.path(),
        &workspace,
        &["config", "set", "timeout_default_ms", "2500"],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    let after_set = std::fs::read_to_string(&config).expect("config");
    assert!(after_set.contains("# kept header"), "{after_set}");
    assert!(
        after_set.contains("project: demo   # keep this comment"),
        "{after_set}"
    );
    assert!(
        after_set.contains("timeout_default_ms: 2500"),
        "{after_set}"
    );

    let (code, stdout, stderr) = run(
        home.path(),
        &workspace,
        &["config", "get", "timeout_default_ms", "--workspace"],
    );
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(stdout.contains("2500"), "{stdout}");
    assert!(stdout.contains("source: workspace"), "{stdout}");

    let (code, stdout, stderr) = run(
        home.path(),
        &workspace,
        &["config", "unset", "timeout_default_ms"],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    assert_eq!(std::fs::read_to_string(&config).expect("config"), original);
}

#[test]
fn invalid_and_unknown_values_are_refused_without_touching_the_file() {
    let home = tempfile::tempdir().expect("home");
    let (_guard, workspace) = temp("ws");
    let config = workspace.join("impresario.config");
    let original = "project: demo\n";
    std::fs::write(&config, original).expect("write config");

    let (code, _stdout, stderr) = run(
        home.path(),
        &workspace,
        &["config", "set", "timeout_default_ms", "banana"],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("IMP-CFG-003"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&config).expect("config"), original);

    let (code, _stdout, stderr) = run(
        home.path(),
        &workspace,
        &["config", "set", "not_a_key", "1"],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("IMP-CFG-001"), "{stderr}");

    let (code, _stdout, stderr) = run(
        home.path(),
        &workspace,
        &["config", "set", "workspace", "elsewhere"],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("global-only"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&config).expect("config"), original);
}

#[test]
fn explain_reports_every_layer_and_json_is_machine_readable() {
    let home = tempfile::tempdir().expect("home");
    let (_guard, workspace) = temp("ws");
    std::fs::write(
        home.path().join("config.yaml"),
        "timeout_default_ms: 1000\nproject: from-global\n",
    )
    .expect("global config");
    std::fs::write(
        workspace.join("impresario.config"),
        "timeout_default_ms: 2000\n",
    )
    .expect("workspace config");

    let (code, stdout, stderr) = run(
        home.path(),
        &workspace,
        &["config", "get", "timeout_default_ms", "--explain"],
    );
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(stdout.contains("timeout_default_ms = 2000"), "{stdout}");
    assert!(stdout.contains("source: workspace"), "{stdout}");
    assert!(stdout.contains("shadowed: global"), "{stdout}");
    assert!(stdout.contains("= 1000"), "{stdout}");
    assert!(stdout.contains("shadowed: default = 30000"), "{stdout}");

    let (code, stdout, stderr) = run(home.path(), &workspace, &["config", "list", "--json"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(json["timeout_default_ms"]["value"], 2000);
    assert!(json["timeout_default_ms"]["source"]
        .as_str()
        .expect("source")
        .contains("workspace"));
    assert_eq!(json["project"]["value"], "from-global");
    assert!(json["workspaces"]["value"].is_null());

    let (code, stdout, stderr) = run(
        home.path(),
        &workspace,
        &["config", "list", "--global", "--json"],
    );
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(json["timeout_default_ms"]["value"], 1000);
    assert_eq!(json["project"]["value"], "from-global");
    assert!(json["app_reset"]["value"].is_null());
}

#[test]
fn workspace_registry_use_add_remove_list_and_new() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    let ws1 = cwd.path().join("one");
    let ws2 = cwd.path().join("two");
    let ws3 = cwd.path().join("three");
    std::fs::create_dir_all(&ws1).expect("ws1");
    std::fs::create_dir_all(&ws2).expect("ws2");

    let (code, stdout, stderr) = run_in(
        home.path(),
        cwd.path(),
        &["workspace", "use", &ws1.display().to_string()],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");

    let (code, stdout, stderr) = run_in(home.path(), cwd.path(), &["workspace", "current"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(stdout.contains(&ws1.display().to_string()), "{stdout}");
    assert!(stdout.contains("global config"), "{stdout}");

    let (code, stdout, stderr) = run_in(home.path(), cwd.path(), &["workspace", "list"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(stdout.contains(&format!("* {}", ws1.display())), "{stdout}");

    let (code, stdout, stderr) = run_in(
        home.path(),
        cwd.path(),
        &["workspace", "add", &ws2.display().to_string()],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    let (code, stdout, _stderr) = run_in(home.path(), cwd.path(), &["workspace", "list"]);
    assert_eq!(code, EXIT_OK);
    assert!(stdout.contains(&ws2.display().to_string()), "{stdout}");

    let (code, stdout, stderr) = run_in(
        home.path(),
        cwd.path(),
        &["workspace", "remove", &ws2.display().to_string()],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    let (code, stdout, _stderr) = run_in(home.path(), cwd.path(), &["workspace", "list"]);
    assert_eq!(code, EXIT_OK);
    assert!(!stdout.contains(&ws2.display().to_string()), "{stdout}");

    let (code, stdout, stderr) = run_in(
        home.path(),
        cwd.path(),
        &["workspace", "new", &ws3.display().to_string(), "--example"],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    assert!(ws3.join("impresario.config").is_file());
    assert!(ws3.join("impresario.devices.config").is_file());
    assert!(ws3.join("impresario.apps.config").is_file());
    assert!(ws3.join("plays/login.play").is_file());
    let (code, stdout, stderr) = run(home.path(), &ws3, &["validate"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
}

#[test]
fn config_path_names_the_selected_file() {
    let home = tempfile::tempdir().expect("home");
    let (_guard, workspace) = temp("ws");
    let (code, stdout, _stderr) = run(home.path(), &workspace, &["config", "path", "--workspace"]);
    assert_eq!(code, EXIT_OK);
    assert_eq!(
        PathBuf::from(stdout.trim()),
        workspace.join("impresario.config")
    );

    let (code, stdout, _stderr) = run(home.path(), &workspace, &["config", "path", "--global"]);
    assert_eq!(code, EXIT_OK);
    assert_eq!(
        PathBuf::from(stdout.trim()),
        home.path().join("config.yaml")
    );
}

#[cfg(windows)]
#[test]
fn config_edit_restores_the_old_file_when_the_result_is_invalid() {
    let home = tempfile::tempdir().expect("home");
    let (_guard, workspace) = temp("ws");
    let config = workspace.join("impresario.config");
    let original = "project: demo\n";
    std::fs::write(&config, original).expect("write config");
    let editor = home.path().join("editor.cmd");
    std::fs::write(
        &editor,
        "@echo off\r\n> \"%1\" echo timeout_default_ms: nope\r\n",
    )
    .expect("editor script");

    let output = command(home.path(), Some(&workspace))
        .env("EDITOR", format!("cmd /C {}", editor.display()))
        .env("CI", "1")
        .args(["config", "edit"])
        .output()
        .expect("run impresario");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(EXIT_USAGE), "stderr={stderr}");
    assert!(stderr.contains("IMP-CFG-003"), "{stderr}");
    assert!(stderr.contains("restored"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&config).expect("config"), original);
}
