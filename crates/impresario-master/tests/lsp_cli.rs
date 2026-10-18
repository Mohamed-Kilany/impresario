//! The `lsp` subcommand end-to-end: the real binary on real stdio (spec 13).

use std::process::{Command, Stdio};

#[test]
fn lsp_serves_on_stdio_and_exits_when_stdin_closes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path();
    std::fs::write(workspace.join("impresario.config"), "project: lsp\n").expect("config");

    let output = Command::new(env!("CARGO_BIN_EXE_impresario"))
        .arg("--workspace")
        .arg(workspace)
        .arg("lsp")
        .stdin(Stdio::null())
        .output()
        .expect("child runs");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
