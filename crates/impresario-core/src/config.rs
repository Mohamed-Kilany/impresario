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
}

/// Per-device `options` map (spec 4.3); every option is optional and carries
/// the documented default.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceOptions {
    #[serde(default)]
    pub cdp_backend: CdpBackend,
    #[serde(default)]
    pub key_dispatch: KeyDispatch,
    #[serde(default)]
    pub page_match: Option<String>,
    #[serde(default)]
    pub clear_app_data: bool,
    /// Host path of the app under test (`.app` bundle or `.ipa`); Apple
    /// platforms only. With `clear_app_data` the Apple transport reinstalls it
    /// so every test starts from a clean container (I-84).
    #[serde(default)]
    pub app_path: Option<String>,
    #[serde(default)]
    pub agent_port: Option<u16>,
    #[serde(default)]
    pub animations_disabled: bool,
}

/// One device entry of `impresario.devices.config` (spec 4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    pub name: String,
    pub platform: Platform,
    pub target_endpoint: String,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub app_url: Option<String>,
    #[serde(default)]
    pub agent_address: Option<String>,
    #[serde(default)]
    pub connector: Option<String>,
    #[serde(default)]
    pub options: DeviceOptions,
}

impl DeviceConfig {
    /// The agent address: `agent_address`, else the `connector` alias.
    #[must_use]
    pub fn agent_address(&self) -> Option<&str> {
        self.agent_address.as_deref().or(self.connector.as_deref())
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

    /// Default agent address for platforms that have one (spec 4.2).
    #[must_use]
    pub fn default_agent_address(&self) -> Option<&'static str> {
        match self.platform {
            Platform::Web | Platform::WebOs | Platform::Tizen => Some("127.0.0.1:50051"),
            _ => None,
        }
    }
}

/// `impresario.devices.config` (spec 4.2).
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
    "target_endpoint",
    "app_id",
    "app_url",
    "agent_address",
    "connector",
    "options",
];

const OPTION_KEYS: &[&str] = &[
    "cdp_backend",
    "key_dispatch",
    "page_match",
    "clear_app_data",
    "app_path",
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

fn span_of(lines: &[&str], key: &str, from: usize, until: usize, file: &Path) -> Option<Span> {
    find_key(lines, key, from, until)
        .map(|(line, col)| Span::new(file, line, col, key.chars().count()))
}

/// The 0-based line index of the top-level `key`, if present.
fn top_level_line(lines: &[&str], key: &str) -> Option<usize> {
    let needle = format!("{key}:");
    lines.iter().position(|line| {
        line.strip_prefix(&needle)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '\r']))
    })
}

/// Ranges `(first_line, last_line)` (0-based, inclusive) of each `- ` list
/// item that follows the `devices:` line.
fn device_ranges(lines: &[&str], devices_line: usize) -> Vec<(usize, usize)> {
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
            if let Some(hint) = suggest(key, DEVICE_KEYS) {
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

    // target_endpoint
    match map.get("target_endpoint").and_then(Value::as_str) {
        Some(endpoint) => {
            if let Some(platform) = parsed_platform {
                if let Err(reason) = validate_endpoint(platform, endpoint) {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::CfgBadEndpoint,
                            format!("invalid target_endpoint for platform '{platform}': {reason}"),
                        )
                        .with_span(key_span("target_endpoint"))
                        .with_help(endpoint_help(platform)),
                    );
                }
            }
        }
        None => diagnostics.push(
            Diagnostic::error(
                Code::CfgMissingField,
                "missing required key 'target_endpoint'",
            )
            .with_span(key_span("target_endpoint")),
        ),
    }

    // Platform-conditioned fields.
    if let Some(platform) = parsed_platform {
        let needs_app_id = platform.is_android_family() || platform.is_apple_family();
        if needs_app_id && map.get("app_id").and_then(Value::as_str).is_none() {
            diagnostics.push(
                Diagnostic::error(
                    Code::CfgMissingField,
                    format!("missing required key 'app_id' for platform '{platform}'"),
                )
                .with_span(key_span("name"))
                .with_help("android/tv/ios/tvos devices need the package name or bundle id"),
            );
        }
        if !needs_app_id && map.get("app_id").is_some() {
            diagnostics.push(invalid_here(
                "app_id",
                format!("'app_id' is not valid for platform '{platform}'"),
            ));
        }
        if !platform.is_web_runtime() && map.get("app_url").is_some() {
            diagnostics.push(invalid_here(
                "app_url",
                format!("'app_url' is only valid for web, webos and tizen (not '{platform}')"),
            ));
        }
        if !platform.is_web_runtime() && map.get("connector").is_some() {
            diagnostics.push(invalid_here(
                "connector",
                format!("'connector' is only valid for web, webos and tizen (not '{platform}')"),
            ));
        }
        if map.get("connector").is_some() && map.get("agent_address").is_some() {
            diagnostics.push(invalid_here(
                "connector",
                "'connector' is an alias of 'agent_address'; set only one".to_string(),
            ));
        }
        if platform == Platform::Tvos && map.get("agent_address").is_none() {
            diagnostics.push(
                Diagnostic::error(
                    Code::CfgMissingField,
                    "missing required key 'agent_address' for platform 'tvos'",
                )
                .with_span(key_span("name"))
                .with_help(
                    "tvOS has no USB auto-forwarding; Bonjour discovery is not implemented \
                     (see docs/STATUS.md) so agent_address is required",
                ),
            );
        }
        if let Some(address) = map
            .get("agent_address")
            .or_else(|| map.get("connector"))
            .and_then(Value::as_str)
        {
            if !valid_host_port(address) {
                let key = if map.get("agent_address").is_some() {
                    "agent_address"
                } else {
                    "connector"
                };
                diagnostics.push(invalid_here(key, format!("'{address}' is not host:port")));
            }
        }
        if let Some(url) = map.get("app_url").and_then(Value::as_str) {
            if !(url.starts_with("http://")
                || url.starts_with("https://")
                || url.starts_with("file://"))
            {
                diagnostics.push(invalid_here(
                    "app_url",
                    "expected an http://, https:// or file:// URL. Do not pass an app id here"
                        .to_string(),
                ));
            }
        }
        validate_options(diagnostics, lines, path, map, platform, from, until);
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
            if let Some(hint) = suggest(key, OPTION_KEYS) {
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

    if let Some(value) = options.get("cdp_backend") {
        if let Some(reason) = platform_problem(
            "cdp_backend",
            &[Platform::Web, Platform::WebOs, Platform::Tizen],
        ) {
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
        if let Some(reason) = platform_problem(
            "key_dispatch",
            &[Platform::Web, Platform::WebOs, Platform::Tizen],
        ) {
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
        if let Some(reason) = platform_problem("page_match", &[Platform::WebOs, Platform::Tizen]) {
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
    if let Some(value) = options.get("clear_app_data") {
        if let Some(reason) = platform_problem(
            "clear_app_data",
            &[
                Platform::Android,
                Platform::AndroidTv,
                Platform::Ios,
                Platform::Tvos,
            ],
        ) {
            invalid(diagnostics, "clear_app_data", reason);
        }
        if value.as_bool().is_none() {
            invalid(
                diagnostics,
                "clear_app_data",
                "expected true or false".to_string(),
            );
        }
    }
    if let Some(value) = options.get("app_path") {
        if let Some(reason) = platform_problem("app_path", &[Platform::Ios, Platform::Tvos]) {
            invalid(diagnostics, "app_path", reason);
        }
        if value.as_str().is_none_or(|s| s.trim().is_empty()) {
            invalid(
                diagnostics,
                "app_path",
                "expected a path to the app under test (.app bundle or .ipa)".to_string(),
            );
        }
    }
    if let Some(value) = options.get("agent_port") {
        if let Some(reason) = platform_problem(
            "agent_port",
            &[
                Platform::Android,
                Platform::AndroidTv,
                Platform::Ios,
                Platform::Tvos,
            ],
        ) {
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
    target_endpoint: ws://192.168.1.50:9998
    app_url: http://localhost:8080/index.html
  - name: flagship_android
    platform: android
    target_endpoint: emulator-5554
    app_id: com.example.shop
  - name: shield_tv
    platform: androidtv
    target_endpoint: 192.168.1.60:5555
    app_id: com.example.shop.tv
  - name: test_iphone
    platform: ios
    target_endpoint: 00008110-001A2345021E001E
    app_id: com.example.shop
  - name: apple_tv
    platform: tvos
    target_endpoint: 00008110-001B3456021E002F
    agent_address: 192.168.1.70:50062
    app_id: com.example.shop.tv
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
        assert_eq!(webos.agent_address(), None);
        assert_eq!(
            webos.app_url.as_deref(),
            Some("http://localhost:8080/index.html")
        );
        let android = &devices.devices[1];
        assert_eq!(android.effective_agent_port(), Some(50061));
        let ios = &devices.devices[3];
        assert_eq!(ios.effective_agent_port(), Some(50062));
    }

    #[test]
    fn duplicate_device_names_are_rejected_at_the_second_entry() {
        let source = "\
devices:
  - name: one
    platform: android
    target_endpoint: emulator-5554
    app_id: com.example.one
  - name: one
    platform: android
    target_endpoint: emulator-5556
    app_id: com.example.one
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
    fn app_id_required_for_native_platforms() {
        let source = "\
devices:
  - name: phone
    platform: ios
    target_endpoint: 00008110-001A2345021E001E
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, Code::CfgMissingField);
        assert!(errors[0].message.contains("app_id"));
    }

    #[test]
    fn app_id_not_allowed_on_web_runtimes() {
        let source = "\
devices:
  - name: site
    platform: web
    target_endpoint: launch
    app_id: com.example.web
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, Code::CfgInvalidValue);
        assert!(errors[0].message.contains("app_id"));
    }

    #[test]
    fn connector_alias_and_conflict() {
        let ok = "\
devices:
  - name: tv
    platform: webos
    target_endpoint: ws://192.168.1.50:9998
    connector: 192.168.1.50:50051
";
        let outcome = parse_devices(ok, devices_path());
        assert!(
            !outcome.diagnostics.has_errors(),
            "{:?}",
            outcome.diagnostics
        );
        let devices = outcome.config.expect("devices");
        assert_eq!(
            devices.devices[0].agent_address(),
            Some("192.168.1.50:50051")
        );

        let conflict = "\
devices:
  - name: tv
    platform: webos
    target_endpoint: ws://192.168.1.50:9998
    connector: 192.168.1.50:50051
    agent_address: 192.168.1.51:50051
";
        let outcome = parse_devices(conflict, devices_path());
        assert_eq!(outcome.diagnostics.errors().count(), 1);
        assert!(outcome
            .diagnostics
            .errors()
            .next()
            .is_some_and(|d| d.message.contains("alias")));
    }

    #[test]
    fn tvos_requires_agent_address() {
        let source = "\
devices:
  - name: apple_tv
    platform: tvos
    target_endpoint: 00008110-001B3456021E002F
    app_id: com.example.shop.tv
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("agent_address"));
        assert!(errors[0].help.as_deref().unwrap_or("").contains("Bonjour"));
    }

    #[test]
    fn option_applicability_and_values() {
        let source = "\
devices:
  - name: phone
    platform: android
    target_endpoint: emulator-5554
    app_id: com.example.shop
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
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("key_dispatch")));
        assert!(messages.iter().any(|m| m.contains("agent_port")));
    }

    #[test]
    fn unknown_device_and_option_keys_get_hints() {
        let source = "\
devices:
  - name: phone
    platform: android
    target_endpoint: emulator-5554
    app_id: com.example.shop
    plataform: android
    options:
      clear_appdata: true
";
        let outcome = parse_devices(source, devices_path());
        let errors: Vec<_> = outcome.diagnostics.errors().collect();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().all(|d| d.code == Code::CfgUnknownKey));
        assert!(errors
            .iter()
            .any(|d| { d.help.as_deref().unwrap_or("").contains("clear_app_data") }));
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
