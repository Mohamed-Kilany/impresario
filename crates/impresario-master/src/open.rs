//! Open a report or artifact with the desktop's default handler.
//!
//! This is the one deliberate exception to "no external tool shell-outs":
//! `start`, `open` and `xdg-open` are desktop UI helpers, not platform
//! development tools. The TUI already used them for report viewing; the
//! resident notification action and `impresario notify open` share the same
//! implementation instead of growing a second copy.

use std::path::Path;

/// Open `path` with the operating system's default handler.
pub fn open_external(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]).arg(path);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(path);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot open {}: {error}", path.display()))
}
