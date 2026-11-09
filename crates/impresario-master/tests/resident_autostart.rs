//! Resident autostart: Linux systemd enablement and XDG fallback.
//!
//! These tests run the real CLI on Linux. They force the systemd/XDG choice
//! through `XDG_RUNTIME_DIR` and keep every hook under a temporary
//! `XDG_CONFIG_HOME`, so no user configuration is touched.

#![cfg(all(unix, not(target_os = "macos")))]

use std::path::Path;

use assert_cmd::Command;

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(home: &Path, config_home: &Path, runtime: &Path, workspace: &Path, args: &[&str]) -> Output {
    let output = Command::cargo_bin("impresario")
        .expect("binary")
        .env("IMPRESARIO_HOME", home)
        .env("XDG_CONFIG_HOME", config_home)
        .env("XDG_RUNTIME_DIR", runtime)
        .arg("--workspace")
        .arg(workspace)
        .args(args)
        .output()
        .expect("impresario runs");
    Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn temp_workspace(base: &Path) -> std::path::PathBuf {
    let workspace = base.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    workspace
}

#[test]
fn systemd_install_enables_the_unit_and_uninstall_removes_it() {
    let base = tempfile::tempdir().expect("base");
    let home = base.path().join("home");
    let config_home = base.path().join("config");
    let runtime = base.path().join("runtime");
    std::fs::create_dir_all(runtime.join("systemd")).expect("runtime");
    let workspace = temp_workspace(base.path());

    let installed = run(
        &home,
        &config_home,
        &runtime,
        &workspace,
        &["service", "install"],
    );
    assert_eq!(installed.code, 0, "stderr={}", installed.stderr);
    let unit = config_home.join("systemd/user/impresario.service");
    let wants = config_home.join("systemd/user/default.target.wants/impresario.service");
    assert!(unit.is_file(), "stdout={}", installed.stdout);
    assert_eq!(
        std::fs::read_link(&wants).expect("wants link"),
        std::path::absolute(&unit).expect("absolute unit")
    );
    let text = std::fs::read_to_string(&unit).expect("unit");
    assert!(text.contains("ExecStart="), "{text}");
    assert!(text.contains("--workspace"), "{text}");
    assert!(
        text.contains(&workspace.display().to_string()),
        "the hook must name the workspace: {text}"
    );
    assert!(text.contains("WantedBy=default.target"), "{text}");
    assert!(!config_home.join("autostart/impresario.desktop").exists());

    let again = run(
        &home,
        &config_home,
        &runtime,
        &workspace,
        &["service", "install"],
    );
    assert_eq!(again.code, 0, "stderr={}", again.stderr);
    assert_eq!(
        std::fs::read_dir(wants.parent().expect("wants parent"))
            .expect("wants dir")
            .count(),
        1
    );

    let removed = run(
        &home,
        &config_home,
        &runtime,
        &workspace,
        &["service", "uninstall"],
    );
    assert_eq!(removed.code, 0, "stderr={}", removed.stderr);
    assert!(!unit.exists());
    assert!(!wants.exists());
    assert!(removed.stdout.contains("removed autostart:"));
}

#[test]
fn xdg_fallback_is_used_without_a_systemd_user_manager() {
    let base = tempfile::tempdir().expect("base");
    let home = base.path().join("home");
    let config_home = base.path().join("config");
    let runtime = base.path().join("runtime");
    std::fs::create_dir_all(&runtime).expect("runtime");
    let workspace = temp_workspace(base.path());

    let installed = run(
        &home,
        &config_home,
        &runtime,
        &workspace,
        &["service", "install"],
    );
    assert_eq!(installed.code, 0, "stderr={}", installed.stderr);
    let desktop = config_home.join("autostart/impresario.desktop");
    assert!(desktop.is_file(), "stdout={}", installed.stdout);
    assert!(!config_home.join("systemd/user/impresario.service").exists());
    let text = std::fs::read_to_string(&desktop).expect("desktop entry");
    assert!(text.contains("Type=Application"), "{text}");
    assert!(text.contains("--workspace"), "{text}");
    assert!(
        text.contains(&workspace.display().to_string()),
        "the fallback must name the workspace: {text}"
    );
    assert!(text.contains("X-GNOME-Autostart-enabled=true"), "{text}");

    let removed = run(
        &home,
        &config_home,
        &runtime,
        &workspace,
        &["service", "uninstall"],
    );
    assert_eq!(removed.code, 0, "stderr={}", removed.stderr);
    assert!(!desktop.exists());
    assert!(removed.stdout.contains("removed autostart:"));
}
