//! `impresario fmt` integration tests (TASK-2 8.6): rewrite, `--check`,
//! exit codes and idempotence through the real binary.

use std::path::Path;

use assert_cmd::Command;

const EXIT_OK: i32 = 0;
const EXIT_FAILURE: i32 = 1;
const EXIT_USAGE: i32 = 2;

fn run(home: &Path, workspace: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::cargo_bin("impresario")
        .expect("binary")
        .env("IMPRESARIO_HOME", home)
        .env_remove("IMPRESARIO_WORKSPACE")
        .arg("--workspace")
        .arg(workspace)
        .args(args)
        .output()
        .expect("run impresario");
    (
        output.status.code().expect("exit code"),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn fmt_check_reports_then_rewrites_idempotently() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    let workspace = dir.path().join("ws");
    let scenes = workspace.join("scenes");
    std::fs::create_dir_all(&scenes).expect("scenes");
    let path = scenes.join("LoginScreen.scene");
    let unformatted = "Scene: LoginScreen\nPlatforms: cross-platform\n\n| Piece | web | android |\n|---|---|---|\n| Title | h1 | id:title |\n| Missing | #m | - |\n";
    std::fs::write(&path, unformatted).expect("scene");

    let (code, stdout, stderr) = run(&home, &workspace, &["fmt", "--check"]);
    assert_eq!(code, EXIT_FAILURE, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("would reformat"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("unchanged"),
        unformatted
    );

    let (code, stdout, stderr) = run(&home, &workspace, &["fmt"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    let formatted = std::fs::read_to_string(&path).expect("formatted");
    assert!(
        formatted.contains("| Piece   | web | android  |"),
        "{formatted}"
    );
    assert!(
        formatted.contains("| Missing | #m  | -        |"),
        "{formatted}"
    );

    let (code, stdout, stderr) = run(&home, &workspace, &["fmt", "--check"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");

    std::fs::write(&path, "Scene: Broken\n").expect("broken");
    let (code, _stdout, stderr) = run(&home, &workspace, &["fmt"]);
    assert_eq!(code, EXIT_USAGE, "stderr={stderr}");
    assert!(stderr.contains("IMP-DSL"), "{stderr}");
}
