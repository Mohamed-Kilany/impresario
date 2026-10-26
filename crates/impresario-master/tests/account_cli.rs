//! The machine-global Apple account command must work without a workspace and
//! must never touch the network for validation-only paths.

use std::path::Path;

use assert_cmd::Command;

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 2;

fn run(home: &Path, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::cargo_bin("impresario")
        .expect("binary")
        .env("IMPRESARIO_HOME", home)
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("impresario runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn account_status_without_a_session_is_workspace_independent() {
    let home = tempfile::tempdir().expect("home");
    let empty = tempfile::tempdir().expect("cwd");
    let (code, stdout, stderr) = run(home.path(), empty.path(), &["account", "status", "--json"]);
    assert_eq!(code, EXIT_OK, "stderr: {stderr}");
    assert_eq!(stdout.trim(), r#"{"logged_in":false}"#);
    assert!(
        !stdout.contains("token"),
        "status must not print token material"
    );
}

#[test]
fn account_logout_without_a_session_is_a_no_op() {
    let home = tempfile::tempdir().expect("home");
    let empty = tempfile::tempdir().expect("cwd");
    let (code, stdout, stderr) = run(home.path(), empty.path(), &["account", "logout", "--yes"]);
    assert_eq!(code, EXIT_OK, "stderr: {stderr}");
    assert!(stdout.contains("nothing to remove"));
}

#[test]
fn account_login_rejects_an_invalid_email_before_any_prompt() {
    let home = tempfile::tempdir().expect("home");
    let empty = tempfile::tempdir().expect("cwd");
    let (code, _stdout, stderr) = run(
        home.path(),
        empty.path(),
        &["account", "login", "--email", "not-an-email"],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("does not look valid"));
}

#[test]
fn account_help_lists_the_global_wizard() {
    let home = tempfile::tempdir().expect("home");
    let empty = tempfile::tempdir().expect("cwd");
    let (code, stdout, _stderr) = run(home.path(), empty.path(), &["account", "--help"]);
    assert_eq!(code, EXIT_OK);
    assert!(stdout.contains("login"));
    assert!(stdout.contains("status"));
    assert!(stdout.contains("logout"));
}
