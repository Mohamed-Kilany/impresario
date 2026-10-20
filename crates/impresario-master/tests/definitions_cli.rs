//! `device`, `app` and `migrate` integration tests (TASK-2 4.3, 5 and 8.9).
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
            "device",
            "add",
            "--name",
            "phone",
            "--platform",
            "android",
            "--transport",
            "usb",
            "--serial",
            "emulator-5554",
            "--label",
            "phone",
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
            "--platform",
            "webos",
            "--transport",
            "ssh",
            "--host",
            "192.168.1.50",
            "--endpoint",
            "ws://192.168.1.50:9998",
            "--key-dispatch",
            "js",
            "--page-match",
            "index.html",
        ],
    );
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    let local = workspace.join("impresario.devices.config");
    assert!(local.is_file(), "workspace device file was not written");
    let text = std::fs::read_to_string(&local).expect("workspace devices");
    assert!(text.contains("key_dispatch: js"), "{text}");
    assert!(text.contains("page_match: index.html"), "{text}");

    let (code, stdout, stderr) = run(&home, &workspace, &["device", "list"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(stdout.contains("phone"), "{stdout}");
    assert!(stdout.contains("tv"), "{stdout}");

    let (code, stdout, stderr) = run(&home, &workspace, &["device", "show", "tv"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(stdout.contains("ws://192.168.1.50:9998"), "{stdout}");

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
        "device",
        "add",
        "--name",
        "phone",
        "--platform",
        "android",
        "--transport",
        "usb",
        "--serial",
        "emulator-5554",
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
            "device",
            "add",
            "--name",
            "bad name",
            "--platform",
            "android",
            "--serial",
            "x",
        ],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("IMP-CFG"), "{stderr}");

    let (code, _stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "device",
            "add",
            "--name",
            "wat",
            "--platform",
            "webos",
            "--transport",
            "usb",
        ],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("IMP-CFG-003"), "{stderr}");
}

#[test]
fn apple_only_options_are_rejected_elsewhere() {
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
            "--platform",
            "ios",
            "--udid",
            "00008110-001A2345021E001E",
            "--animations-disabled",
        ],
    );
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    let text = std::fs::read_to_string(workspace.join("impresario.devices.config"))
        .expect("workspace devices");
    assert!(text.contains("animations_disabled: true"), "{text}");

    let (code, _stdout, stderr) = run(
        &home,
        &workspace,
        &[
            "device",
            "add",
            "--workspace",
            "--name",
            "tv",
            "--platform",
            "webos",
            "--host",
            "192.168.1.50",
            "--animations-disabled",
        ],
    );
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("IMP-CFG-003"), "{stderr}");
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

#[test]
fn migrate_is_dry_runnable_idempotent_and_backs_up() {
    let (_guard, home, workspace) = temp();
    let path = workspace.join("impresario.devices.config");
    let legacy = "devices:\n  - name: phone\n    platform: android\n    target_endpoint: emulator-5554\n    app_id: com.example.shop\n";
    std::fs::write(&path, legacy).expect("legacy devices");

    let (code, stdout, stderr) = run(&home, &workspace, &["migrate", "--dry-run"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("unchanged"),
        legacy,
        "dry-run must not write"
    );
    assert!(stdout.contains("apps:"), "{stdout}");
    assert!(stdout.contains("impresario.apps.config"), "{stdout}");

    let (code, stdout, stderr) = run(&home, &workspace, &["migrate"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    let backup = std::fs::read_to_string(format!("{}.bak", path.display())).expect("backup");
    assert_eq!(backup, legacy);
    let devices = std::fs::read_to_string(&path).expect("devices");
    assert!(devices.contains("serial: emulator-5554"), "{devices}");
    assert!(!devices.contains("app_id"), "{devices}");
    let apps = std::fs::read_to_string(workspace.join("impresario.apps.config")).expect("apps");
    assert!(apps.contains("com.example.shop"), "{apps}");
    assert!(apps.contains("name: phone_app"), "{apps}");

    let (code, stdout, stderr) = run(&home, &workspace, &["migrate"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("already"), "{stdout}");
}

#[test]
fn migrate_converts_legacy_dsl_and_is_idempotent() {
    let (_guard, home, workspace) = temp();
    let scenes = workspace.join("scenes");
    let plays = workspace.join("plays");
    std::fs::create_dir_all(&scenes).expect("scenes");
    std::fs::create_dir_all(&plays).expect("plays");
    let scene_path = scenes.join("LoginScreen.scene");
    let play_path = plays.join("login.play");
    let legacy_scene =
        "Scene: LoginScreen\nPlatform: android\n\nPieces:\n    Title: android=\"id:title\"\n";
    let legacy_play =
        "Play: Login\n\nAct: A\n\nStage: Fill\n    Scene: LoginScreen\n        Check: Title\n";
    std::fs::write(&scene_path, legacy_scene).expect("scene");
    std::fs::write(&play_path, legacy_play).expect("play");

    let (code, stdout, stderr) = run(&home, &workspace, &["migrate", "--dry-run"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("| Piece"), "{stdout}");
    assert!(stdout.contains("Perform: Fill"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(&scene_path).expect("unchanged"),
        legacy_scene
    );

    let (code, stdout, stderr) = run(&home, &workspace, &["migrate"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("converted"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(format!("{}.bak", scene_path.display())).expect("backup"),
        legacy_scene
    );
    let migrated_play = std::fs::read_to_string(&play_path).expect("play");
    assert!(migrated_play.contains("Perform: Fill"), "{migrated_play}");
    let theatre = std::fs::read_to_string(workspace.join("theatres/login.theatre"))
        .expect("generated theatre");
    assert!(theatre.contains("Stage: Fill"), "{theatre}");

    let (code, stdout, stderr) = run(&home, &workspace, &["migrate"]);
    assert_eq!(code, EXIT_OK, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("already"), "{stdout}");
}

#[test]
fn migrate_refuses_mixed_legacy_and_new_entries() {
    let (_guard, home, workspace) = temp();
    let path = workspace.join("impresario.devices.config");
    std::fs::write(
        &path,
        "devices:\n  - name: phone\n    platform: android\n    connection:\n      transport: usb\n      serial: emulator-5554\n  - name: old\n    platform: android\n    target_endpoint: emulator-5556\n",
    )
    .expect("mixed devices");
    let (code, _stdout, stderr) = run(&home, &workspace, &["migrate"]);
    assert_eq!(code, EXIT_USAGE);
    assert!(stderr.contains("legacy"), "{stderr}");
}

#[test]
fn migrate_without_apps_does_not_create_an_apps_file() {
    let (_guard, home, workspace) = temp();
    let path = workspace.join("impresario.devices.config");
    std::fs::write(
        &path,
        "devices:\n  - name: phone\n    platform: android\n    target_endpoint: emulator-5554\n    options:\n      clear_app_data: true\n",
    )
    .expect("legacy devices");
    let (code, _stdout, stderr) = run(&home, &workspace, &["migrate"]);
    assert_eq!(code, EXIT_OK, "stderr={stderr}");
    assert!(!workspace.join("impresario.apps.config").exists());
    assert!(stderr.contains("clear_app_data"), "{stderr}");
}
