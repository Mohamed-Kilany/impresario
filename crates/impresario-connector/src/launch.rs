//! Chromium launch for `web` devices with `target_endpoint: launch[:headless]`
//! (spec 10.3): spawn `CHROME_BIN` or a PATH lookup with
//! `--remote-debugging-port=0 --user-data-dir=<tempdir>` (+ `--headless=new`),
//! read `DevTools listening on ws://...` from stderr, and kill the process
//! when the session closes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::errors::ConnectorError;

/// How long Chromium may take to print the DevTools endpoint.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

/// A running Chromium owned by one session.
#[derive(Debug)]
pub struct LaunchedBrowser {
    /// The browser-level websocket URL (page resolution runs against its
    /// host:port).
    pub ws_url: String,
    pub host: String,
    pub port: u16,
    child: Child,
    profile: tempfile::TempDir,
}

impl LaunchedBrowser {
    /// Kill the browser (idempotent) and drop its temporary profile.
    pub async fn kill(&mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }

    /// Whether the process is still running.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Path of the temporary profile (tests and diagnostics).
    #[must_use]
    pub fn profile_path(&self) -> &Path {
        self.profile.path()
    }
}

/// Launch Chromium and wait for its DevTools endpoint (opening `about:blank`).
pub async fn launch_chromium(
    headless: bool,
    extra_args: &[String],
) -> Result<LaunchedBrowser, ConnectorError> {
    launch_chromium_at(headless, "about:blank", extra_args).await
}

/// Launch Chromium opening `start_url` (the connector opens `about:blank`;
/// tests open a fixture page directly, because headless Chromium rejects
/// several startup URLs).
pub async fn launch_chromium_at(
    headless: bool,
    start_url: &str,
    extra_args: &[String],
) -> Result<LaunchedBrowser, ConnectorError> {
    let binary = find_chromium().ok_or_else(|| {
        ConnectorError::device_unavailable(
            "no Chromium found: set CHROME_BIN or install google-chrome/chromium on PATH"
                .to_string(),
        )
    })?;
    let profile = tempfile::Builder::new()
        .prefix("impresario-chrome-")
        .tempdir()
        .map_err(|error| {
            ConnectorError::device_unavailable(format!(
                "cannot create a Chromium profile directory: {error}"
            ))
        })?;

    let mut command = Command::new(&binary);
    command
        .arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        // Decision I-37: keep the renderer active. Headless/occluded windows
        // otherwise stop producing BeginFrames, so `scroll` events and
        // rendering-dependent probes stall until the page is activated.
        .arg("--disable-background-timer-throttling")
        .arg("--disable-backgrounding-occluded-windows")
        .arg("--disable-renderer-backgrounding")
        .arg(start_url);
    if headless {
        command.arg("--headless=new");
    }
    command.args(extra_args);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|error| {
        ConnectorError::device_unavailable(format!(
            "cannot start Chromium ({}) : {error}",
            binary.display()
        ))
    })?;

    let Some(stderr) = child.stderr.take() else {
        let _ = child.start_kill();
        return Err(ConnectorError::device_unavailable(
            "Chromium stderr is unavailable; cannot read the DevTools endpoint".to_string(),
        ));
    };

    let mut lines = BufReader::new(stderr).lines();
    let mut endpoint: Option<String> = None;
    let mut tail: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + LAUNCH_TIMEOUT;
    while endpoint.is_none() {
        let next = tokio::time::timeout_at(deadline, lines.next_line()).await;
        match next {
            Ok(Ok(Some(line))) => {
                if let Some(ws) = devtools_endpoint(&line) {
                    endpoint = Some(ws);
                } else {
                    tracing::debug!(target: "impresario_connector::chromium", "{line}");
                    if tail.len() >= 20 {
                        tail.remove(0);
                    }
                    tail.push(line);
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                let _ = child.start_kill();
                return Err(ConnectorError::device_unavailable(format!(
                    "reading Chromium stderr failed: {error}"
                )));
            }
            Err(_) => break,
        }
    }

    let Some(ws_url) = endpoint else {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(ConnectorError::device_unavailable(format!(
            "Chromium did not print a DevTools endpoint within {} s; last output: {}",
            LAUNCH_TIMEOUT.as_secs(),
            tail.join(" | ")
        )));
    };

    let (host, port) = match split_ws_endpoint(&ws_url) {
        Some(parts) => parts,
        None => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(ConnectorError::device_unavailable(format!(
                "Chromium printed an unusable DevTools endpoint: {ws_url}"
            )));
        }
    };

    // Keep draining stderr so the pipe never fills up.
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(target: "impresario_connector::chromium", "{line}");
        }
    });

    Ok(LaunchedBrowser {
        ws_url,
        host,
        port,
        child,
        profile,
    })
}

/// `DevTools listening on ws://...` on stderr.
#[must_use]
pub fn devtools_endpoint(line: &str) -> Option<String> {
    let marker = "DevTools listening on ";
    let index = line.find(marker)?;
    let rest = line.get(index + marker.len()..)?.trim();
    let ws = rest.split_whitespace().next()?;
    if ws.starts_with("ws://") || ws.starts_with("wss://") {
        Some(ws.to_string())
    } else {
        None
    }
}

/// `(host, port)` from a `ws://host:port/...` URL.
#[must_use]
pub fn split_ws_endpoint(ws_url: &str) -> Option<(String, u16)> {
    let url = url::Url::parse(ws_url).ok()?;
    let host = url.host_str()?.to_string();
    let port = url.port()?;
    Some((host, port))
}

/// `CHROME_BIN`, then PATH, then well-known install locations
/// (decision I-35: Windows/macOS installers do not add Chrome to PATH).
#[must_use]
pub fn find_chromium() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CHROME_BIN") {
        if !path.is_empty() {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Some(path);
            }
            tracing::warn!(path = %path.display(), "CHROME_BIN does not point at a file; continuing the search");
        }
    }
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
    ] {
        if let Some(found) = find_in_path(name) {
            return Some(found);
        }
    }
    for candidate in WELL_KNOWN_BINARIES {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

#[cfg(windows)]
const WELL_KNOWN_BINARIES: &[&str] = &[
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
];

#[cfg(target_os = "macos")]
const WELL_KNOWN_BINARIES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
];

#[cfg(all(unix, not(target_os = "macos")))]
const WELL_KNOWN_BINARIES: &[&str] = &[
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/snap/bin/chromium",
];

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devtools_line_extraction() {
        let line = "[1017/120000.123:INFO:CONSOLE(0)] DevTools listening on ws://127.0.0.1:54213/devtools/browser/abc";
        assert_eq!(
            devtools_endpoint(line).as_deref(),
            Some("ws://127.0.0.1:54213/devtools/browser/abc")
        );
        assert_eq!(devtools_endpoint("[123:ERROR:foo] noise"), None);
        assert_eq!(devtools_endpoint("DevTools listening on not-a-url"), None);
    }

    #[test]
    fn endpoint_split() {
        assert_eq!(
            split_ws_endpoint("ws://127.0.0.1:54213/devtools/browser/abc"),
            Some(("127.0.0.1".to_string(), 54213))
        );
        assert_eq!(split_ws_endpoint("ws://127.0.0.1/devtools/browser"), None);
    }

    #[test]
    fn chrome_bin_env_wins() {
        // The lookup order is CHROME_BIN -> PATH -> well-known paths; only the
        // first step is unit-testable without touching the host.
        let dir = tempfile::tempdir().expect("tempdir");
        let fake = dir.path().join("chrome");
        std::fs::write(&fake, b"#!/bin/sh\n").expect("write");
        std::env::set_var("CHROME_BIN", &fake);
        assert_eq!(find_chromium(), Some(fake.clone()));
        std::env::remove_var("CHROME_BIN");
    }
}
