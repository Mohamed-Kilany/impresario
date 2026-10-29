//! `device` and `app` integration tests (TASK-2 4.3 and 5).
//!
//! Each test gets its own global home via `IMPRESARIO_HOME`; workspace scope is
//! selected with the subcommand-level `--workspace` flag.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 2;

fn command(home: &Path, workspace: &Path) -> Command {
    let mut command = Command::cargo_bin("impresario").expect("binary");
    command
        .env("IMPRESARIO_HOME", home)
        .env_remove("IMPRESARIO_WORKSPACE")
        .arg("--workspace")
        .arg(workspace);
    command
}

fn run(home: &Path, workspace: &Path, args: &[&str]) -> (i32, String, String) {
    let output = command(home, workspace)
        .args(args)
        .output()
        .expect("run impresario");
    (
        output.status.code().expect("exit code"),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn temp() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&workspace).expect("workspace");
    (dir, home, workspace)
}

#[test]
fn device_lifecycle_merges_global_and_workspace_layers() {
    let (_guard, home, workspace) = temp();
    let (code, stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "device", "add", "--name", "phone", "--type", "android", "--ip", "10.0.0.5",
        ],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    let global = home.join("devices.yaml");
    assert!(global.is_file(), "global device file was not written");

    let (code, stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "device",
            "add",
            "--workspace",
            "--name",
            "tv",
            "--type",
            "webOS",
            "--ip",
            "192.168.1.50",
        ],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    let local = workspace.join("impresario.devices.config");
    assert!(local.is_file(), "workspace device file was not written");
    let text = std::fs::read_to_string(&local).expect("workspace devices");
    assert!(text.contains("type: webOS"), "{text}");
    assert!(text.contains("ip: 192.168.1.50"), "{text}");
    assert!(!text.contains("connection:"), "{text}");

    let (code, stdout, stderr) = run(&home, &workspace, &["device", "list"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(stdout.contains("phone"), "{stdout}");
    assert!(stdout.contains("tv"), "{stdout}");

    let (code, stdout, stderr) = run(&home, &workspace, &["device", "show", "tv"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(stdout.contains("192.168.1.50"), "{stdout}");
    assert!(stdout.contains("webOS"), "{stdout}");

    let (code, _stdout, stderr) = run(
        &home,
        &workspace,
        &["device", "rename", "tv", "living_room", "--workspace"],
    );
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(std::fs::read_to_string(&local)
        .expect("workspace devices")
        .contains("living_room"));

    let (code, _stdout, stderr) = run(&home, &workspace, &["device", "remove", "phone"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(!std::fs::read_to_string(&global)
        .expect("global devices")
        .contains("phone"));
}

#[test]
fn duplicate_devices_and_bad_fields_are_rejected() {
    let (_guard, home, workspace) = temp();
    let args = [
        "device", "add", "--name", "phone", "--type", "android", "--ip", "10.0.0.5",
    ];
    let (code, _stdout, stderr) = run(&home, &workspace, &args);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    let (code, _stdout, stderr) = run(&home, &workspace, &args);
    assert_eq!(code, EXIT_USAGE, "duplicate must fail");
    assert!(stderr.contains("already exists"), "{stderr}");

    let (code, _stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "device", "add", "--name", "bad name", "--type", "android", "--ip", "10.0.0.6",
        ],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("IMP-CFG"), "{stderr}");

    let (code, _stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "device", "add", "--name", "wat", "--type", "bogus", "--ip", "10.0.0.7",
        ],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("unknown type"), "{stderr}");
}

#[test]
fn exact_types_and_ip_validation() {
    let (_guard, home, workspace) = temp();
    let (code, _stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "device",
            "add",
            "--workspace",
            "--name",
            "iphone",
            "--type",
            "iOS",
            "--ip",
            "192.168.1.70",
        ],
    );
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    let text = std::fs::read_to_string(workspace.join("impresario.devices.config"))
        .expect("workspace devices");
    assert!(text.contains("type: iOS"), "{text}");
    assert!(text.contains("ip: 192.168.1.70"), "{text}");

    let (code, _stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "device",
            "add",
            "--workspace",
            "--name",
            "tv",
            "--type",
            "webOS",
            "--ip",
            "not-an-ip",
        ],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("not an IPv4 or IPv6 address"), "{stderr}");
}

#[test]
fn app_lifecycle_and_check() {
    let (_guard, home, workspace) = temp();
    let (code, stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "app",
            "add",
            "--workspace",
            "--name",
            "shop",
            "--platform",
            "android=com.example.shop",
            "--platform",
            "web=http://localhost:8080/index.html",
            "--launch-activity",
            "android=.MainActivity",
            "--launch-param",
            "channel=beta",
            "--clear-data",
        ],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    let file = workspace.join("impresario.apps.config");
    let text = std::fs::read_to_string(&file).expect("apps file");
    assert!(text.contains("com.example.shop"), "{text}");
    assert!(text.contains("launch_activity: .MainActivity"), "{text}");
    assert!(text.contains("channel: beta"), "{text}");
    assert!(text.contains("clear_data: true"), "{text}");

    let (code, stdout, stderr) = run(&home, &workspace, &["app", "list"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(stdout.contains("shop"), "{stdout}");
    assert!(stdout.contains("android"), "{stdout}");

    let (code, stdout, stderr) = run(&home, &workspace, &["app", "check"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");

    let (code, _stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "app",
            "add",
            "--workspace",
            "--name",
            "broken",
            "--platform",
            "android=com.example.broken",
            "--package",
            "android=./does-not-exist.apk",
        ],
    );
    // Adding is allowed: the artifact may be built later.
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    let (code, stdout, stderr) = run(&home, &workspace, &["app", "check"]);
    assert_eq!(code, EXIT_USAGE, "missing package must fail");
    assert!(
        stderr.contains("IMP") || stdout.contains("IMP"),
        "{stdout}{stderr}"
    );
}
