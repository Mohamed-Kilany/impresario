//! Embedded web console (`impresario web`, decision I-192).
//!
//! The console is a **client of the same CLI**, like the TUI. Reads reuse the
//! shared library functions or the CLI's JSON output; every mutation spawns
//! the current executable with the same workspace/config/device flags the TUI
//! uses. Validation, exit codes, redaction, device locks and resident locks
//! therefore stay single-sourced.
//!
//! Transport is a small, hand-written HTTP/1.1 server over
//! [`std::net::TcpListener`] with one thread per connection and a hard
//! connection cap. There is no HTTP framework and no external asset: the
//! single-page console, stylesheet, script and favicon are compiled into the
//! binary.
//!
//! Security defaults:
//! - bind `127.0.0.1:7810`;
//! - a non-loopback bind requires `--allow-remote` and is documented as
//!   trusted-network-only (no TLS, no authentication);
//! - every request must carry an allowed `Host` (loopback names in the
//!   default mode), which blocks DNS-rebinding reads;
//! - mutations require `Content-Type: application/json`, an allowed
//!   `Origin`/`Host` pair and a curated action name; arbitrary commands are
//!   never accepted.

mod actions;
mod api;
mod assets;
pub(crate) mod http;
mod resources;

use std::collections::VecDeque;
use std::io::Write as _;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use clap::Args;
use impresario_core::home::Home;
use serde::Serialize;
use serde_json::{json, Value};

/// Default loopback port of the console.
pub const DEFAULT_PORT: u16 = 7810;
/// Default bind address.
pub const DEFAULT_BIND: &str = "127.0.0.1";
/// Header and body limits (64 KiB / 2 MiB).
pub(crate) const MAX_HEADER_BYTES: usize = 64 * 1024;
pub(crate) const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Concurrent connection cap; excess connections get a short 503.
const MAX_CONNECTIONS: usize = 64;
/// Read/write timeout for one connection.
const IO_TIMEOUT: Duration = Duration::from_secs(15);
/// How many executed actions are kept for the history endpoint.
const ACTION_HISTORY: usize = 100;

/// `impresario web`: serve the embedded console.
#[derive(Debug, Args)]
pub struct WebArgs {
    /// Port to listen on (0 = any free port, printed on start).
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,
    /// Bind address (loopback by default).
    #[arg(long, default_value = DEFAULT_BIND, value_name = "ADDR")]
    pub bind: String,
    /// Allow binding a non-loopback address (trusted networks only).
    #[arg(long)]
    pub allow_remote: bool,
    /// Open the console in the default browser after binding.
    #[arg(long)]
    pub open: bool,
}

/// One executed mutation, kept in memory for the API history.
#[derive(Debug, Clone, Serialize)]
pub struct ActionRecord {
    pub at: String,
    pub action: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub message: String,
}

/// Shared server state. The active workspace is mutable so `workspace use`
/// and `workspace new` can switch the console without a restart.
pub struct WebState {
    pub home: Home,
    base: RwLock<PathBuf>,
    config_override: Option<PathBuf>,
    devices_override: Option<PathBuf>,
    pub bind: String,
    pub port: u16,
    pub loopback: bool,
    pub allow_remote: bool,
    pub started_at: String,
    pub started: Instant,
    pub mutation: Mutex<()>,
    actions: Mutex<VecDeque<ActionRecord>>,
    connections: AtomicUsize,
}

impl WebState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        home: Home,
        base: PathBuf,
        config_path: PathBuf,
        devices_path: PathBuf,
        bind: String,
        port: u16,
        loopback: bool,
        allow_remote: bool,
    ) -> Self {
        let default_config = base.join("impresario.config");
        let default_devices = base.join("impresario.devices.config");
        let config_override = (config_path != default_config).then_some(config_path);
        let devices_override = (devices_path != default_devices).then_some(devices_path);
        Self {
            home,
            base: RwLock::new(base),
            config_override,
            devices_override,
            bind,
            port,
            loopback,
            allow_remote,
            started_at: now_rfc3339(),
            started: Instant::now(),
            mutation: Mutex::new(()),
            actions: Mutex::new(VecDeque::new()),
            connections: AtomicUsize::new(0),
        }
    }

    /// The active workspace directory.
    #[must_use]
    pub fn base(&self) -> PathBuf {
        self.base
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Switch the active workspace after the CLI accepted it.
    pub fn set_base(&self, path: PathBuf) {
        *self
            .base
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = path;
    }

    /// The effective workspace config path.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.config_override
            .clone()
            .unwrap_or_else(|| self.base().join("impresario.config"))
    }

    /// The effective workspace devices path.
    #[must_use]
    pub fn devices_path(&self) -> PathBuf {
        self.devices_override
            .clone()
            .unwrap_or_else(|| self.base().join("impresario.devices.config"))
    }

    /// The workspace apps file.
    #[must_use]
    pub fn apps_path(&self) -> PathBuf {
        self.base().join("impresario.apps.config")
    }

    /// Record one executed action.
    pub fn record_action(&self, record: ActionRecord) {
        if let Ok(mut actions) = self.actions.lock() {
            actions.push_front(record);
            actions.truncate(ACTION_HISTORY);
        }
    }

    /// Recent action history, newest first.
    #[must_use]
    pub fn action_history(&self) -> Vec<ActionRecord> {
        self.actions
            .lock()
            .map(|actions| actions.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn try_enter(&self) -> bool {
        let mut current = self.connections.load(Ordering::Relaxed);
        loop {
            if current >= MAX_CONNECTIONS {
                return false;
            }
            match self.connections.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn leave(&self) {
        self.connections.fetch_sub(1, Ordering::AcqRel);
    }

    /// True when the request's `Host` is acceptable for this bind mode.
    #[must_use]
    pub fn host_allowed(&self, host: Option<&str>) -> bool {
        if self.allow_remote && !self.loopback {
            return host.is_some_and(|value| !value.trim().is_empty());
        }
        let Some(host) = host else {
            return false;
        };
        let name = host_name(host);
        name.eq_ignore_ascii_case("localhost")
            || name == "127.0.0.1"
            || name == "::1"
            || name == "[::1]"
    }
}

fn host_name(host: &str) -> &str {
    let trimmed = host.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some((name, _)) = rest.split_once(']') {
            return name;
        }
        return trimmed;
    }
    match trimmed.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|value| value.is_ascii_digit()) => name,
        _ => trimmed,
    }
}

fn now_rfc3339() -> String {
    crate::time_fmt::rfc3339_utc(crate::time_fmt::now_millis())
}

/// `impresario web` entry point.
pub fn run(
    home: Home,
    base: PathBuf,
    config_path: PathBuf,
    devices_path: PathBuf,
    args: &WebArgs,
) -> i32 {
    let bind_ip: IpAddr = match args.bind.parse() {
        Ok(ip) => ip,
        Err(error) => {
            eprintln!("web: invalid bind address '{}': {error}", args.bind);
            return crate::cli::EXIT_USAGE;
        }
    };
    let loopback = bind_ip.is_loopback();
    if !loopback && !args.allow_remote {
        eprintln!(
            "web: {} is not a loopback address; pass --allow-remote to serve on a trusted network",
            bind_ip
        );
        return crate::cli::EXIT_USAGE;
    }
    let address = SocketAddr::new(bind_ip, args.port);
    let listener = match TcpListener::bind(address) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("web: cannot bind {address}: {error}");
            return crate::cli::EXIT_INFRA;
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        eprintln!("web: cannot configure the listener: {error}");
        return crate::cli::EXIT_INFRA;
    }
    let port = match listener.local_addr() {
        Ok(local) => local.port(),
        Err(_) => args.port,
    };
    let state = Arc::new(WebState::new(
        home,
        base.clone(),
        config_path,
        devices_path,
        args.bind.clone(),
        port,
        loopback,
        args.allow_remote,
    ));
    let host = match bind_ip {
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
        IpAddr::V6(_) => format!("[{bind_ip}]"),
        IpAddr::V4(_) => bind_ip.to_string(),
    };
    let url = format!("http://{host}:{port}/");
    println!("impresario web: listening on {url}");
    if !loopback {
        println!(
            "impresario web: non-loopback bind; trusted networks only, no TLS or authentication"
        );
    }
    eprintln!("impresario web: workspace {}", base.display());
    if args.open {
        if let Err(error) = open_url(&url) {
            eprintln!("web: cannot open the browser: {error}");
        }
    }
    serve(&listener, state, Arc::new(AtomicBool::new(false)));
    crate::cli::EXIT_OK
}

fn serve(listener: &TcpListener, state: Arc<WebState>, shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                if !state.try_enter() {
                    let mut stream = stream;
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                    let response = http::Response::error(
                        503,
                        "busy",
                        "the console has too many open connections",
                    );
                    let _ = http::write_response(&mut stream, &response, false);
                    continue;
                }
                let connection_state = Arc::clone(&state);
                let spawned = std::thread::Builder::new()
                    .name("impresario-web".to_string())
                    .spawn(move || {
                        handle_connection(stream, &connection_state);
                        connection_state.leave();
                    });
                if spawned.is_err() {
                    state.leave();
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                eprintln!("web: accept failed: {error}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn handle_connection(mut stream: TcpStream, state: &WebState) {
    // A Windows listener's non-blocking flag is inherited by accepted
    // sockets, so a body sent in a second packet would fail with WouldBlock.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let head_only;
    let response = match http::read_request(&mut stream) {
        Ok(request) => {
            head_only = request.method == "HEAD";
            api::handle(state, &request)
        }
        Err(error) => {
            head_only = false;
            error.response()
        }
    };
    let _ = http::write_response(&mut stream, &response, head_only);
    let _ = stream.flush();
}

/// Open a URL in the platform browser (unlike `open::open_external`, a URL
/// does not have to exist on disk).
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]).arg(url);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Output of one CLI child.
#[derive(Debug, Clone)]
pub(crate) struct CliOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    /// The most useful human message: stderr, else stdout, else the code.
    pub(crate) fn message(&self) -> String {
        let stderr = self.stderr.trim();
        if !stderr.is_empty() {
            return stderr.to_string();
        }
        let stdout = self.stdout.trim();
        if !stdout.is_empty() {
            return stdout.to_string();
        }
        format!("command exited with {}", self.code)
    }

    /// The parsed stdout, when it is JSON.
    pub(crate) fn json(&self) -> Result<Value, String> {
        serde_json::from_str(self.stdout.trim())
            .map_err(|error| format!("the command did not return JSON: {error}"))
    }
}

/// Spawn the current executable as a CLI client of `state`'s workspace.
pub(crate) fn run_cli(state: &WebState, args: &[String]) -> Result<CliOutput, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate the impresario executable: {error}"))?;
    let mut command = std::process::Command::new(executable);
    command
        .arg("--workspace")
        .arg(state.base())
        .arg("--config")
        .arg(state.config_path())
        .arg("--devices-config")
        .arg(state.devices_path())
        .arg("--no-tui")
        .arg("--plain")
        .arg("--no-color");
    command.args(args);
    command.env("IMPRESARIO_NO_PATH_INSTALL", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot run impresario {}: {error}", args.join(" ")))?;
    Ok(CliOutput {
        code: output.status.code().unwrap_or(crate::cli::EXIT_INTERRUPTED),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// A curated action failure.
#[derive(Debug)]
pub(crate) struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub details: Option<Value>,
}

impl ApiError {
    pub(crate) fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }

    pub(crate) fn bad(message: impl Into<String>) -> Self {
        Self::new(400, "invalid_parameter", message)
    }

    pub(crate) fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub(crate) fn response(self) -> http::Response {
        let mut error = serde_json::Map::new();
        error.insert("code".to_string(), json!(self.code));
        error.insert("message".to_string(), json!(self.message));
        if let Some(details) = self.details {
            error.insert("details".to_string(), details);
        }
        http::Response::json(
            self.status,
            json!({
                "ok": false,
                "error": Value::Object(error),
            }),
        )
    }
}

/// Build the `{"ok":true,"data":...}` envelope.
pub(crate) fn ok(data: Value) -> http::Response {
    http::Response::json(200, json!({ "ok": true, "data": data }))
}

/// Resolve a user-supplied path against the workspace and confine it.
pub(crate) fn workspace_file(state: &WebState, raw: &str) -> Result<PathBuf, ApiError> {
    if raw.trim().is_empty() {
        return Err(ApiError::bad("a file path is required"));
    }
    if raw.contains('\0') {
        return Err(ApiError::bad("the path contains a NUL byte"));
    }
    let base = state.base();
    let candidate = Path::new(raw);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base.join(candidate)
    };
    let base = base
        .canonicalize()
        .map_err(|error| ApiError::new(500, "workspace_unavailable", error.to_string()))?;
    let resolved = candidate.canonicalize().map_err(|error| {
        ApiError::new(
            404,
            "not_found",
            format!("{}: {error}", candidate.display()),
        )
    })?;
    if !resolved.starts_with(&base) {
        return Err(ApiError::new(
            403,
            "path_outside_workspace",
            "the requested path is outside the workspace",
        ));
    }
    if !resolved.is_file() {
        return Err(ApiError::new(404, "not_found", "the path is not a file"));
    }
    Ok(resolved)
}

/// Resolve a path that may not exist yet (test save/new), requiring it to stay
/// inside the workspace and to have an allowed DSL extension.
pub(crate) fn workspace_target(state: &WebState, raw: &str) -> Result<PathBuf, ApiError> {
    if raw.trim().is_empty() {
        return Err(ApiError::bad("a file path is required"));
    }
    if raw.contains('\0') {
        return Err(ApiError::bad("the path contains a NUL byte"));
    }
    let base = state.base();
    let base = base
        .canonicalize()
        .map_err(|error| ApiError::new(500, "workspace_unavailable", error.to_string()))?;
    let candidate = Path::new(raw);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base.join(candidate)
    };
    if candidate
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(ApiError::new(
            403,
            "path_outside_workspace",
            "the path must not contain '..'",
        ));
    }
    let parent = candidate
        .parent()
        .ok_or_else(|| ApiError::bad("the path has no parent directory"))?;
    let parent = parent.canonicalize().map_err(|error| {
        ApiError::new(404, "not_found", format!("{}: {error}", parent.display()))
    })?;
    if !parent.starts_with(&base) {
        return Err(ApiError::new(
            403,
            "path_outside_workspace",
            "the requested path is outside the workspace",
        ));
    }
    let extension = candidate
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !matches!(extension, "scene" | "theatre" | "play") {
        return Err(ApiError::bad(
            "test files must end in .scene, .theatre or .play",
        ));
    }
    // An existing target may be a symlink whose link lives inside the
    // workspace while its content does not. Resolve it and confine the real
    // file; a dangling link or a non-file is rejected.
    if candidate.symlink_metadata().is_ok() {
        let resolved = candidate.canonicalize().map_err(|error| {
            ApiError::new(
                404,
                "not_found",
                format!("{}: {error}", candidate.display()),
            )
        })?;
        if !resolved.starts_with(&base) {
            return Err(ApiError::new(
                403,
                "path_outside_workspace",
                "the requested path is outside the workspace",
            ));
        }
        if !resolved.is_file() {
            return Err(ApiError::bad("the target is not a regular file"));
        }
        return Ok(resolved);
    }
    Ok(candidate)
}

/// Read a UTF-8 text file with a size cap.
pub(crate) fn read_text(path: &Path, cap: usize) -> Result<String, ApiError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| ApiError::new(404, "not_found", format!("{}: {error}", path.display())))?;
    if metadata.len() > cap as u64 {
        return Err(ApiError::new(
            413,
            "too_large",
            format!("{} is larger than {} bytes", path.display(), cap),
        ));
    }
    std::fs::read_to_string(path).map_err(|error| {
        ApiError::new(
            500,
            "read_failed",
            format!("cannot read {}: {error}", path.display()),
        )
    })
}

/// Keep the newest `limit` bytes of a text, at a character boundary.
pub(crate) fn tail(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut start = text.len().saturating_sub(limit);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text.get(start..).unwrap_or_default()
}

/// List files under `root`, workspace-relative and slash-separated.
pub(crate) fn list_files(root: &Path, base: &Path, max: usize) -> Vec<Value> {
    let mut files = Vec::new();
    collect_files(root, base, 0, max, &mut files);
    files.sort_by(|left, right| {
        let left = left.get("path").and_then(Value::as_str).unwrap_or_default();
        let right = right
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        left.cmp(right)
    });
    files
}

fn collect_files(root: &Path, base: &Path, depth: usize, max: usize, files: &mut Vec<Value>) {
    if depth > 4 || files.len() >= max {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if files.len() >= max {
            return;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_files(&path, base, depth + 1, max, files);
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            files.push(json!({
                "path": relative,
                "name": entry.file_name().to_string_lossy(),
                "size": size,
            }));
        }
    }
}

/// Serve a JSON artifact: text extensions are returned as text, images as a
/// data URL, everything else as base64.
pub(crate) fn artifact_json(path: &Path) -> Result<Value, ApiError> {
    const TEXT_CAP: usize = 2 * 1024 * 1024;
    const IMAGE_CAP: u64 = 8 * 1024 * 1024;
    let metadata = std::fs::metadata(path)
        .map_err(|error| ApiError::new(404, "not_found", format!("{}: {error}", path.display())))?;
    if !metadata.is_file() {
        return Err(ApiError::new(
            404,
            "not_found",
            "the artifact is not a file",
        ));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = metadata.len();
    let text_extension = matches!(
        extension.as_str(),
        "json" | "jsonl" | "log" | "txt" | "md" | "yaml" | "yml" | "html" | "xml" | "csv"
    );
    if text_extension {
        if size > TEXT_CAP as u64 {
            return Err(ApiError::new(
                413,
                "too_large",
                format!("{name} is larger than {TEXT_CAP} bytes"),
            ));
        }
        let text = std::fs::read_to_string(path).map_err(|error| {
            ApiError::new(500, "read_failed", format!("cannot read {name}: {error}"))
        })?;
        return Ok(json!({
            "name": name,
            "size": size,
            "encoding": "text",
            "text": text,
        }));
    }
    let image_extension = matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif");
    if image_extension && size > IMAGE_CAP {
        return Err(ApiError::new(
            413,
            "too_large",
            format!("{name} is larger than {IMAGE_CAP} bytes"),
        ));
    }
    let cap = if image_extension {
        IMAGE_CAP as usize
    } else {
        TEXT_CAP
    };
    if size > cap as u64 {
        return Err(ApiError::new(
            413,
            "too_large",
            format!("{name} is larger than {cap} bytes"),
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| {
        ApiError::new(500, "read_failed", format!("cannot read {name}: {error}"))
    })?;
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    };
    Ok(json!({
        "name": name,
        "size": size,
        "encoding": "base64",
        "mime": mime,
        "text": encoded,
    }))
}
