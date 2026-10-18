//! `impresario parse --json` at the CLI boundary (Phase 2 acceptance).
//!
//! Runs the real binary against the committed `examples/` workspace and
//! snapshots the stdout document. Every absolute `path`/`file` field is
//! normalized to a fixed relative string so the snapshot does not depend on
//! the machine (CI-friendly since Phase 8).

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Rewrite every `"file"` string in the document (spans nest deeply) to
/// `normalized`, so no absolute path leaks into the snapshot.
fn normalize_files(value: &mut serde_json::Value, normalized: &str) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if key == "file" && child.is_string() {
                    *child = serde_json::Value::String(normalized.to_string());
                } else {
                    normalize_files(child, normalized);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                normalize_files(item, normalized);
            }
        }
        _ => {}
    }
}

fn parse_json(file: &str, normalized_path: &str) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_impresario"))
        .current_dir(repo_root())
        .args(["--workspace", "examples", "parse", file, "--json"])
        .output()
        .expect("run impresario");
    assert!(
        output.status.success(),
        "parse --json failed for {file}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is a JSON document");
    value["path"] = serde_json::Value::String(normalized_path.to_string());
    normalize_files(&mut value, normalized_path);
    value
}

#[test]
fn parse_json_play_matches_the_snapshot() {
    let value = parse_json("plays/login.play", "examples/plays/login.play");
    insta::assert_json_snapshot!("parse_json_login_play", value);
}

#[test]
fn parse_json_scene_matches_the_snapshot() {
    let value = parse_json(
        "scenes/LoginScreen.scene",
        "examples/scenes/LoginScreen.scene",
    );
    insta::assert_json_snapshot!("parse_json_login_scene", value);
}

#[test]
fn parse_reports_broken_files_on_stderr_with_exit_2() {
    let dir = repo_root().join("target").join("tmp-parse-broken");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("broken.play");
    std::fs::write(
        &file,
        "Play: Broken\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Action: tap => Nope\n",
    )
    .expect("write");
    let output = Command::new(env!("CARGO_BIN_EXE_impresario"))
        .current_dir(repo_root())
        .args(["--no-color", "parse"])
        .arg(&file)
        .output()
        .expect("run impresario");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("IMP-DSL-010"), "{stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.is_empty(), "stdout: {stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}
