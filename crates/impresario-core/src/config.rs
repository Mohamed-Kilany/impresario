//! Configuration files (spec section 4): `impresario.config` and
//! `impresario.devices.config`.
//!
//! Parsing is two-stage on purpose: the YAML tree is validated key by key so
//! that *all* errors in a file are collected (spec section 4), each with a
//! file/line/column and a "did you mean" hint for near-miss keys; only when
//! the tree is clean is it deserialized into the typed structs below.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};

use crate::diag::{suggest, Code, Diagnostic, Diagnostics};
use crate::home::Home;
use crate::platforms::Platform;
use crate::span::Span;
use crate::strings::free_text_ok;

/// `app_reset` policies (spec 4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppReset {
    #[default]
    PerAct,
    PerPlay,
    Never,
}

impl AppReset {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AppReset::PerAct => "per_act",
            AppReset::PerPlay => "per_play",
            AppReset::Never => "never",
        }
    }
}

impl std::fmt::Display for AppReset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Reporters the master can attach (spec 4.1 and 8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reporter {
    Console,
    Junit,
    Json,
    Html,
}

impl Reporter {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Reporter::Console => "console",
            Reporter::Junit => "junit",
            Reporter::Json => "json",
            Reporter::Html => "html",
        }
    }

    /// All reporters, in canonical order.
    pub const ALL: [Reporter; 4] = [
        Reporter::Console,
        Reporter::Junit,
        Reporter::Json,
        Reporter::Html,
    ];
}

impl std::fmt::Display for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn default_plays_dir() -> PathBuf {
    PathBuf::from("./plays")
}

fn default_scenes_dir() -> PathBuf {
    PathBuf::from("./scenes")
}

fn default_theatres_dir() -> PathBuf {
    PathBuf::from("./theatres")
}

fn default_app_reset() -> AppReset {
    AppReset::PerAct
}

fn default_reporters() -> Vec<Reporter> {
    vec![Reporter::Console, Reporter::Junit, Reporter::Json]
}

/// `impresario.config` (spec 4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpresarioConfig {
    pub project: String,
    pub timeout_default_ms: u64,
    pub screenshot_on_failure: bool,
    pub report_output: PathBuf,
    #[serde(default = "default_plays_dir")]
    pub plays_dir: PathBuf,
    #[serde(default = "default_scenes_dir")]
    pub scenes_dir: PathBuf,
    #[serde(default = "default_theatres_dir")]
    pub theatres_dir: PathBuf,
    #[serde(default)]
    pub retries: u32,
    #[serde(default = "parallel_default")]
    pub parallel_devices: u32,
    #[serde(default = "default_app_reset")]
    pub app_reset: AppReset,
    #[serde(default = "poll_default")]
    pub poll_interval_ms: u64,
    #[serde(default = "scroll_attempts_default")]
    pub scroll_max_attempts: u32,
    #[serde(default = "focus_steps_default")]
    pub focus_max_steps: u32,
    #[serde(default = "agent_start_default")]
    pub agent_start_timeout_ms: u64,
    #[serde(default = "command_grace_default")]
    pub command_grace_ms: u64,
    #[serde(default = "default_reporters")]
    pub reporters: Vec<Reporter>,
}

fn parallel_default() -> u32 {
    4
}
fn poll_default() -> u64 {
    100
}
fn scroll_attempts_default() -> u32 {
    10
}
fn focus_steps_default() -> u32 {
    50
}
fn agent_start_default() -> u64 {
    60_000
}
fn command_grace_default() -> u64 {
    2000
}

/// `cdp_backend` option (spec 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CdpBackend {
    #[default]
    Raw,
    Chromiumoxide,
}

impl CdpBackend {
    /// The YAML/wire spelling of the option value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CdpBackend::Raw => "raw",
            CdpBackend::Chromiumoxide => "chromiumoxide",
        }
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == CdpBackend::default()
    }
}

/// `key_dispatch` option (spec 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyDispatch {
    #[default]
    Cdp,
    Js,
}

impl KeyDispatch {
    /// The YAML/wire spelling of the option value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            KeyDispatch::Cdp => "cdp",
            KeyDispatch::Js => "js",
        }
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == KeyDispatch::default()
    }
}

/// How the master reaches a device (TASK-2 4.3). `auto` is resolved from the
/// platform and the connection fields at run time; the wizard writes the
/// resolved value so a hand-edited file is explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    #[default]
    Auto,
    Usb,
    Tcp,
    Network,
    Cdp,
    Launch,
    Ssh,
    Sdb,
}

impl TransportKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TransportKind::Auto => "auto",
            TransportKind::Usb => "usb",
            TransportKind::Tcp => "tcp",
            TransportKind::Network => "network",
            TransportKind::Cdp => "cdp",
            TransportKind::Launch => "launch",
            TransportKind::Ssh => "ssh",
            TransportKind::Sdb => "sdb",
        }
    }

    #[must_use]
    pub fn is_auto(self) -> bool {
        self == TransportKind::Auto
    }
}

/// Connection half of a device definition (TASK-2 4.3). A device never names
/// an app; `endpoint` is the device-side endpoint (CDP URL, agent override or
/// inspector address), never an app URL.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConnection {
    #[serde(default)]
    pub transport: TransportKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Transport port. Defaults: webOS 9922, Tizen 26101, Android TCP 5555.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Android/Tizen serial (`emulator-5554`, USB serial or DUID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Apple UDID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udid: Option<String>,
    /// Device-side endpoint (CDP `ws://...`, inspector or agent override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

impl DeviceConnection {
    /// The documented default port for a TCP-style transport.
    #[must_use]
    pub fn port_or_default(&self, platform: Platform) -> Option<u16> {
        self.port.or(match platform {
            Platform::WebOs => Some(9922),
            Platform::Tizen => Some(26101),
            Platform::Android | Platform::AndroidTv => Some(5555),
            Platform::Web | Platform::Ios | Platform::Tvos => None,
        })
    }
}

/// Secret/identity references of a device (TASK-2 4.3). Values are references
/// (`keychain:<name>`, a key name under `keys/`, a signing identity or a Tizen
/// certificate profile); plaintext secrets are rejected by the wizard.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

impl DeviceAuth {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.key.is_none() && self.identity.is_none() && self.profile.is_none()
    }
}

/// Per-device transport `options` map (TASK-2 4.3). App options (clear data,
/// package path, launch params) live on the app, never here.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceOptions {
    #[serde(default, skip_serializing_if = "CdpBackend::is_default")]
    pub cdp_backend: CdpBackend,
    #[serde(default, skip_serializing_if = "KeyDispatch::is_default")]
    pub key_dispatch: KeyDispatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_match: Option<String>,
    /// Address the master's embedded connector listens on (web runtimes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_port: Option<u16>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub animations_disabled: bool,
}

impl DeviceOptions {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &DeviceOptions::default()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// An adb serial is a network address when it ends in `:port`.
fn serial_is_tcp(serial: &str) -> bool {
    serial
        .rsplit_once(':')
        .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
}

/// One device entry of a devices file (TASK-2 4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    pub name: String,
    pub platform: Platform,
    #[serde(default)]
    pub connection: DeviceConnection,
    #[serde(default, skip_serializing_if = "DeviceAuth::is_empty")]
    pub auth: DeviceAuth,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "DeviceOptions::is_default")]
    pub options: DeviceOptions,
}

impl DeviceConfig {
    /// The transport, with `auto` resolved from the platform and fields.
    #[must_use]
    pub fn effective_transport(&self) -> TransportKind {
        if !self.connection.transport.is_auto() {
            return self.connection.transport;
        }
        match self.platform {
            Platform::Web => {
                if self
                    .connection
                    .endpoint
                    .as_deref()
                    .is_some_and(|value| value.starts_with("ws://") || value.starts_with("wss://"))
                {
                    TransportKind::Cdp
                } else {
                    TransportKind::Launch
                }
            }
            Platform::WebOs => TransportKind::Ssh,
            Platform::Tizen => TransportKind::Sdb,
            Platform::Android | Platform::AndroidTv => {
                if self
                    .connection
                    .serial
                    .as_deref()
                    .is_some_and(|serial| !serial_is_tcp(serial))
                {
                    TransportKind::Usb
                } else {
                    TransportKind::Tcp
                }
            }
            Platform::Ios | Platform::Tvos => {
                if self.connection.host.is_some() {
                    TransportKind::Network
                } else {
                    TransportKind::Usb
                }
            }
        }
    }

    /// The endpoint the agent handshake reports (spec 7.1). It is the device
    /// target, not an app URL: CDP URL for web runtimes, serial/UDID for native
    /// devices.
    #[must_use]
    pub fn target_endpoint(&self) -> String {
        match self.platform {
            Platform::Web => self
                .connection
                .endpoint
                .clone()
                .unwrap_or_else(|| "launch".to_string()),
            Platform::WebOs | Platform::Tizen => {
                self.connection.endpoint.clone().unwrap_or_default()
            }
            Platform::Android | Platform::AndroidTv => {
                if let Some(serial) = &self.connection.serial {
                    serial.clone()
                } else {
                    let host = self.connection.host.as_deref().unwrap_or_default();
                    let port = self
                        .connection
                        .port_or_default(self.platform)
                        .unwrap_or(5555);
                    if host.is_empty() {
                        String::new()
                    } else {
                        format!("{host}:{port}")
                    }
                }
            }
            Platform::Ios | Platform::Tvos => self.connection.udid.clone().unwrap_or_default(),
        }
    }

    /// The connector listen address (web runtimes), with the documented default.
    #[must_use]
    pub fn connector_address(&self) -> Option<&str> {
        self.options
            .connector_address
            .as_deref()
            .or(match self.platform {
                Platform::Web | Platform::WebOs | Platform::Tizen => Some("127.0.0.1:50051"),
                _ => None,
            })
    }

    /// The device-side agent port, with the documented default per platform.
    #[must_use]
    pub fn effective_agent_port(&self) -> Option<u16> {
        if let Some(port) = self.options.agent_port {
            return Some(port);
        }
        match self.platform {
            Platform::Android | Platform::AndroidTv => Some(50061),
            Platform::Ios | Platform::Tvos => Some(50062),
            Platform::Web | Platform::WebOs | Platform::Tizen => None,
        }
    }

    /// Human-readable one-line connection summary for `device list`.
    #[must_use]
    pub fn connection_summary(&self) -> String {
        let transport = self.effective_transport().as_str();
        match self.platform {
            Platform::Web => format!("{transport} {}", self.target_endpoint()),
            Platform::WebOs | Platform::Tizen => {
                let host = self.connection.host.as_deref().unwrap_or("?");
                let port = self
                    .connection
                    .port_or_default(self.platform)
                    .map_or_else(|| "-".to_string(), |p| p.to_string());
                format!("{transport} {host}:{port} -> {}", self.target_endpoint())
            }
            Platform::Android | Platform::AndroidTv => {
                if let Some(serial) = &self.connection.serial {
                    format!("{transport} {serial}")
                } else {
                    let host = self.connection.host.as_deref().unwrap_or("?");
                    let port = self
                        .connection
                        .port_or_default(self.platform)
                        .unwrap_or(5555);
                    format!("{transport} {host}:{port}")
                }
            }
            Platform::Ios | Platform::Tvos => {
                format!("{transport} {}", self.target_endpoint())
            }
        }
    }
}

/// `impresario.devices.config` (workspace) and `~/.impresario/devices.yaml`
/// (global), TASK-2 3.3/4.3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicesConfig {
    pub devices: Vec<DeviceConfig>,
}

/// Result of loading one configuration file: the typed value is present only
/// when the file validated without errors.
#[derive(Debug)]
pub struct ConfigOutcome<T> {
    pub config: Option<T>,
    pub diagnostics: Diagnostics,
}

const CONFIG_KEYS: &[&str] = &[
    "project",
    "timeout_default_ms",
    "screenshot_on_failure",
    "report_output",
    "plays_dir",
    "scenes_dir",
    "theatres_dir",
    "retries",
    "parallel_devices",
    "app_reset",
    "poll_interval_ms",
    "scroll_max_attempts",
    "focus_max_steps",
    "agent_start_timeout_ms",
    "command_grace_ms",
    "reporters",
];

const DEVICE_KEYS: &[&str] = &[
    "name",
    "platform",
    "connection",
    "auth",
    "labels",
    "options",
];

const CONNECTION_KEYS: &[&str] = &["transport", "host", "port", "serial", "udid", "endpoint"];

const AUTH_KEYS: &[&str] = &["key", "identity", "profile"];

const OPTION_KEYS: &[&str] = &[
    "cdp_backend",
    "key_dispatch",
    "page_match",
    "connector_address",
    "agent_port",
    "animations_disabled",
];

// ---------------------------------------------------------------------------
// Line scanning helpers (spans for diagnostics)
// ---------------------------------------------------------------------------

/// Finds the first line (0-based index) at or after `from` on which `key`
/// appears as a mapping key (`key:` at the start of an entry) and returns the
/// 1-based line/col of the key text.
fn find_key(lines: &[&str], key: &str, from: usize, until: usize) -> Option<(usize, usize)> {
    let needle = format!("{key}:");
    let mut line_no = from;
    while line_no <= until {
        let line = lines.get(line_no)?;
        let mut search_from = 0usize;
        while let Some(offset) = line.get(search_from..).and_then(|rest| rest.find(&needle)) {
            let start = search_from + offset;
            let before_ok = line
                .get(..start)
                .and_then(|prefix| prefix.chars().next_back())
                .is_none_or(|c| c.is_whitespace() || c == '-' || c == ',');
            let after = line.get(start + needle.len()..);
            let after_ok =
                after.is_none_or(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '\r']));
            if before_ok && after_ok {
                let col = line
                    .get(..start)
                    .map_or(1, |prefix| prefix.chars().count() + 1);
                return Some((line_no + 1, col));
            }
            search_from = start + needle.len();
        }
        line_no += 1;
    }
    None
}

pub(crate) fn span_of(
    lines: &[&str],
    key: &str,
    from: usize,
    until: usize,
    file: &Path,
) -> Option<Span> {
    find_key(lines, key, from, until)
        .map(|(line, col)| Span::new(file, line, col, key.chars().count()))
}

/// The 0-based line index of the top-level `key`, if present.
pub(crate) fn top_level_line(lines: &[&str], key: &str) -> Option<usize> {
    let needle = format!("{key}:");
    lines.iter().position(|line| {
        line.strip_prefix(&needle)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '\r']))
    })
}

/// Ranges `(first_line, last_line)` (0-based, inclusive) of each `- ` list
/// item that follows the `devices:` line.
pub(crate) fn device_ranges(lines: &[&str], devices_line: usize) -> Vec<(usize, usize)> {
    let list_indent = lines
        .get(devices_line)
        .map(|line| line.len() - line.trim_start().len())
        .unwrap_or(0);
    let mut starts: Vec<usize> = Vec::new();
    for (idx, line) in lines.iter().enumerate().skip(devices_line + 1) {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with("- ") || trimmed == "-" {
            if indent > list_indent {
                starts.push(idx);
            }
            continue;
        }
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with("//")
            && indent <= list_indent
        {
            break;
        }
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (i, start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or_else(
            || {
                let mut end = lines.len().saturating_sub(1);
                while end > *start && lines.get(end).is_some_and(|l| l.trim().is_empty()) {
                    end -= 1;
                }
                end
            },
            |next| next.saturating_sub(1),
        );
        ranges.push((*start, end));
    }
    ranges
}

// ---------------------------------------------------------------------------
// impresario.config
// ---------------------------------------------------------------------------

/// Parse `impresario.config` from source text.
#[must_use]
pub fn parse_config(source: &str, path: &Path) -> ConfigOutcome<ImpresarioConfig> {
    let mut diagnostics = Diagnostics::new();
    let value = match serde_yaml_ng::from_str::<Value>(source) {
        Ok(value) => value,
        Err(err) => {
            diagnostics.push(yaml_syntax_diagnostic(&err, path));
            return ConfigOutcome {
                config: None,
                diagnostics,
            };
        }
    };
    let lines: Vec<&str> = source.lines().collect();
    let Some(map) = value.as_mapping() else {
        diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                "the config file must be a YAML mapping",
            )
            .with_span(Span::new(path, 1, 1, 0))
            .with_help("see docs/CONFIG.md for the file layout"),
        );
        return ConfigOutcome {
            config: None,
            diagnostics,
        };
    };

    for key in map.keys().filter_map(Value::as_str) {
        if !CONFIG_KEYS.contains(&key) {
            let mut diag = Diagnostic::error(Code::CfgUnknownKey, format!("unknown key '{key}'"))
                .with_span(span_of_top(&lines, key, path));
            if let Some(hint) = suggest(key, CONFIG_KEYS) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }

    for required in [
        "project",
        "timeout_default_ms",
        "screenshot_on_failure",
        "report_output",
    ] {
        if map.get(required).is_none() {
            diagnostics.push(
                Diagnostic::error(
                    Code::CfgMissingField,
                    format!("missing required key '{required}'"),
                )
                .with_span(Span::new(path, 1, 1, 0)),
            );
        }
    }

    // project: free text
    if let Some(project) = map.get("project") {
        match project.as_str() {
            Some(text) if free_text_ok(text) => {}
            Some(_) => diagnostics.push(invalid_value(
                &lines,
                path,
                "project",
                "expected 1..=200 characters with no leading/trailing whitespace",
            )),
            None => diagnostics.push(invalid_value(&lines, path, "project", "expected a string")),
        }
    }

    check_int(
        &mut diagnostics,
        &lines,
        path,
        map,
        "timeout_default_ms",
        Some((100, 600_000)),
    );
    check_bool(&mut diagnostics, &lines, path, map, "screenshot_on_failure");
    if let Some(output) = map.get("report_output") {
        match output.as_str() {
            Some(text) if !text.trim().is_empty() => {}
            Some(_) => diagnostics.push(invalid_value(
                &lines,
                path,
                "report_output",
                "expected a non-empty directory path",
            )),
            None => diagnostics.push(invalid_value(
                &lines,
                path,
                "report_output",
                "expected a directory path",
            )),
        }
    }
    for key in ["plays_dir", "scenes_dir", "theatres_dir"] {
        if let Some(value) = map.get(key) {
            match value.as_str() {
                Some(text) if !text.trim().is_empty() => {}
                _ => diagnostics.push(invalid_value(
                    &lines,
                    path,
                    key,
                    "expected a directory path",
                )),
            }
        }
    }
    check_int(&mut diagnostics, &lines, path, map, "retries", Some((0, 5)));
    check_int(
        &mut diagnostics,
        &lines,
        path,
        map,
        "parallel_devices",
        Some((1, 64)),
    );
    check_int(
        &mut diagnostics,
        &lines,
        path,
        map,
        "poll_interval_ms",
        Some((20, 2000)),
    );
    check_int(
        &mut diagnostics,
        &lines,
        path,
        map,
        "scroll_max_attempts",
        Some((1, 100)),
    );
    check_int(
        &mut diagnostics,
        &lines,
        path,
        map,
        "focus_max_steps",
        Some((1, 500)),
    );
    check_int(
        &mut diagnostics,
        &lines,
        path,
        map,
        "agent_start_timeout_ms",
        Some((1000, 600_000)),
    );
    check_int(
        &mut diagnostics,
        &lines,
        path,
        map,
        "command_grace_ms",
        Some((0, 30_000)),
    );
    if let Some(reset) = map.get("app_reset") {
        let valid = reset
            .as_str()
            .is_some_and(|s| matches!(s, "per_act" | "per_play" | "never"));
        if !valid {
            let mut diag = invalid_value(
                &lines,
                path,
                "app_reset",
                "expected one of: per_act, per_play, never",
            );
            if let Some(text) = reset.as_str() {
                if let Some(hint) = suggest(text, &["per_act", "per_play", "never"]) {
                    diag = diag.with_help(hint);
                }
            }
            diagnostics.push(diag);
        }
    }
    if let Some(reporters) = map.get("reporters") {
        match reporters.as_sequence() {
            Some(seq) => {
                let mut seen: HashSet<&str> = HashSet::new();
                for item in seq {
                    let Some(name) = item.as_str() else {
                        diagnostics.push(invalid_value(
                            &lines,
                            path,
                            "reporters",
                            "expected a list of reporter names",
                        ));
                        break;
                    };
                    let known = Reporter::ALL.iter().any(|r| r.as_str() == name);
                    if !known {
                        let mut diag = invalid_value(
                            &lines,
                            path,
                            "reporters",
                            format!("unknown reporter '{name}'"),
                        );
                        if let Some(hint) = suggest(name, &["console", "junit", "json", "html"]) {
                            diag = diag.with_help(hint);
                        }
                        diagnostics.push(diag);
                    } else if !seen.insert(name) {
                        diagnostics.push(invalid_value(
                            &lines,
                            path,
                            "reporters",
                            format!("duplicate reporter '{name}'"),
                        ));
                    }
                }
            }
            None => diagnostics.push(invalid_value(
                &lines,
                path,
                "reporters",
                "expected a list such as [console, junit, json]",
            )),
        }
    }

    if diagnostics.has_errors() {
        return ConfigOutcome {
            config: None,
            diagnostics,
        };
    }
    match serde_yaml_ng::from_value::<ImpresarioConfig>(value) {
        Ok(config) => ConfigOutcome {
            config: Some(config),
            diagnostics,
        },
        Err(err) => {
            diagnostics.push(Diagnostic::error(
                Code::CfgInvalidValue,
                format!("invalid configuration: {err}"),
            ));
            ConfigOutcome {
                config: None,
                diagnostics,
            }
        }
    }
}

/// Read and parse `impresario.config` from disk.
#[must_use]
pub fn load_config(path: &Path) -> ConfigOutcome<ImpresarioConfig> {
    match std::fs::read_to_string(path) {
        Ok(source) => parse_config(&source, path),
        Err(err) => ConfigOutcome {
            config: None,
            diagnostics: {
                let mut diags = Diagnostics::new();
                diags.push(Diagnostic::error(
                    Code::CfgInvalidValue,
                    format!("cannot read {}: {err}", path.display()),
                ));
                diags
            },
        },
    }
}

/// Static check for `report_output`: the nearest existing ancestor must be a
/// directory (the master creates the rest).
#[must_use]
pub fn check_report_output(output: &Path) -> Option<Diagnostic> {
    let mut probe = output;
    loop {
        if probe.as_os_str().is_empty() {
            return None;
        }
        match std::fs::metadata(probe) {
            Ok(meta) if meta.is_dir() => return None,
            Ok(_) => {
                return Some(Diagnostic::error(
                    Code::CfgInvalidValue,
                    format!(
                        "report_output parent '{}' is not a directory",
                        probe.display()
                    ),
                ));
            }
            Err(_) => match probe.parent() {
                Some(parent) => probe = parent,
                None => return None,
            },
        }
    }
}

fn span_of_top(lines: &[&str], key: &str, path: &Path) -> Span {
    span_of(lines, key, 0, lines.len().saturating_sub(1), path)
        .unwrap_or_else(|| Span::new(path, 1, 1, 0))
}

fn invalid_value(lines: &[&str], path: &Path, key: &str, reason: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        Code::CfgInvalidValue,
        format!("invalid value for '{key}': {}", reason.into()),
    )
    .with_span(span_of_top(lines, key, path))
}

fn check_int(
    diagnostics: &mut Diagnostics,
    lines: &[&str],
    path: &Path,
    map: &Mapping,
    key: &str,
    range: Option<(u64, u64)>,
) {
    let Some(value) = map.get(key) else { return };
    let Some(number) = value.as_u64() else {
        diagnostics.push(invalid_value(
            lines,
            path,
            key,
            "expected a non-negative integer",
        ));
        return;
    };
    if let Some((min, max)) = range {
        if number < min || number > max {
            diagnostics.push(invalid_value(
                lines,
                path,
                key,
                format!("expected {min}..={max}, got {number}"),
            ));
        }
    }
}

fn check_bool(
    diagnostics: &mut Diagnostics,
    lines: &[&str],
    path: &Path,
    map: &Mapping,
    key: &str,
) {
    let Some(value) = map.get(key) else { return };
    if value.as_bool().is_none() {
        diagnostics.push(invalid_value(lines, path, key, "expected true or false"));
    }
}

pub(crate) fn yaml_syntax_diagnostic(err: &serde_yaml_ng::Error, path: &Path) -> Diagnostic {
    let span = err.location().map_or(Span::new(path, 1, 1, 0), |loc| {
        Span::new(path, loc.line(), loc.column(), 0)
    });
    Diagnostic::error(Code::CfgInvalidValue, format!("invalid YAML: {err}")).with_span(span)
}

// ---------------------------------------------------------------------------
// impresario.devices.config
// ---------------------------------------------------------------------------

/// Parse `impresario.devices.config` from source text.
#[must_use]
pub fn parse_devices(source: &str, path: &Path) -> ConfigOutcome<DevicesConfig> {
    let mut diagnostics = Diagnostics::new();
    let value = match serde_yaml_ng::from_str::<Value>(source) {
        Ok(value) => value,
        Err(err) => {
            diagnostics.push(yaml_syntax_diagnostic(&err, path));
            return ConfigOutcome {
                config: None,
                diagnostics,
            };
        }
    };
    let lines: Vec<&str> = source.lines().collect();
    let last_line = lines.len().saturating_sub(1);
    let Some(map) = value.as_mapping() else {
        diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                "the devices file must be a YAML mapping",
            )
            .with_span(Span::new(path, 1, 1, 0)),
        );
        return ConfigOutcome {
            config: None,
            diagnostics,
        };
    };
    for key in map.keys().filter_map(Value::as_str) {
        if key != "devices" {
            let mut diag = Diagnostic::error(Code::CfgUnknownKey, format!("unknown key '{key}'"))
                .with_span(span_of_top(&lines, key, path))
                .with_help("the devices file only has a top-level 'devices' list");
            if let Some(hint) = suggest(key, &["devices"]) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }
    let Some(devices_value) = map.get("devices") else {
        diagnostics.push(
            Diagnostic::error(Code::CfgMissingField, "missing required key 'devices'")
                .with_span(Span::new(path, 1, 1, 0)),
        );
        return ConfigOutcome {
            config: None,
            diagnostics,
        };
    };
    let Some(devices) = devices_value.as_sequence() else {
        diagnostics.push(invalid_value(
            &lines,
            path,
            "devices",
            "expected a list of devices",
        ));
        return ConfigOutcome {
            config: None,
            diagnostics,
        };
    };
    let ranges = top_level_line(&lines, "devices")
        .map(|line| device_ranges(&lines, line))
        .unwrap_or_default();

    let mut seen_names: HashSet<String> = HashSet::new();
    for (index, device) in devices.iter().enumerate() {
        let (from, until) = ranges.get(index).copied().unwrap_or((0, last_line));
        validate_device(
            &mut diagnostics,
            &lines,
            path,
            device,
            from,
            until,
            &mut seen_names,
        );
    }

    if diagnostics.has_errors() {
        return ConfigOutcome {
            config: None,
            diagnostics,
        };
    }
    match serde_yaml_ng::from_value::<DevicesConfig>(value) {
        Ok(config) => ConfigOutcome {
            config: Some(config),
            diagnostics,
        },
        Err(err) => {
            diagnostics.push(Diagnostic::error(
                Code::CfgInvalidValue,
                format!("invalid devices configuration: {err}"),
            ));
            ConfigOutcome {
                config: None,
                diagnostics,
            }
        }
    }
}

/// Read and parse `impresario.devices.config` from disk.
#[must_use]
pub fn load_devices(path: &Path) -> ConfigOutcome<DevicesConfig> {
    match std::fs::read_to_string(path) {
        Ok(source) => parse_devices(&source, path),
        Err(err) => ConfigOutcome {
            config: None,
            diagnostics: {
                let mut diags = Diagnostics::new();
                diags.push(Diagnostic::error(
                    Code::CfgInvalidValue,
                    format!("cannot read {}: {err}", path.display()),
                ));
                diags
            },
        },
    }
}

/// Load the global `~/.impresario/devices.yaml` and the workspace devices file
/// and merge them by name (workspace wins, TASK-2 3.3). Missing files are empty
/// layers, so a workspace without devices still loads (and `run` explains that
/// no devices are configured). Diagnostics carry the file they came from.
#[must_use]
pub fn load_devices_merged(workspace_path: &Path) -> ConfigOutcome<DevicesConfig> {
    let mut diagnostics = Diagnostics::new();
    let mut merged: Vec<DeviceConfig> = Vec::new();
    let global_path = Home::resolve().ok().map(|home| home.devices_path());
    if let Some(path) = global_path.as_deref() {
        if let Some(source) = read_optional_layer(path, &mut diagnostics) {
            let outcome = parse_devices(&source, path);
            diagnostics.extend(outcome.diagnostics);
            if let Some(config) = outcome.config {
                merge_devices(&mut merged, &config.devices);
            }
        }
    }
    if let Some(source) = read_optional_layer(workspace_path, &mut diagnostics) {
        let outcome = parse_devices(&source, workspace_path);
        diagnostics.extend(outcome.diagnostics);
        if let Some(config) = outcome.config {
            merge_devices(&mut merged, &config.devices);
        }
    }
    let config = (!diagnostics.has_errors()).then_some(DevicesConfig { devices: merged });
    ConfigOutcome {
        config,
        diagnostics,
    }
}

fn merge_devices(merged: &mut Vec<DeviceConfig>, layer: &[DeviceConfig]) {
    for device in layer {
        if let Some(existing) = merged.iter_mut().find(|item| item.name == device.name) {
            *existing = device.clone();
        } else {
            merged.push(device.clone());
        }
    }
}

pub(crate) fn read_optional_layer(path: &Path, diagnostics: &mut Diagnostics) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(source) => Some(source),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            diagnostics.push(Diagnostic::error(
                Code::CfgInvalidValue,
                format!("cannot read {}: {err}", path.display()),
            ));
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_device(
    diagnostics: &mut Diagnostics,
    lines: &[&str],
    path: &Path,
    device: &Value,
    from: usize,
    until: usize,
    seen_names: &mut HashSet<String>,
) {
    let key_span = |key: &str| {
        span_of(lines, key, from, until, path).unwrap_or_else(|| Span::new(path, from + 1, 1, 0))
    };
    let invalid_here = |key: &str, reason: String| {
        Diagnostic::error(
            Code::CfgInvalidValue,
            format!("invalid value for '{key}': {reason}"),
        )
        .with_span(key_span(key))
    };
    let Some(map) = device.as_mapping() else {
        diagnostics.push(
            Diagnostic::error(Code::CfgInvalidValue, "each device must be a YAML mapping")
                .with_span(key_span("name")),
        );
        return;
    };

    for key in map.keys().filter_map(Value::as_str) {
        if !DEVICE_KEYS.contains(&key) {
            let mut diag =
                Diagnostic::error(Code::CfgUnknownKey, format!("unknown device key '{key}'"))
                    .with_span(key_span(key));
            if matches!(
                key,
                "target_endpoint" | "app_id" | "app_url" | "agent_address" | "connector"
            ) {
                diag = diag.with_help(
                    "this is the legacy device schema; run `impresario migrate` to split the \
                     device and its app, then use `connection`/`auth`/`labels`/`options`",
                );
            } else if let Some(hint) = suggest(key, DEVICE_KEYS) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }

    // name
    match map.get("name").and_then(Value::as_str) {
        Some(name) if valid_device_name(name) => {
            if !seen_names.insert(name.to_string()) {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgDuplicateDevice,
                        format!("duplicate device name '{name}'"),
                    )
                    .with_span(key_span("name"))
                    .with_help("device names must be unique within the devices file"),
                );
            }
        }
        Some(name) => diagnostics.push(invalid_here(
            "name",
            format!("'{name}' must match ^[a-z0-9][a-z0-9_-]{{0,62}}$"),
        )),
        None => diagnostics.push(
            Diagnostic::error(Code::CfgMissingField, "missing required key 'name'")
                .with_span(key_span("name")),
        ),
    }

    // platform
    let mut parsed_platform: Option<Platform> = None;
    match map.get("platform").and_then(Value::as_str) {
        Some(text) => match text.parse::<Platform>() {
            Ok(platform) => parsed_platform = Some(platform),
            Err(()) => {
                let mut diag = invalid_here("platform", format!("unknown platform '{text}'"));
                if let Some(hint) = suggest(
                    text,
                    &[
                        "web",
                        "webos",
                        "tizen",
                        "android",
                        "androidtv",
                        "ios",
                        "tvos",
                    ],
                ) {
                    diag = diag.with_help(hint);
                }
                diagnostics.push(diag);
            }
        },
        None => diagnostics.push(
            Diagnostic::error(Code::CfgMissingField, "missing required key 'platform'")
                .with_span(key_span("platform")),
        ),
    }

    if let Some(platform) = parsed_platform {
        validate_connection(diagnostics, lines, path, map, platform, from, until);
        validate_auth(diagnostics, lines, path, map, platform, from, until);
        validate_labels(diagnostics, lines, path, map, from, until);
        validate_options(diagnostics, lines, path, map, platform, from, until);
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_connection(
    diagnostics: &mut Diagnostics,
    lines: &[&str],
    path: &Path,
    device: &Mapping,
    platform: Platform,
    from: usize,
    until: usize,
) {
    let key_span = |key: &str| {
        span_of(lines, key, from, until, path).unwrap_or_else(|| Span::new(path, from + 1, 1, 0))
    };
    let Some(connection_value) = device.get("connection") else {
        diagnostics.push(
            Diagnostic::error(Code::CfgMissingField, "missing required key 'connection'")
                .with_span(key_span("name"))
                .with_help("a device is only a platform plus its connection; apps live separately"),
        );
        return;
    };
    let Some(connection) = connection_value.as_mapping() else {
        diagnostics.push(
            Diagnostic::error(Code::CfgInvalidValue, "'connection' must be a YAML mapping")
                .with_span(key_span("connection")),
        );
        return;
    };
    let invalid = |diagnostics: &mut Diagnostics, key: &str, reason: String| {
        diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                format!("invalid value for 'connection.{key}': {reason}"),
            )
            .with_span(key_span(key)),
        );
    };
    for key in connection.keys().filter_map(Value::as_str) {
        if !CONNECTION_KEYS.contains(&key) {
            let mut diag = Diagnostic::error(
                Code::CfgUnknownKey,
                format!("unknown connection key '{key}'"),
            )
            .with_span(key_span(key));
            if let Some(hint) = suggest(key, CONNECTION_KEYS) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }

    let transport = match connection.get("transport") {
        Some(value) => match value.as_str() {
            Some(text) => match text {
                "auto" => Some(TransportKind::Auto),
                "usb" => Some(TransportKind::Usb),
                "tcp" => Some(TransportKind::Tcp),
                "network" => Some(TransportKind::Network),
                "cdp" => Some(TransportKind::Cdp),
                "launch" => Some(TransportKind::Launch),
                "ssh" => Some(TransportKind::Ssh),
                "sdb" => Some(TransportKind::Sdb),
                _ => {
                    invalid(
                        diagnostics,
                        "transport",
                        format!(
                            "unknown transport '{text}'; expected auto, usb, tcp, network, cdp, \
                             launch, ssh or sdb"
                        ),
                    );
                    None
                }
            },
            None => {
                invalid(
                    diagnostics,
                    "transport",
                    "expected a transport name".to_string(),
                );
                None
            }
        },
        None => Some(TransportKind::Auto),
    };
    if let Some(transport) = transport {
        let allowed: &[TransportKind] = match platform {
            Platform::Web => &[
                TransportKind::Auto,
                TransportKind::Launch,
                TransportKind::Cdp,
            ],
            Platform::WebOs => &[TransportKind::Auto, TransportKind::Ssh],
            Platform::Tizen => &[TransportKind::Auto, TransportKind::Sdb],
            Platform::Android | Platform::AndroidTv => {
                &[TransportKind::Auto, TransportKind::Usb, TransportKind::Tcp]
            }
            Platform::Ios | Platform::Tvos => &[
                TransportKind::Auto,
                TransportKind::Usb,
                TransportKind::Network,
            ],
        };
        if !allowed.contains(&transport) {
            let names: Vec<&str> = allowed.iter().map(|t| t.as_str()).collect();
            invalid(
                diagnostics,
                "transport",
                format!(
                    "'{}' is not valid on platform '{platform}'; valid: {}",
                    transport.as_str(),
                    names.join(", ")
                ),
            );
        }
    }

    let host = connection.get("host").and_then(Value::as_str);
    if connection.get("host").is_some() && host.is_none_or(|value| !valid_host(value)) {
        invalid(
            diagnostics,
            "host",
            "expected a non-empty hostname or IP address without spaces".to_string(),
        );
    }
    let port = connection.get("port");
    match port.and_then(Value::as_u64) {
        Some(value) if (1..=65535).contains(&value) => {}
        Some(value) => invalid(
            diagnostics,
            "port",
            format!("{value} is not a port in 1..=65535"),
        ),
        None if port.is_some() => {
            invalid(diagnostics, "port", "expected an integer port".to_string());
        }
        None => {}
    }
    let serial = connection.get("serial").and_then(Value::as_str);
    if connection.get("serial").is_some() && serial.is_none_or(|value| value.trim().is_empty()) {
        invalid(
            diagnostics,
            "serial",
            "expected a non-empty serial".to_string(),
        );
    }
    let udid = connection.get("udid").and_then(Value::as_str);
    if let Some(value) = udid {
        if !valid_udid(value) {
            invalid(
                diagnostics,
                "udid",
                format!("'{value}' is not a UDID or simulator UUID"),
            );
        }
    } else if connection.get("udid").is_some() {
        invalid(diagnostics, "udid", "expected a string UDID".to_string());
    }
    let endpoint = connection.get("endpoint").and_then(Value::as_str);
    if connection.get("endpoint").is_some() && endpoint.is_none_or(|value| value.is_empty()) {
        invalid(
            diagnostics,
            "endpoint",
            "expected a non-empty endpoint".to_string(),
        );
    }

    match platform {
        Platform::Web => {
            if host.is_some() || port.is_some() || serial.is_some() || udid.is_some() {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgInvalidValue,
                        "web devices only use transport, endpoint and page_match; host/port/serial/udid belong to native or TV devices",
                    )
                    .with_span(key_span("connection")),
                );
            }
            match transport.unwrap_or(TransportKind::Auto) {
                TransportKind::Cdp | TransportKind::Auto
                    if endpoint.is_some_and(|value| {
                        value.starts_with("ws://") || value.starts_with("wss://")
                    }) => {}
                TransportKind::Cdp => {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::CfgMissingField,
                            "transport 'cdp' needs connection.endpoint with a ws:// URL",
                        )
                        .with_span(key_span("endpoint"))
                        .with_help(endpoint_help(platform)),
                    );
                }
                TransportKind::Launch | TransportKind::Auto => {
                    if let Some(value) = endpoint {
                        if value != "launch" && value != "launch:headless" {
                            diagnostics.push(
                                Diagnostic::error(
                                    Code::CfgInvalidValue,
                                    "transport 'launch' cannot take a CDP endpoint; use transport 'cdp' or remove connection.endpoint",
                                )
                                .with_span(key_span("endpoint")),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        Platform::WebOs => {
            if host.is_none() {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgMissingField,
                        "webOS devices need connection.host (the TV's IP or hostname)",
                    )
                    .with_span(key_span("host")),
                );
            }
            if let Some(value) = endpoint {
                if let Err(reason) = validate_endpoint(platform, value) {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::CfgBadEndpoint,
                            format!("invalid connection.endpoint for '{platform}': {reason}"),
                        )
                        .with_span(key_span("endpoint"))
                        .with_help(endpoint_help(platform)),
                    );
                }
            }
            if serial.is_some() || udid.is_some() {
                invalid(
                    diagnostics,
                    "serial",
                    "webOS devices use host/port, not serial/udid".to_string(),
                );
            }
        }
        Platform::Tizen => {
            if host.is_none() {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgMissingField,
                        "Tizen devices need connection.host (the TV's IP or hostname)",
                    )
                    .with_span(key_span("host")),
                );
            }
            if let Some(value) = endpoint {
                if let Err(reason) = validate_endpoint(platform, value) {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::CfgBadEndpoint,
                            format!("invalid connection.endpoint for '{platform}': {reason}"),
                        )
                        .with_span(key_span("endpoint"))
                        .with_help(endpoint_help(platform)),
                    );
                }
            }
            if udid.is_some() {
                invalid(
                    diagnostics,
                    "udid",
                    "Tizen devices use serial (DUID) or host, not udid".to_string(),
                );
            }
        }
        Platform::Android | Platform::AndroidTv => {
            if udid.is_some() {
                invalid(
                    diagnostics,
                    "udid",
                    "Android devices use serial (USB/TCP), not udid".to_string(),
                );
            }
            let transport = transport.unwrap_or(TransportKind::Auto);
            let tcp = matches!(transport, TransportKind::Tcp)
                || (transport == TransportKind::Auto && serial.is_none() && host.is_some());
            if tcp {
                if host.is_none() {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::CfgMissingField,
                            "Android TCP devices need connection.host",
                        )
                        .with_span(key_span("host")),
                    );
                }
                if serial.is_some() {
                    invalid(
                        diagnostics,
                        "serial",
                        "TCP devices use host/port; remove serial or select transport 'usb'"
                            .to_string(),
                    );
                }
            } else {
                if serial.is_none() {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::CfgMissingField,
                            "Android USB devices need connection.serial",
                        )
                        .with_span(key_span("serial")),
                    );
                }
                if host.is_some() {
                    invalid(
                        diagnostics,
                        "host",
                        "USB devices use serial; remove host or select transport 'tcp'".to_string(),
                    );
                }
            }
            if let Some(value) = endpoint {
                if !valid_host_port(value) {
                    invalid(
                        diagnostics,
                        "endpoint",
                        format!("'{value}' is not host:port (agent override)"),
                    );
                }
            }
        }
        Platform::Ios | Platform::Tvos => {
            if serial.is_some() {
                invalid(
                    diagnostics,
                    "serial",
                    "Apple devices use udid, not serial".to_string(),
                );
            }
            if udid.is_none() {
                diagnostics.push(
                    Diagnostic::error(Code::CfgMissingField, "Apple devices need connection.udid")
                        .with_span(key_span("udid")),
                );
            }
            if let Some(value) = endpoint {
                if !valid_host_port(value) {
                    invalid(
                        diagnostics,
                        "endpoint",
                        format!("'{value}' is not host:port (agent override)"),
                    );
                }
            }
            if platform == Platform::Tvos
                && transport.unwrap_or(TransportKind::Auto) != TransportKind::Network
                && endpoint.is_none()
            {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgMissingField,
                        "tvOS over USB needs connection.endpoint (agent host:port)",
                    )
                    .with_span(key_span("endpoint"))
                    .with_help(
                        "Bonjour discovery is not implemented (docs/STATUS.md); use transport \
                         'network' with discovery or set connection.endpoint",
                    ),
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_auth(
    diagnostics: &mut Diagnostics,
    lines: &[&str],
    path: &Path,
    device: &Mapping,
    platform: Platform,
    from: usize,
    until: usize,
) {
    let key_span = |key: &str| {
        span_of(lines, key, from, until, path).unwrap_or_else(|| Span::new(path, from + 1, 1, 0))
    };
    let Some(auth_value) = device.get("auth") else {
        return;
    };
    let Some(auth) = auth_value.as_mapping() else {
        diagnostics.push(
            Diagnostic::error(Code::CfgInvalidValue, "'auth' must be a YAML mapping")
                .with_span(key_span("auth")),
        );
        return;
    };
    for key in auth.keys().filter_map(Value::as_str) {
        if !AUTH_KEYS.contains(&key) {
            let mut diag =
                Diagnostic::error(Code::CfgUnknownKey, format!("unknown auth key '{key}'"))
                    .with_span(key_span(key));
            if let Some(hint) = suggest(key, AUTH_KEYS) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }
    for key in AUTH_KEYS {
        if let Some(value) = auth.get(*key) {
            match value.as_str() {
                Some(text) if !text.trim().is_empty() => {
                    if text.starts_with("keychain:") && text.len() == "keychain:".len() {
                        diagnostics.push(
                            Diagnostic::error(
                                Code::CfgInvalidValue,
                                format!("auth.{key} has an empty keychain reference"),
                            )
                            .with_span(key_span(key)),
                        );
                    }
                }
                _ => diagnostics.push(
                    Diagnostic::error(
                        Code::CfgInvalidValue,
                        format!("auth.{key} must be a non-empty reference"),
                    )
                    .with_span(key_span(key)),
                ),
            }
        }
    }
    if platform == Platform::Web
        && auth
            .keys()
            .filter_map(Value::as_str)
            .any(|key| AUTH_KEYS.contains(&key))
    {
        diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                "web devices do not use auth; credentials belong to native/TV transports",
            )
            .with_span(key_span("auth")),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_labels(
    diagnostics: &mut Diagnostics,
    lines: &[&str],
    path: &Path,
    device: &Mapping,
    from: usize,
    until: usize,
) {
    let key_span = |key: &str| {
        span_of(lines, key, from, until, path).unwrap_or_else(|| Span::new(path, from + 1, 1, 0))
    };
    let Some(labels_value) = device.get("labels") else {
        return;
    };
    let Some(labels) = labels_value.as_sequence() else {
        diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                "'labels' must be a list of label strings",
            )
            .with_span(key_span("labels")),
        );
        return;
    };
    let mut seen: HashSet<&str> = HashSet::new();
    for label in labels {
        let Some(text) = label.as_str() else {
            diagnostics.push(
                Diagnostic::error(Code::CfgInvalidValue, "each label must be a string")
                    .with_span(key_span("labels")),
            );
            continue;
        };
        if !valid_label(text) {
            diagnostics.push(
                Diagnostic::error(
                    Code::CfgInvalidValue,
                    format!("invalid label '{text}'; expected ^[a-z0-9][a-z0-9_-]{{0,31}}$"),
                )
                .with_span(key_span("labels")),
            );
        } else if !seen.insert(text) {
            diagnostics.push(
                Diagnostic::error(Code::CfgInvalidValue, format!("duplicate label '{text}'"))
                    .with_span(key_span("labels")),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_options(
    diagnostics: &mut Diagnostics,
    lines: &[&str],
    path: &Path,
    device: &Mapping,
    platform: Platform,
    from: usize,
    until: usize,
) {
    let Some(options_value) = device.get("options") else {
        return;
    };
    let key_span = |key: &str| {
        span_of(lines, key, from, until, path).unwrap_or_else(|| Span::new(path, from + 1, 1, 0))
    };
    let Some(options) = options_value.as_mapping() else {
        diagnostics.push(
            Diagnostic::error(Code::CfgInvalidValue, "'options' must be a YAML mapping")
                .with_span(key_span("options")),
        );
        return;
    };
    for key in options.keys().filter_map(Value::as_str) {
        if !OPTION_KEYS.contains(&key) {
            let mut diag = Diagnostic::error(
                Code::CfgUnknownKey,
                format!("unknown device option '{key}'"),
            )
            .with_span(key_span(key));
            if matches!(key, "clear_app_data" | "app_path") {
                diag = diag.with_help(
                    "app options moved to the apps file (TASK-2 5); run `impresario migrate`",
                );
            } else if let Some(hint) = suggest(key, OPTION_KEYS) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }
    let invalid = |diagnostics: &mut Diagnostics, key: &str, reason: String| {
        diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                format!("invalid value for option '{key}': {reason}"),
            )
            .with_span(key_span(key)),
        );
    };
    let platform_problem = |key: &str, allowed: &[Platform]| -> Option<String> {
        if allowed.contains(&platform) {
            None
        } else {
            let names: Vec<&str> = allowed.iter().map(|p| p.as_str()).collect();
            Some(format!(
                "'{key}' is not valid on platform '{platform}'; valid on: {}",
                names.join(", ")
            ))
        }
    };
    let web_runtimes = [Platform::Web, Platform::WebOs, Platform::Tizen];
    let native = [
        Platform::Android,
        Platform::AndroidTv,
        Platform::Ios,
        Platform::Tvos,
    ];

    if let Some(value) = options.get("cdp_backend") {
        if let Some(reason) = platform_problem("cdp_backend", &web_runtimes) {
            invalid(diagnostics, "cdp_backend", reason);
        }
        if !value
            .as_str()
            .is_some_and(|s| matches!(s, "raw" | "chromiumoxide"))
        {
            invalid(
                diagnostics,
                "cdp_backend",
                "expected 'raw' or 'chromiumoxide'".to_string(),
            );
        }
    }
    if let Some(value) = options.get("key_dispatch") {
        if let Some(reason) = platform_problem("key_dispatch", &web_runtimes) {
            invalid(diagnostics, "key_dispatch", reason);
        }
        if !value.as_str().is_some_and(|s| matches!(s, "cdp" | "js")) {
            invalid(
                diagnostics,
                "key_dispatch",
                "expected 'cdp' or 'js'".to_string(),
            );
        }
    }
    if let Some(value) = options.get("page_match") {
        if let Some(reason) = platform_problem("page_match", &web_runtimes) {
            invalid(diagnostics, "page_match", reason);
        }
        if value.as_str().is_none_or(|s| s.is_empty()) {
            invalid(
                diagnostics,
                "page_match",
                "expected a non-empty substring".to_string(),
            );
        }
    }
    if let Some(value) = options.get("connector_address") {
        if let Some(reason) = platform_problem("connector_address", &web_runtimes) {
            invalid(diagnostics, "connector_address", reason);
        }
        match value.as_str() {
            Some(text) if valid_host_port(text) => {}
            _ => invalid(
                diagnostics,
                "connector_address",
                "expected host:port".to_string(),
            ),
        }
    }
    if let Some(value) = options.get("agent_port") {
        if let Some(reason) = platform_problem("agent_port", &native) {
            invalid(diagnostics, "agent_port", reason);
        }
        match value.as_u64() {
            Some(port) if (1..=65535).contains(&port) => {}
            _ => invalid(
                diagnostics,
                "agent_port",
                "expected a port in 1..=65535".to_string(),
            ),
        }
    }
    if let Some(value) = options.get("animations_disabled") {
        if let Some(reason) =
            platform_problem("animations_disabled", &[Platform::Ios, Platform::Tvos])
        {
            invalid(diagnostics, "animations_disabled", reason);
        }
        if value.as_bool().is_none() {
            invalid(
                diagnostics,
                "animations_disabled",
                "expected true or false".to_string(),
            );
        }
    }
}

fn valid_host(text: &str) -> bool {
    !text.is_empty()
        && !text.contains(char::is_whitespace)
        && !text.contains('/')
        && !text.contains('\\')
}

fn valid_label(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && text.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn endpoint_help(platform: Platform) -> &'static str {
    match platform {
        Platform::Web => "expected 'launch', 'launch:headless' or a ws:// URL",
        Platform::WebOs | Platform::Tizen => "expected ws://host:port[/...]",
        Platform::Android | Platform::AndroidTv => "expected an adb serial such as emulator-5554",
        Platform::Ios | Platform::Tvos => "expected a device UDID or a simulator UUID",
    }
}

/// Device name rule: `^[a-z0-9][a-z0-9_-]{0,62}$`.
#[must_use]
pub fn valid_device_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    let mut count = 1usize;
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return false;
        }
        count += 1;
    }
    count <= 63
}

fn valid_host_port(text: &str) -> bool {
    let Some((host, port)) = text.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && !port.is_empty()
        && port.chars().all(|c| c.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|p| p > 0)
}

fn ws_url_parts(url: &str) -> Option<(bool, &str, bool)> {
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))?;
    let secure = url.starts_with("wss://");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    let has_port = if let Some((host, port)) = authority.rsplit_once(':') {
        !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0)
    } else {
        false
    };
    let has_host = !authority.starts_with(':') && !authority.is_empty();
    Some((secure, authority, has_host && has_port))
}

/// `target_endpoint` validation per platform (spec 4.2).
pub fn validate_endpoint(platform: Platform, endpoint: &str) -> Result<(), String> {
    match platform {
        Platform::Web => {
            if endpoint == "launch" || endpoint == "launch:headless" {
                return Ok(());
            }
            match ws_url_parts(endpoint) {
                Some((_, authority, _)) if !authority.is_empty() => Ok(()),
                _ => Err(format!(
                    "'{endpoint}' is not 'launch', 'launch:headless' or a ws:// URL"
                )),
            }
        }
        Platform::WebOs | Platform::Tizen => match ws_url_parts(endpoint) {
            Some((_, authority, true)) => {
                if authority.starts_with(':') {
                    return Err(format!("'{endpoint}' has no host"));
                }
                Ok(())
            }
            Some(_) => Err(format!(
                "'{endpoint}' needs an explicit port (ws://host:port)"
            )),
            None => Err(format!("'{endpoint}' is not a ws:// or wss:// URL")),
        },
        Platform::Android | Platform::AndroidTv => {
            let ok = (4..=64).contains(&endpoint.chars().count())
                && endpoint
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
            if ok {
                Ok(())
            } else {
                Err(format!(
                    "'{endpoint}' is not a valid adb serial (^[A-Za-z0-9._:-]{{4,64}}$)"
                ))
            }
        }
        Platform::Ios | Platform::Tvos => {
            if valid_udid(endpoint) {
                Ok(())
            } else {
                Err(format!("'{endpoint}' is not a UDID or simulator UUID"))
            }
        }
    }
}

/// Modern UDID (8-16 hex), legacy 40-hex UDID, or RFC 4122 UUID.
#[must_use]
pub fn valid_udid(text: &str) -> bool {
    fn all_hex(s: &str) -> bool {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
    }
    let segments: Vec<&str> = text.split('-').collect();
    match segments.as_slice() {
        [a, b] => a.len() == 8 && b.len() == 16 && all_hex(a) && all_hex(b),
        [only] => only.len() == 40 && all_hex(only),
        [a, b, c, d, e] => {
            a.len() == 8
                && b.len() == 4
                && c.len() == 4
                && d.len() == 4
                && e.len() == 12
                && [a, b, c, d, e].iter().all(|s| all_hex(s))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_path() -> &'static Path {
        Path::new("impresario.config")
    }

    fn devices_path() -> &'static Path {
        Path::new("impresario.devices.config")
    }

    #[test]
    fn parses_the_spec_example_with_defaults() {
        let source = "\
project: impresario_tests
timeout_default_ms: 10000
screenshot_on_failure: true
report_output: ./reports
";
        let outcome = parse_config(source, config_path());
        assert!(
            !outcome.diagnostics.has_errors(),
            "{:?}",
            outcome.diagnostics
        );
        let config = outcome.config.expect("config");
        assert_eq!(config.project, "impresario_tests");
        assert_eq!(config.timeout_default_ms, 10_000);
        assert!(config.screenshot_on_failure);
        assert_eq!(config.report_output, PathBuf::from("./reports"));
        assert_eq!(config.plays_dir, PathBuf::from("./plays"));
        assert_eq!(config.scenes_dir, PathBuf::from("./scenes"));
        assert_eq!(config.retries, 0);
        assert_eq!(config.parallel_devices, 4);
        assert_eq!(config.app_reset, AppReset::PerAct);
        assert_eq!(config.poll_interval_ms, 100);
        assert_eq!(config.scroll_max_attempts, 10);
        assert_eq!(config.focus_max_steps, 50);
        assert_eq!(config.agent_start_timeout_ms, 60_000);
        assert_eq!(config.command_grace_ms, 2000);
        assert_eq!(
            config.reporters,
            vec![Reporter::Console, Reporter::Junit, Reporter::Json]
        );
    }

    #[test]
    fn reports_unknown_keys_with_a_hint_and_position() {
        let source = "\
project: demo
timeout_defualt_ms: 1000
screenshot_on_failure: true
report_output: ./reports
";
        let outcome = parse_config(source, config_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        // The typo produces the unknown-key error; the required key is then
        // also missing, and both are reported together.
        assert_eq!(errors.len(), 2);
        let unknown = errors
            .iter()
            .find(|d| d.code == Code::CfgUnknownKey)
            .expect("unknown key error");
        assert!(unknown.message.contains("timeout_defualt_ms"));
        assert!(unknown
            .help
            .as_deref()
            .unwrap_or("")
            .contains("timeout_default_ms"));
        let span = unknown.span.clone().expect("span");
        assert_eq!(span.line, 2);
        assert_eq!(span.col, 1);
        assert!(errors
            .iter()
            .any(|d| d.code == Code::CfgMissingField && d.message.contains("timeout_default_ms")));
    }

    #[test]
    fn collects_every_error_in_one_pass() {
        let source = "\
project: demo
timeout_default_ms: 5
screenshot_on_failure: maybe
report_output: ./reports
retries: 9
";
        let outcome = parse_config(source, config_path());
        assert!(outcome.config.is_none());
        let codes: Vec<u16> = outcome
            .diagnostics
            .errors()
            .map(|d| match d.code {
                Code::CfgInvalidValue => 3,
                _ => 0,
            })
            .collect();
        assert_eq!(outcome.diagnostics.errors().count(), 3, "{codes:?}");
    }

    #[test]
    fn missing_required_keys_are_reported() {
        let source = "timeout_default_ms: 1000\n";
        let outcome = parse_config(source, config_path());
        let missing: Vec<_> = outcome
            .diagnostics
            .errors()
            .filter(|d| d.code == Code::CfgMissingField)
            .collect();
        assert_eq!(missing.len(), 3); // project, screenshot_on_failure, report_output
    }

    #[test]
    fn reporters_subset_and_duplicates() {
        let source = "\
project: demo
timeout_default_ms: 1000
screenshot_on_failure: true
report_output: ./reports
reporters: [console, json, consle, console]
";
        let outcome = parse_config(source, config_path());
        assert_eq!(outcome.diagnostics.errors().count(), 2);
        assert!(outcome
            .diagnostics
            .errors()
            .any(|d| d.message.contains("unknown reporter")));
        assert!(outcome
            .diagnostics
            .errors()
            .any(|d| d.message.contains("duplicate reporter")));
    }

    #[test]
    fn parses_the_spec_devices_example() {
        let source = "\
devices:
  - name: living_room_tv
    platform: webos
    connection:
      transport: ssh
      host: 192.168.1.50
      endpoint: ws://192.168.1.50:9998
    labels: [tv, living_room]
  - name: flagship_android
    platform: android
    connection:
      serial: emulator-5554
    options:
      agent_port: 50061
  - name: shield_tv
    platform: androidtv
    connection:
      transport: tcp
      host: 192.168.1.60
      port: 5555
  - name: test_iphone
    platform: ios
    connection:
      udid: 00008110-001A2345021E001E
  - name: apple_tv
    platform: tvos
    connection:
      udid: 00008110-001B3456021E002F
      endpoint: 192.168.1.70:50062
";
        let outcome = parse_devices(source, devices_path());
        assert!(
            !outcome.diagnostics.has_errors(),
            "{:?}",
            outcome.diagnostics
        );
        let devices = outcome.config.expect("devices");
        assert_eq!(devices.devices.len(), 5);
        let webos = &devices.devices[0];
        assert_eq!(webos.platform, Platform::WebOs);
        assert_eq!(webos.connector_address(), Some("127.0.0.1:50051"));
        assert_eq!(webos.target_endpoint(), "ws://192.168.1.50:9998");
        assert_eq!(
            webos.connection.port_or_default(Platform::WebOs),
            Some(9922)
        );
        assert_eq!(webos.labels, ["tv", "living_room"]);
        let android = &devices.devices[1];
        assert_eq!(android.effective_transport(), TransportKind::Usb);
        assert_eq!(android.effective_agent_port(), Some(50061));
        let ios = &devices.devices[3];
        assert_eq!(ios.effective_agent_port(), Some(50062));
        assert_eq!(ios.effective_transport(), TransportKind::Usb);
    }

    #[test]
    fn duplicate_device_names_are_rejected_at_the_second_entry() {
        let source = "\
devices:
  - name: one
    platform: android
    connection:
      serial: emulator-5554
  - name: one
    platform: android
    connection:
      serial: emulator-5556
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, Code::CfgDuplicateDevice);
        assert_eq!(errors[0].span.as_ref().map(|s| s.line), Some(6));
    }

    #[test]
    fn endpoint_rules_per_platform() {
        assert!(validate_endpoint(Platform::Web, "launch").is_ok());
        assert!(validate_endpoint(Platform::Web, "launch:headless").is_ok());
        assert!(validate_endpoint(Platform::Web, "ws://localhost:9222").is_ok());
        assert!(validate_endpoint(Platform::Web, "emulator-5554").is_err());

        assert!(validate_endpoint(Platform::WebOs, "ws://h:9998").is_ok());
        assert!(validate_endpoint(Platform::WebOs, "wss://h:9998/devtools/page/1").is_ok());
        assert!(validate_endpoint(Platform::WebOs, "ws://h").is_err());
        assert!(validate_endpoint(Platform::WebOs, "launch").is_err());

        assert!(validate_endpoint(Platform::Android, "emulator-5554").is_ok());
        assert!(validate_endpoint(Platform::Android, "192.168.1.60:5555").is_ok());
        assert!(validate_endpoint(Platform::Android, "ab").is_err());
        assert!(validate_endpoint(Platform::Android, "has space").is_err());

        assert!(validate_endpoint(Platform::Ios, "00008110-001A2345021E001E").is_ok());
        assert!(
            validate_endpoint(Platform::Ios, "aabbccddeeff00112233445566778899aabbccdd").is_ok()
        );
        assert!(validate_endpoint(Platform::Ios, "A22F8D4C-1B2E-4B7A-9C3D-112233445566").is_ok());
        assert!(validate_endpoint(Platform::Ios, "nope").is_err());
    }

    #[test]
    fn legacy_device_fields_point_at_migrate() {
        let source = "\
devices:
  - name: phone
    platform: ios
    target_endpoint: 00008110-001A2345021E001E
    app_id: com.example.shop
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert_eq!(
            errors
                .iter()
                .filter(|d| d.code == Code::CfgUnknownKey)
                .count(),
            2
        );
        assert!(errors
            .iter()
            .filter(|d| d.message.contains("target_endpoint") || d.message.contains("app_id"))
            .all(|d| d.help.as_deref().unwrap_or("").contains("migrate")));
    }

    #[test]
    fn connector_address_option_is_web_only() {
        let ok = "\
devices:
  - name: tv
    platform: webos
    connection:
      host: 192.168.1.50
      endpoint: ws://192.168.1.50:9998
    options:
      connector_address: 192.168.1.50:50051
";
        let outcome = parse_devices(ok, devices_path());
        assert!(
            !outcome.diagnostics.has_errors(),
            "{:?}",
            outcome.diagnostics
        );
        let devices = outcome.config.expect("devices");
        assert_eq!(
            devices.devices[0].connector_address(),
            Some("192.168.1.50:50051")
        );

        let bad = "\
devices:
  - name: phone
    platform: android
    connection:
      serial: emulator-5554
    options:
      connector_address: 192.168.1.50:50051
";
        let outcome = parse_devices(bad, devices_path());
        assert_eq!(outcome.diagnostics.errors().count(), 1);
        assert!(outcome
            .diagnostics
            .errors()
            .next()
            .is_some_and(|d| d.message.contains("connector_address")));
    }

    #[test]
    fn tvos_requires_endpoint() {
        let source = "\
devices:
  - name: apple_tv
    platform: tvos
    connection:
      udid: 00008110-001B3456021E002F
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("connection.endpoint"));
        assert!(errors[0].help.as_deref().unwrap_or("").contains("Bonjour"));

        let network = "\
devices:
  - name: apple_tv
    platform: tvos
    connection:
      transport: network
      udid: 00008110-001B3456021E002F
";
        let outcome = parse_devices(network, devices_path());
        assert!(
            !outcome.diagnostics.has_errors(),
            "{:?}",
            outcome.diagnostics
        );
    }

    #[test]
    fn option_applicability_and_values() {
        let source = "\
devices:
  - name: phone
    platform: android
    connection:
      serial: emulator-5554
    options:
      key_dispatch: js
      agent_port: 70000
      clear_app_data: true
";
        let outcome = parse_devices(source, devices_path());
        let messages: Vec<String> = outcome
            .diagnostics
            .errors()
            .map(|d| d.message.clone())
            .collect();
        assert_eq!(messages.len(), 3, "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("key_dispatch")));
        assert!(messages.iter().any(|m| m.contains("agent_port")));
        assert!(messages.iter().any(|m| m.contains("clear_app_data")));
        assert!(outcome
            .diagnostics
            .errors()
            .find(|d| d.message.contains("clear_app_data"))
            .and_then(|d| d.help.as_deref())
            .is_some_and(|help| help.contains("apps file")));
    }

    #[test]
    fn unknown_device_and_option_keys_get_hints() {
        let source = "\
devices:
  - name: phone
    platform: android
    connection:
      serial: emulator-5554
    plataform: android
    options:
      agent_prot: 50061
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().all(|d| d.code == Code::CfgUnknownKey));
        assert!(errors
            .iter()
            .any(|d| { d.help.as_deref().unwrap_or("").contains("agent_port") }));
    }

    #[test]
    fn labels_and_auth_are_validated() {
        let source = "\
devices:
  - name: phone
    platform: android
    connection:
      serial: emulator-5554
    labels: [TV, tv, bad label]
    auth:
      keychain: \"keychain:android\"
";
        let outcome = parse_devices(source, devices_path());
        let messages: Vec<String> = outcome
            .diagnostics
            .errors()
            .map(|d| d.message.clone())
            .collect();
        assert_eq!(messages.len(), 3, "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("unknown auth key")));
        assert!(messages.iter().any(|m| m.contains("invalid label 'TV'")));
        assert!(messages
            .iter()
            .any(|m| m.contains("invalid label 'bad label'")));
    }

    #[test]
    fn device_name_rule() {
        assert!(valid_device_name("living_room_tv"));
        assert!(valid_device_name("a"));
        assert!(valid_device_name("shield-2"));
        assert!(!valid_device_name(""));
        assert!(!valid_device_name("UPPER"));
        assert!(!valid_device_name("_leading"));
        assert!(!valid_device_name(&"a".repeat(64)));
        assert!(valid_device_name(&"a".repeat(63)));
    }

    #[test]
    fn yaml_syntax_errors_carry_a_position() {
        let source = "devices: [\n";
        let outcome = parse_devices(source, devices_path());
        assert_eq!(outcome.diagnostics.errors().count(), 1);
        assert!(outcome
            .diagnostics
            .errors()
            .next()
            .is_some_and(|d| d.span.is_some()));
    }

    #[test]
    fn android_tcp_endpoint_is_host_and_default_port() {
        let source = "\
devices:
  - name: phone
    platform: android
    connection:
      transport: tcp
      host: 192.168.1.5
";
        let device = parse_devices(source, devices_path())
            .config
            .expect("tcp device")
            .devices
            .remove(0);
        assert_eq!(device.effective_transport().as_str(), "tcp");
        assert_eq!(device.target_endpoint(), "192.168.1.5:5555");
    }

    #[test]
    fn android_wireless_serial_resolves_to_tcp() {
        let source = "\
devices:
  - name: phone
    platform: android
    connection:
      serial: 192.168.1.5:39373
";
        let device = parse_devices(source, devices_path())
            .config
            .expect("wireless device")
            .devices
            .remove(0);
        assert_eq!(device.effective_transport().as_str(), "tcp");
        assert_eq!(device.target_endpoint(), "192.168.1.5:39373");
    }
}
