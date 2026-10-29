//! Configuration files (spec section 4): `impresario.config` and
//! `impresario.devices.config`.
//!
//! Parsing is two-stage on purpose: the YAML tree is validated key by key so
//! that *all* errors in a file are collected (spec section 4), each with a
//! file/line/column and a "did you mean" hint for near-miss keys; only when
//! the tree is clean is it deserialized into the typed structs below.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};

use crate::diag::{suggest, Code, Diagnostic, Diagnostics};
use crate::home::Home;
use crate::platforms::{DeviceType, Platform};
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
    /// Tri-state probe policy (TASK-3 D-26): `fail` (default) or `warn`.
    #[serde(default)]
    pub unverifiable_policy: crate::probe::UnverifiablePolicy,
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

/// Internal connection overrides used only by tests and the interactive
/// wizard. They are deliberately not part of the serialized devices file: the
/// public schema is `name?`, `type` and `ip`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConnection {
    #[serde(default)]
    pub transport: TransportKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

impl DeviceConnection {
    /// The hardcoded protocol port for a platform. Web is the remote Chrome
    /// DevTools port; iOS/tvOS use RemotePairing discovery, not a fixed port.
    #[must_use]
    pub fn port_or_default(&self, platform: Platform) -> Option<u16> {
        self.port.or(match platform {
            Platform::WebOs => Some(9922),
            Platform::Tizen => Some(26101),
            Platform::Android | Platform::AndroidTv => Some(5555),
            Platform::Web => Some(9222),
            Platform::Ios | Platform::Tvos => None,
        })
    }
}

/// Internal credential references. Device files no longer expose credentials;
/// the fields remain for test fixtures and the wizard's private state.
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

/// Internal transport tuning. The public devices file does not expose these;
/// defaults are chosen programmatically and ports are allocated at run time.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceOptions {
    #[serde(default, skip_serializing_if = "CdpBackend::is_default")]
    pub cdp_backend: CdpBackend,
    #[serde(default, skip_serializing_if = "KeyDispatch::is_default")]
    pub key_dispatch: KeyDispatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_match: Option<String>,
    /// Test-only override; production always allocates a fresh connector port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_address: Option<String>,
    /// Test-only override; production always allocates a fresh agent port.
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

fn default_platform() -> Platform {
    Platform::Web
}

/// One device entry of a devices file. The only public fields are an optional
/// label (`name`), the exact product `type`, and its `ip`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    /// Optional label used by actors. When omitted it is derived from type+IP.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The exact product type (`web`, `webOS`, `tizenOS`, ...).
    #[serde(rename = "type")]
    pub device_type: DeviceType,
    /// The device's IP address. `127.0.0.1` means the local host.
    pub ip: String,
    /// Internal platform family, filled by the parser from `device_type`.
    #[serde(skip, default = "default_platform")]
    pub platform: Platform,
    #[serde(skip)]
    pub connection: DeviceConnection,
    #[serde(skip)]
    pub auth: DeviceAuth,
    #[serde(skip)]
    pub labels: Vec<String>,
    #[serde(skip)]
    pub options: DeviceOptions,
}

impl DeviceConfig {
    /// Build a device for tests and internal callers.
    #[must_use]
    pub fn new(name: impl Into<String>, platform: Platform, ip: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            device_type: DeviceType::from_platform(platform),
            ip: ip.into(),
            platform,
            connection: DeviceConnection::default(),
            auth: DeviceAuth::default(),
            labels: Vec::new(),
            options: DeviceOptions::default(),
        }
    }

    /// The exact config type.
    #[must_use]
    pub fn device_type(&self) -> DeviceType {
        self.device_type
    }

    /// The IP without IPv6 brackets.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.ip
    }

    /// The IP formatted for `host:port`.
    #[must_use]
    pub fn socket_host(&self) -> String {
        if self.ip.contains(':') {
            format!("[{}]", self.ip)
        } else {
            self.ip.clone()
        }
    }

    /// The hardcoded protocol port for this type.
    #[must_use]
    pub fn protocol_port(&self) -> Option<u16> {
        self.connection.port_or_default(self.platform)
    }

    /// True when the device is the local host (`127.0.0.1`, `::1` or `localhost`).
    #[must_use]
    pub fn is_local(&self) -> bool {
        let text = self.ip.trim_matches(['[', ']']);
        text.eq_ignore_ascii_case("localhost")
            || text.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }

    /// The transport is derived entirely from the device type.
    #[must_use]
    pub fn effective_transport(&self) -> TransportKind {
        match self.platform {
            Platform::Web if self.is_local() => TransportKind::Launch,
            Platform::Web => TransportKind::Cdp,
            Platform::WebOs => TransportKind::Ssh,
            Platform::Tizen => TransportKind::Sdb,
            Platform::Android | Platform::AndroidTv => TransportKind::Tcp,
            Platform::Ios | Platform::Tvos => TransportKind::Network,
        }
    }

    /// The endpoint the agent handshake reports. WebOS/Tizen replace this with
    /// the forwarded inspector URL during prepare; native devices report their
    /// discovered serial/UDID at run time.
    #[must_use]
    pub fn target_endpoint(&self) -> String {
        match self.platform {
            Platform::Web if self.is_local() => "launch".to_string(),
            Platform::Web => format!("ws://{}:9222", self.socket_host()),
            Platform::WebOs | Platform::Tizen => String::new(),
            Platform::Android | Platform::AndroidTv => {
                format!("{}:5555", self.socket_host())
            }
            Platform::Ios | Platform::Tvos => self.ip.clone(),
        }
    }

    /// The connector listen address. Production uses `None` and the transport
    /// allocates a free port; tests may pin one internally.
    #[must_use]
    pub fn connector_address(&self) -> Option<&str> {
        self.options.connector_address.as_deref()
    }

    /// The device-side agent port. Production uses `None` and the transport
    /// allocates a free port; tests may pin one internally.
    #[must_use]
    pub fn effective_agent_port(&self) -> Option<u16> {
        self.options.agent_port
    }

    /// Human-readable one-line connection summary for `device list`.
    #[must_use]
    pub fn connection_summary(&self) -> String {
        let transport = self.effective_transport().as_str();
        match self.platform {
            Platform::Web => format!("{transport} {}", self.target_endpoint()),
            Platform::WebOs | Platform::Tizen => {
                let port = self
                    .protocol_port()
                    .map_or_else(|| "-".to_string(), |p| p.to_string());
                format!("{transport} {}:{port}", self.socket_host())
            }
            Platform::Android | Platform::AndroidTv => {
                format!("{transport} {}:5555", self.socket_host())
            }
            Platform::Ios | Platform::Tvos => {
                format!("{transport} {}", self.ip)
            }
        }
    }
}

/// `impresario.devices.config` (workspace) and `~/.impresario/devices.yaml`
/// (global), TASK-2 3.3/4.3.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicesConfig {
    pub devices: Vec<DeviceConfig>,
    /// Named actor groups. Members are device names or `@group` references.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, Vec<String>>,
}

/// Failure while expanding an actor or `@group`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorResolveError {
    /// A bare name is neither a configured device nor (with `@`) a group.
    Unknown(String),
    /// A `@group` membership cycle; the path ends with the repeated group.
    Cycle(Vec<String>),
}

impl DevicesConfig {
    /// Look a device up by name.
    #[must_use]
    pub fn device(&self, name: &str) -> Option<&DeviceConfig> {
        self.devices.iter().find(|device| device.name == name)
    }

    /// Expand one actor reference to device names. Plain names are devices;
    /// `@name` is a group and expands recursively in deterministic order.
    pub fn expand_actor(&self, reference: &str) -> Result<BTreeSet<String>, ActorResolveError> {
        expand_actor_ref(&self.devices, &self.groups, reference)
    }

    /// Expand several actor references, de-duplicated.
    pub fn expand_actors<'a>(
        &self,
        references: impl IntoIterator<Item = &'a str>,
    ) -> Result<BTreeSet<String>, ActorResolveError> {
        expand_actor_refs(&self.devices, &self.groups, references)
    }

    /// Diagnostic-free validation of group references and cycles. Used by the
    /// merged loader; parse-time checks only cover YAML shape.
    pub fn validate_groups(&self) -> Vec<(Code, String)> {
        let mut errors = Vec::new();
        for group in self.groups.keys() {
            let reference = format!("@{group}");
            match self.expand_actor(&reference) {
                Ok(_) => {}
                Err(ActorResolveError::Unknown(name)) => {
                    errors.push((
                        Code::CfgUnknownActor,
                        format!("group '@{group}' references unknown actor '{name}'"),
                    ));
                }
                Err(ActorResolveError::Cycle(path)) => {
                    errors.push((
                        Code::CfgGroupCycle,
                        format!(
                            "group '@{group}' has a membership cycle: {}",
                            path.join(" -> ")
                        ),
                    ));
                }
            }
        }
        errors
    }
}

/// Expand one actor reference against a merged device/group view.
pub fn expand_actor_ref(
    devices: &[DeviceConfig],
    groups: &BTreeMap<String, Vec<String>>,
    reference: &str,
) -> Result<BTreeSet<String>, ActorResolveError> {
    let mut out = BTreeSet::new();
    expand_actor_into(devices, groups, reference, &mut out, &mut Vec::new())?;
    Ok(out)
}

/// Expand several actor references, de-duplicated.
pub fn expand_actor_refs<'a>(
    devices: &[DeviceConfig],
    groups: &BTreeMap<String, Vec<String>>,
    references: impl IntoIterator<Item = &'a str>,
) -> Result<BTreeSet<String>, ActorResolveError> {
    let mut out = BTreeSet::new();
    let mut stack = Vec::new();
    for reference in references {
        expand_actor_into(devices, groups, reference, &mut out, &mut stack)?;
    }
    Ok(out)
}

fn expand_actor_into(
    devices: &[DeviceConfig],
    groups: &BTreeMap<String, Vec<String>>,
    reference: &str,
    out: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Result<(), ActorResolveError> {
    if let Some(group) = reference.strip_prefix('@') {
        if let Some(position) = stack.iter().position(|item| item == group) {
            let mut cycle: Vec<String> = stack.get(position..).unwrap_or(&[]).to_vec();
            cycle.push(group.to_string());
            return Err(ActorResolveError::Cycle(cycle));
        }
        let Some(members) = groups.get(group) else {
            return Err(ActorResolveError::Unknown(reference.to_string()));
        };
        stack.push(group.to_string());
        for member in members {
            expand_actor_into(devices, groups, member, out, stack)?;
        }
        stack.pop();
        return Ok(());
    }
    if devices.iter().any(|device| device.name == reference) {
        out.insert(reference.to_string());
        Ok(())
    } else {
        Err(ActorResolveError::Unknown(reference.to_string()))
    }
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
    "unverifiable_policy",
];

const DEVICE_KEYS: &[&str] = &["name", "type", "ip"];

const DEVICE_TYPES: &[&str] = &[
    "web",
    "webOS",
    "tizenOS",
    "android",
    "androidtv",
    "iOS",
    "tvOS",
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
    if let Some(policy) = map.get("unverifiable_policy") {
        let valid = policy
            .as_str()
            .is_some_and(|s| matches!(s, "fail" | "warn"));
        if !valid {
            let mut diag = invalid_value(
                &lines,
                path,
                "unverifiable_policy",
                "expected one of: fail, warn",
            );
            if let Some(text) = policy.as_str() {
                if let Some(hint) = suggest(text, &["fail", "warn"]) {
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
            Err(_) => probe = probe.parent()?,
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
        if key != "devices" && key != "groups" {
            let mut diag = Diagnostic::error(Code::CfgUnknownKey, format!("unknown key '{key}'"))
                .with_span(span_of_top(&lines, key, path))
                .with_help("the devices file has top-level 'devices' and optional 'groups' keys");
            if let Some(hint) = suggest(key, &["devices", "groups"]) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }
    if let Some(groups_value) = map.get("groups") {
        match groups_value.as_mapping() {
            None => diagnostics.push(invalid_value(
                &lines,
                path,
                "groups",
                "expected a mapping of group name to actor list",
            )),
            Some(groups) => {
                for (name, members) in groups {
                    let Some(name) = name.as_str() else {
                        diagnostics.push(invalid_value(
                            &lines,
                            path,
                            "groups",
                            "group names must be strings",
                        ));
                        continue;
                    };
                    if !valid_device_name(name) {
                        diagnostics.push(invalid_value(
                            &lines,
                            path,
                            "groups",
                            format!("invalid group name '{name}'"),
                        ));
                    }
                    let Some(members) = members.as_sequence() else {
                        diagnostics.push(invalid_value(
                            &lines,
                            path,
                            "groups",
                            format!("group '{name}' needs a list of actor references"),
                        ));
                        continue;
                    };
                    for member in members {
                        let Some(member) = member.as_str() else {
                            diagnostics.push(invalid_value(
                                &lines,
                                path,
                                "groups",
                                format!("group '{name}' members must be strings"),
                            ));
                            continue;
                        };
                        if !valid_actor_ref(member) {
                            diagnostics.push(invalid_value(
                                &lines,
                                path,
                                "groups",
                                format!("group '{name}' has invalid actor reference '{member}'"),
                            ));
                        }
                    }
                }
            }
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
        Ok(mut config) => {
            for device in &mut config.devices {
                device.platform = device.device_type.platform();
                if let Ok(ip) = device.ip.trim().parse::<IpAddr>() {
                    device.ip = ip.to_string();
                }
                if device.name.is_empty() {
                    device.name = derived_device_name(device.device_type, &device.ip);
                }
                // The public schema has no connection block. These fields are
                // private defaults used by transports that still accept a
                // test-only override.
                device.connection.host = Some(device.ip.clone());
                device.connection.transport = TransportKind::Auto;
                device.connection.port = None;
                device.connection.serial = None;
                device.connection.udid = None;
                device.connection.endpoint = None;
            }
            ConfigOutcome {
                config: Some(config),
                diagnostics,
            }
        }
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

/// Test-only transport overrides kept outside the public devices schema.
///
/// Integration tests need to point a device at an in-process mock agent, but
/// the public schema intentionally has no transport/port knobs. The test
/// harness writes a sibling sidecar file and opts in with an environment
/// variable; release builds never read it.
#[cfg(debug_assertions)]
fn apply_test_device_options(config: &mut DevicesConfig, devices_path: &Path) {
    if std::env::var_os("IMPRESARIO_TEST_DEVICE_OPTIONS").is_none() {
        return;
    }
    let Some(dir) = devices_path.parent() else {
        return;
    };
    let sidecar = dir.join("impresario.devices.test-options.yaml");
    let Ok(source) = std::fs::read_to_string(sidecar) else {
        return;
    };
    #[derive(Deserialize)]
    struct TestOptions {
        devices: BTreeMap<String, DeviceOptions>,
    }
    let Ok(options) = serde_yaml_ng::from_str::<TestOptions>(&source) else {
        return;
    };
    for device in &mut config.devices {
        if let Some(overrides) = options.devices.get(&device.name) {
            device.options = overrides.clone();
        }
    }
}

#[cfg(not(debug_assertions))]
fn apply_test_device_options(_config: &mut DevicesConfig, _devices_path: &Path) {}

/// Read and parse `impresario.devices.config` from disk.
#[must_use]
pub fn load_devices(path: &Path) -> ConfigOutcome<DevicesConfig> {
    match std::fs::read_to_string(path) {
        Ok(source) => {
            let mut outcome = parse_devices(&source, path);
            if let Some(config) = outcome.config.as_mut() {
                apply_test_device_options(config, path);
            }
            outcome
        }
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
    let mut merged_groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let global_path = Home::resolve().ok().map(|home| home.devices_path());
    if let Some(path) = global_path.as_deref() {
        if let Some(source) = read_optional_layer(path, &mut diagnostics) {
            let outcome = parse_devices(&source, path);
            diagnostics.extend(outcome.diagnostics);
            if let Some(config) = outcome.config {
                merge_devices(&mut merged, &config.devices);
                merge_groups(&mut merged_groups, config.groups);
            }
        }
    }
    if let Some(source) = read_optional_layer(workspace_path, &mut diagnostics) {
        let outcome = parse_devices(&source, workspace_path);
        diagnostics.extend(outcome.diagnostics);
        if let Some(config) = outcome.config {
            merge_devices(&mut merged, &config.devices);
            merge_groups(&mut merged_groups, config.groups);
        }
    }
    let mut merged_config = DevicesConfig {
        devices: merged,
        groups: merged_groups,
    };
    apply_test_device_options(&mut merged_config, workspace_path);
    if !diagnostics.has_errors() {
        for (code, message) in merged_config.validate_groups() {
            diagnostics.push(
                Diagnostic::error(code, message).with_help(
                    "groups may reference devices and '@groups' across both config layers",
                ),
            );
        }
    }
    let config = (!diagnostics.has_errors()).then_some(merged_config);
    ConfigOutcome {
        config,
        diagnostics,
    }
}

fn merge_groups(merged: &mut BTreeMap<String, Vec<String>>, layer: BTreeMap<String, Vec<String>>) {
    for (name, members) in layer {
        merged.insert(name, members);
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
                .with_span(key_span("type")),
        );
        return;
    };

    for key in map.keys().filter_map(Value::as_str) {
        if !DEVICE_KEYS.contains(&key) {
            let mut diag =
                Diagnostic::error(Code::CfgUnknownKey, format!("unknown device key '{key}'"))
                    .with_span(key_span(key));
            diag = diag.with_help(match key {
                "platform" => {
                    "use 'type' with one of: web, webOS, tizenOS, android, androidtv, iOS, tvOS"
                }
                "connection" | "auth" | "options" | "labels" => {
                    "device files only take name (optional), type and ip; transport, ports and credentials are discovered from the IP"
                }
                "target_endpoint" | "app_id" | "app_url" | "agent_address" | "connector" => {
                    "the legacy device schema was removed; use name, type and ip"
                }
                _ => "the devices file accepts name (optional), type and ip",
            });
            if let Some(hint) = suggest(key, DEVICE_KEYS) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }

    // name (optional label)
    let mut parsed_name: Option<String> = None;
    match map.get("name") {
        Some(value) => match value.as_str() {
            Some(name) if valid_device_name(name) => parsed_name = Some(name.to_string()),
            Some(name) => diagnostics.push(invalid_here(
                "name",
                format!("'{name}' must match ^[a-z0-9][a-z0-9_-]{{0,62}}$"),
            )),
            None => diagnostics.push(invalid_here("name", "expected a string".to_string())),
        },
        None => {}
    }

    // type (exact spelling)
    let mut parsed_type: Option<DeviceType> = None;
    match map.get("type").and_then(Value::as_str) {
        Some(text) => match text.parse::<DeviceType>() {
            Ok(device_type) => parsed_type = Some(device_type),
            Err(()) => {
                let mut diag = invalid_here("type", format!("unknown device type '{text}'"));
                if let Some(hint) = suggest(text, DEVICE_TYPES) {
                    diag = diag.with_help(hint);
                } else {
                    diag = diag.with_help(
                        "expected one of: web, webOS, tizenOS, android, androidtv, iOS, tvOS",
                    );
                }
                diagnostics.push(diag);
            }
        },
        None => diagnostics.push(
            Diagnostic::error(Code::CfgMissingField, "missing required key 'type'")
                .with_span(key_span("type")),
        ),
    }

    // ip (an address, not a hostname)
    let mut parsed_ip: Option<String> = None;
    match map.get("ip").and_then(Value::as_str) {
        Some(text) => match text.trim().parse::<IpAddr>() {
            Ok(ip) => parsed_ip = Some(ip.to_string()),
            Err(_) => diagnostics.push(invalid_here(
                "ip",
                format!("'{text}' is not an IPv4 or IPv6 address"),
            )),
        },
        None => diagnostics.push(
            Diagnostic::error(Code::CfgMissingField, "missing required key 'ip'")
                .with_span(key_span("ip")),
        ),
    }

    if let (Some(device_type), Some(ip)) = (parsed_type, parsed_ip) {
        let name = parsed_name.unwrap_or_else(|| derived_device_name(device_type, &ip));
        if !seen_names.insert(name.clone()) {
            diagnostics.push(
                Diagnostic::error(
                    Code::CfgDuplicateDevice,
                    format!("duplicate device name '{name}'"),
                )
                .with_span(key_span("name"))
                .with_help("give one of the devices an explicit name"),
            );
        }
    }
}

fn derived_device_name(device_type: DeviceType, ip: &str) -> String {
    let host = ip.replace([':', '.'], "-");
    format!("{}-{host}", device_type.slug())
}

/// An actor reference is a device/group name, optionally prefixed with `@`
/// to mark a group explicitly.
#[must_use]
pub fn valid_actor_ref(name: &str) -> bool {
    valid_device_name(name.strip_prefix('@').unwrap_or(name))
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
        _ => simulator_udid(text),
    }
}

/// An RFC 4122 UUID, which is how CoreSimulator names simulators.
///
/// Physical iOS/tvOS UDIDs use the 8-16 or 40-hex forms above, so this
/// distinction is how config and transport decide whether loopback applies.
#[must_use]
pub fn simulator_udid(text: &str) -> bool {
    fn all_hex(s: &str) -> bool {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
    }
    let segments: Vec<&str> = text.split('-').collect();
    matches!(
        segments.as_slice(),
        [a, b, c, d, e]
            if a.len() == 8
                && b.len() == 4
                && c.len() == 4
                && d.len() == 4
                && e.len() == 12
                && [a, b, c, d, e].iter().all(|s| all_hex(s))
    )
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
    fn parses_the_minimal_devices_schema() {
        let source = "\
devices:
  - name: living_room_tv
    type: webOS
    ip: 192.168.1.50
  - type: android
    ip: 192.168.1.60
  - name: shield_tv
    type: androidtv
    ip: 192.168.1.61
  - type: iOS
    ip: 192.168.1.70
  - name: apple_tv
    type: tvOS
    ip: fd00::1
  - type: tizenOS
    ip: 192.168.1.80
  - type: web
    ip: 127.0.0.1
";
        let outcome = parse_devices(source, devices_path());
        assert!(
            !outcome.diagnostics.has_errors(),
            "{:?}",
            outcome.diagnostics
        );
        let devices = outcome.config.expect("devices");
        assert_eq!(devices.devices.len(), 7);
        let webos = &devices.devices[0];
        assert_eq!(webos.name, "living_room_tv");
        assert_eq!(webos.device_type(), DeviceType::WebOs);
        assert_eq!(webos.platform, Platform::WebOs);
        assert_eq!(webos.ip, "192.168.1.50");
        assert_eq!(webos.host(), "192.168.1.50");
        assert_eq!(webos.protocol_port(), Some(9922));
        assert_eq!(webos.effective_transport(), TransportKind::Ssh);
        assert_eq!(webos.connection.host.as_deref(), Some("192.168.1.50"));
        assert_eq!(webos.connection.port, None);
        assert_eq!(webos.connection.serial, None);
        assert_eq!(webos.connection.udid, None);
        assert_eq!(webos.connection.endpoint, None);
        assert_eq!(webos.connection.transport, TransportKind::Auto);

        let android = &devices.devices[1];
        assert_eq!(android.name, "android-192-168-1-60");
        assert_eq!(android.effective_transport(), TransportKind::Tcp);
        assert_eq!(android.target_endpoint(), "192.168.1.60:5555");

        let tizen = &devices.devices[5];
        assert_eq!(tizen.protocol_port(), Some(26101));
        assert_eq!(tizen.effective_transport(), TransportKind::Sdb);

        let apple = &devices.devices[4];
        assert_eq!(apple.name, "apple_tv");
        assert_eq!(apple.socket_host(), "[fd00::1]");
        assert_eq!(apple.effective_transport(), TransportKind::Network);
        assert_eq!(apple.connection.host.as_deref(), Some("fd00::1"));

        let web = &devices.devices[6];
        assert_eq!(web.effective_transport(), TransportKind::Launch);
        assert_eq!(web.target_endpoint(), "launch");
    }

    #[test]
    fn duplicate_device_names_are_rejected_at_the_second_entry() {
        let source = "\
devices:
  - name: one
    type: android
    ip: 192.168.1.60
  - name: one
    type: android
    ip: 192.168.1.61
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, Code::CfgDuplicateDevice);
        assert_eq!(errors[0].span.as_ref().map(|s| s.line), Some(5));
    }

    #[test]
    fn derived_names_use_the_type_slug_and_ip() {
        let source = "\
devices:
  - type: webOS
    ip: 192.168.1.50
  - type: tvOS
    ip: fd00::1
";
        let devices = parse_devices(source, devices_path())
            .config
            .expect("devices");
        assert_eq!(devices.devices[0].name, "webos-192-168-1-50");
        assert_eq!(devices.devices[1].name, "tvos-fd00--1");
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
    fn legacy_device_fields_are_rejected() {
        let source = "\
devices:
  - name: phone
    platform: ios
    target_endpoint: 00008110-001A2345021E001E
    app_id: com.example.shop
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 5, "{errors:?}");
        assert_eq!(
            errors
                .iter()
                .filter(|d| d.code == Code::CfgUnknownKey)
                .count(),
            3
        );
        assert_eq!(
            errors
                .iter()
                .filter(|d| d.code == Code::CfgMissingField)
                .count(),
            2
        );
        assert!(errors
            .iter()
            .all(|d| !d.help.as_deref().unwrap_or("").contains("migrate")));
        assert!(errors.iter().any(|d| d.message.contains("target_endpoint")
            && d.help.as_deref().unwrap_or("").contains("removed")));
    }

    #[test]
    fn connection_auth_options_and_labels_are_rejected() {
        let source = "\
devices:
  - name: tv
    type: webOS
    ip: 192.168.1.50
    connection:
      host: 192.168.1.50
    auth:
      keychain: keychain:webos
    options:
      connector_address: 192.168.1.50:50051
    labels: [tv]
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 4, "{errors:?}");
        for key in ["connection", "auth", "options", "labels"] {
            assert!(
                errors.iter().any(|d| d.message.contains(key)),
                "missing rejection for {key}: {errors:?}"
            );
        }
        assert!(errors
            .iter()
            .all(|d| d.help.as_deref().unwrap_or("").contains("type and ip")));
    }

    #[test]
    fn exact_type_spellings_are_enforced() {
        let source = "\
devices:
  - type: webos
    ip: 192.168.1.50
  - type: tizen
    ip: 192.168.1.51
  - type: ios
    ip: 192.168.1.52
  - type: tvos
    ip: 192.168.1.53
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(errors.iter().all(|d| d.code == Code::CfgInvalidValue));
        assert!(errors
            .iter()
            .all(|d| d.message.contains("unknown device type")));
        assert!(errors
            .iter()
            .any(|d| d.help.as_deref().unwrap_or("").contains("webOS")));
    }

    #[test]
    fn unknown_device_keys_get_hints() {
        let source = "\
devices:
  - type: android
    ip: 192.168.1.60
    nmae: phone
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].code, Code::CfgUnknownKey);
        assert!(errors[0].help.as_deref().unwrap_or("").contains("name"));
    }

    #[test]
    fn ip_must_be_an_address() {
        let source = "\
devices:
  - type: web
    ip: example.test
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].message.contains("not an IPv4 or IPv6 address"));
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
}
