//! `doctor` must keep probing after a broken workspace or value file.
//!
//! This is the regression for the "doctor is diagnostic even when normal
//! startup is broken" contract: the configuration diagnostics are reported,
//! but the tool/platform probes still run and the JSON shape stays complete.

use assert_cmd::Command;
use serde_json::Value;

#[test]
fn doctor_reports_broken_config_and_still_probes() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("impresario.config"),
        "parallel_devices: not-a-number\n",
    )
    .expect("write config");
    std::fs::write(
        workspace.path().join("impresario.devices.config"),
        ": bad\n",
    )
    .expect("write devices");

    let output = Command::cargo_bin("impresario")
        .expect("binary")
        .env("IMPRESARIO_HOME", home.path())
        .arg("--workspace")
        .arg(workspace.path())
        .args(["doctor", "--json"])
        .output()
        .expect("run doctor");

    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("doctor JSON");
    let config_errors = report["config_errors"]
        .as_array()
        .expect("config_errors array");
    assert!(!config_errors.is_empty(), "config errors are reported");
    let device_errors = report["device_errors"]
        .as_array()
        .expect("device_errors array");
    assert!(!device_errors.is_empty(), "device errors are reported");
    assert!(
        report["platforms"]
            .as_array()
            .is_some_and(|platforms| !platforms.is_empty()),
        "platform probes still ran"
    );
    assert!(
        report["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()),
        "tool probes still ran"
    );
    assert_eq!(report["ok"], false);
}
