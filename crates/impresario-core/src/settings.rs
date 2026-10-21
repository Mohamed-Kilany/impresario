//! Layered configuration with lossless editing (TASK-2 W-CONFIG).
//!
//! One schema describes every supported key, its type, its built-in default
//! and its documentation. The same schema validates hand-edited files, parses
//! `impresario config set` values, powers `--explain` and generates the table
//! in `docs/CONFIG.md`; there is no second list of keys to drift from it.
//!
//! Files are edited through `yaml-edit`, which round-trips the concrete syntax
//! tree: comments, key order, quoting and line endings survive a `set`/`unset`
//! byte for byte everywhere except the edited scalar (spike S-5, decision
//! I-91).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use serde_yaml_ng::Value;
use yaml_edit::path::YamlPath as _;
use yaml_edit::YamlFile;

use crate::config::{AppReset, ImpresarioConfig, Reporter};
use crate::diag::{suggest, Code, Diagnostic, Diagnostics};
use crate::span::Span;

/// Where a setting may be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Global and workspace config.
    Both,
    /// Only the global config (`workspace`).
    GlobalOnly,
}

/// The value type and validation rule of one setting.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// Free text with a maximum length; no leading/trailing whitespace.
    Text {
        /// Maximum character count.
        max: usize,
    },
    /// A non-empty path.
    Path,
    /// `true` or `false`.
    Bool,
    /// An integer in an inclusive range.
    Int {
        /// Inclusive lower bound.
        min: i64,
        /// Inclusive upper bound.
        max: i64,
    },
    /// One of a fixed set of spellings.
    Enum(&'static [&'static str]),
    /// A comma-separated list of one of a fixed set of spellings.
    List(&'static [&'static str]),
    /// A list of non-empty paths.
    PathList,
}

/// One entry of the configuration schema.
#[derive(Debug, Clone, Copy)]
pub struct SettingDef {
    /// Dotted key as typed on the command line.
    pub key: &'static str,
    /// Type and validation rule.
    pub kind: Kind,
    /// Default as command-line text (`""` means "computed" or "unset").
    pub default: &'static str,
    /// Writable scope.
    pub scope: Scope,
    /// One-line documentation for `docs/CONFIG.md` and `config list`.
    pub doc: &'static str,
}

impl Kind {
    /// A short type name for the generated table.
    #[must_use]
    pub fn type_name(self) -> String {
        match self {
            Kind::Text { .. } => "string".to_string(),
            Kind::Path => "path".to_string(),
            Kind::Bool => "bool".to_string(),
            Kind::Int { min, max } => format!("integer {min}..={max}"),
            Kind::Enum(values) => format!("one of: {}", values.join(", ")),
            Kind::List(values) => format!("list of: {}", values.join(", ")),
            Kind::PathList => "list of paths".to_string(),
        }
    }
}

const REPORTERS: &[&str] = &["console", "junit", "json", "html"];

/// Every supported configuration key, in display order.
pub const SETTINGS: &[SettingDef] = &[
    SettingDef {
        key: "project",
        kind: Kind::Text { max: 200 },
        default: "",
        scope: Scope::Both,
        doc: "Project name used in reports (default: the workspace directory name).",
    },
    SettingDef {
        key: "timeout_default_ms",
        kind: Kind::Int {
            min: 100,
            max: 600_000,
        },
        default: "30000",
        scope: Scope::Both,
        doc: "Default timeout for one command, in milliseconds.",
    },
    SettingDef {
        key: "screenshot_on_failure",
        kind: Kind::Bool,
        default: "true",
        scope: Scope::Both,
        doc: "Capture a screenshot when a step fails.",
    },
    SettingDef {
        key: "report_output",
        kind: Kind::Path,
        default: "./reports",
        scope: Scope::Both,
        doc: "Directory for report files.",
    },
    SettingDef {
        key: "plays_dir",
        kind: Kind::Path,
        default: "./plays",
        scope: Scope::Both,
        doc: "Directory scanned for .play files.",
    },
    SettingDef {
        key: "scenes_dir",
        kind: Kind::Path,
        default: "./scenes",
        scope: Scope::Both,
        doc: "Directory scanned for .scene files.",
    },
    SettingDef {
        key: "theatres_dir",
        kind: Kind::Path,
        default: "./theatres",
        scope: Scope::Both,
        doc: "Directory scanned for .theatre files.",
    },
    SettingDef {
        key: "retries",
        kind: Kind::Int { min: 0, max: 5 },
        default: "0",
        scope: Scope::Both,
        doc: "Retry a failed act this many times.",
    },
    SettingDef {
        key: "parallel_devices",
        kind: Kind::Int { min: 1, max: 64 },
        default: "4",
        scope: Scope::Both,
        doc: "Maximum devices driven in parallel within one job.",
    },
    SettingDef {
        key: "app_reset",
        kind: Kind::Enum(&["per_act", "per_play", "never"]),
        default: "per_act",
        scope: Scope::Both,
        doc: "When app state is reset between acts or plays.",
    },
    SettingDef {
        key: "poll_interval_ms",
        kind: Kind::Int { min: 20, max: 2000 },
        default: "100",
        scope: Scope::Both,
        doc: "Polling interval for waits and checks, in milliseconds.",
    },
    SettingDef {
        key: "scroll_max_attempts",
        kind: Kind::Int { min: 1, max: 100 },
        default: "10",
        scope: Scope::Both,
        doc: "Maximum scroll attempts while looking for a piece.",
    },
    SettingDef {
        key: "focus_max_steps",
        kind: Kind::Int { min: 1, max: 500 },
        default: "50",
        scope: Scope::Both,
        doc: "Maximum focus traversal steps on TV platforms.",
    },
    SettingDef {
        key: "agent_start_timeout_ms",
        kind: Kind::Int {
            min: 1000,
            max: 600_000,
        },
        default: "60000",
        scope: Scope::Both,
        doc: "How long to wait for an executor to start, in milliseconds.",
    },
    SettingDef {
        key: "command_grace_ms",
        kind: Kind::Int {
            min: 0,
            max: 30_000,
        },
        default: "2000",
        scope: Scope::Both,
        doc: "Extra grace period after a command completes, in milliseconds.",
    },
    SettingDef {
        key: "reporters",
        kind: Kind::List(REPORTERS),
        default: "console,junit,json",
        scope: Scope::Both,
        doc: "Reporters attached to a run.",
    },
    SettingDef {
        key: "workspace",
        kind: Kind::Path,
        default: "",
        scope: Scope::GlobalOnly,
        doc: "Workspace the CLI uses when no flag, environment variable or marker applies.",
    },
    SettingDef {
        key: "workspaces",
        kind: Kind::PathList,
        default: "",
        scope: Scope::GlobalOnly,
        doc: "Workspaces remembered by `impresario workspace add`.",
    },
    SettingDef {
        key: "ui.theme",
        kind: Kind::Text { max: 40 },
        default: "auto",
        scope: Scope::Both,
        doc: "TUI color theme (`auto`, `dark`, `light` or a theme name).",
    },
    SettingDef {
        key: "ui.history_size",
        kind: Kind::Int {
            min: 100,
            max: 100_000,
        },
        default: "1000",
        scope: Scope::Both,
        doc: "Number of interactive prompt entries kept.",
    },
];

/// Look up a schema entry.
#[must_use]
pub fn definition(key: &str) -> Option<&'static SettingDef> {
    SETTINGS.iter().find(|def| def.key == key)
}

/// A typed setting value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    /// Text or path.
    Text(String),
    /// Boolean.
    Bool(bool),
    /// Integer.
    Int(i64),
    /// Enum spelling.
    Enum(String),
    /// List of enum spellings.
    List(Vec<String>),
}

impl SettingValue {
    /// Render for `config get`/`config list` (plain, no quotes).
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            SettingValue::Text(text) | SettingValue::Enum(text) => text.clone(),
            SettingValue::Bool(value) => value.to_string(),
            SettingValue::Int(value) => value.to_string(),
            SettingValue::List(values) => values.join(","),
        }
    }
}

/// Parse one command-line value against the schema.
pub fn parse_cli_value(def: &SettingDef, raw: &str) -> Result<SettingValue, String> {
    match def.kind {
        Kind::Text { max } => {
            if raw.trim() != raw || raw.is_empty() {
                return Err("expected text without leading/trailing whitespace".to_string());
            }
            if raw.chars().count() > max {
                return Err(format!("expected at most {max} characters"));
            }
            Ok(SettingValue::Text(raw.to_string()))
        }
        Kind::Path => {
            if raw.trim().is_empty() {
                return Err("expected a non-empty path".to_string());
            }
            Ok(SettingValue::Text(raw.to_string()))
        }
        Kind::PathList => {
            let mut parsed = Vec::new();
            for item in raw.split(',') {
                let item = item.trim();
                if item.is_empty() {
                    continue;
                }
                if parsed.iter().any(|seen| seen == item) {
                    return Err(format!("duplicate path '{item}'"));
                }
                parsed.push(item.to_string());
            }
            if parsed.is_empty() {
                return Err("expected a comma-separated list of paths".to_string());
            }
            Ok(SettingValue::List(parsed))
        }
        Kind::Bool => match raw {
            "true" => Ok(SettingValue::Bool(true)),
            "false" => Ok(SettingValue::Bool(false)),
            _ => Err("expected true or false".to_string()),
        },
        Kind::Int { min, max } => {
            let value = raw
                .parse::<i64>()
                .map_err(|_| format!("expected an integer in {min}..={max}"))?;
            if value < min || value > max {
                return Err(format!("expected an integer in {min}..={max}"));
            }
            Ok(SettingValue::Int(value))
        }
        Kind::Enum(values) => {
            if values.contains(&raw) {
                Ok(SettingValue::Enum(raw.to_string()))
            } else {
                Err(format!("expected one of: {}", values.join(", ")))
            }
        }
        Kind::List(values) => {
            let mut parsed = Vec::new();
            for item in raw.split(',') {
                let item = item.trim();
                if item.is_empty() {
                    continue;
                }
                if !values.contains(&item) {
                    return Err(format!("expected a list of: {}", values.join(", ")));
                }
                if parsed.iter().any(|seen| seen == item) {
                    return Err(format!("duplicate value '{item}'"));
                }
                parsed.push(item.to_string());
            }
            if parsed.is_empty() {
                return Err(format!("expected a list of: {}", values.join(", ")));
            }
            Ok(SettingValue::List(parsed))
        }
    }
}

fn invalid_value(
    def: &SettingDef,
    reason: &str,
    lines: &[&str],
    path: &Path,
    value: Option<&Value>,
) -> Diagnostic {
    let mut diag = Diagnostic::error(
        Code::CfgInvalidValue,
        format!("invalid '{}': {reason}", def.key),
    )
    .with_span(span_of(lines, path, def.key));
    if let (Kind::Enum(values) | Kind::List(values), Some(text)) =
        (def.kind, value.and_then(Value::as_str))
    {
        if let Some(hint) = suggest(text, values) {
            diag = diag.with_help(hint);
        }
    }
    diag
}

fn parse_yaml_value(
    def: &SettingDef,
    value: &Value,
    lines: &[&str],
    path: &Path,
    diagnostics: &mut Diagnostics,
) -> Option<SettingValue> {
    let mut reject = |reason: &str| {
        diagnostics.push(invalid_value(def, reason, lines, path, Some(value)));
        None
    };
    match def.kind {
        Kind::Text { max } => match value.as_str() {
            Some(text)
                if text.trim() == text && !text.is_empty() && text.chars().count() <= max =>
            {
                Some(SettingValue::Text(text.to_string()))
            }
            Some(_) => reject("expected text without leading/trailing whitespace"),
            None => reject("expected a string"),
        },
        Kind::Path => match value.as_str() {
            Some(text) if !text.trim().is_empty() => Some(SettingValue::Text(text.to_string())),
            _ => reject("expected a non-empty path"),
        },
        Kind::PathList => {
            let Some(sequence) = value.as_sequence() else {
                return reject("expected a list of paths");
            };
            let mut parsed = Vec::new();
            for item in sequence {
                let Some(text) = item.as_str() else {
                    return reject("expected a list of paths");
                };
                if text.trim().is_empty() {
                    return reject("expected a list of paths");
                }
                if parsed.iter().any(|seen: &String| seen == text) {
                    return reject(&format!("duplicate path '{text}'"));
                }
                parsed.push(text.to_string());
            }
            Some(SettingValue::List(parsed))
        }
        Kind::Bool => match value.as_bool() {
            Some(boolean) => Some(SettingValue::Bool(boolean)),
            None => reject("expected true or false"),
        },
        Kind::Int { min, max } => match value.as_i64() {
            Some(number) if number >= min && number <= max => Some(SettingValue::Int(number)),
            _ => reject(&format!("expected an integer in {min}..={max}")),
        },
        Kind::Enum(values) => match value.as_str() {
            Some(text) if values.contains(&text) => Some(SettingValue::Enum(text.to_string())),
            _ => reject(&format!("expected one of: {}", values.join(", "))),
        },
        Kind::List(values) => {
            let Some(sequence) = value.as_sequence() else {
                return reject(&format!("expected a list of: {}", values.join(", ")));
            };
            let mut parsed = Vec::new();
            for item in sequence {
                let Some(text) = item.as_str() else {
                    return reject("expected a list of strings");
                };
                if !values.contains(&text) {
                    return reject(&format!("unknown value '{text}'"));
                }
                if parsed.iter().any(|seen: &String| seen == text) {
                    return reject(&format!("duplicate value '{text}'"));
                }
                parsed.push(text.to_string());
            }
            Some(SettingValue::List(parsed))
        }
    }
}

/// One parsed config file.
#[derive(Debug, Clone)]
pub struct SettingsFile {
    /// Path as the user sees it.
    pub path: PathBuf,
    /// Values present in the file, by dotted key.
    pub values: BTreeMap<String, SettingValue>,
    /// Parse and validation errors.
    pub diagnostics: Diagnostics,
}

impl SettingsFile {
    /// An absent file is an empty layer, not an error.
    #[must_use]
    pub fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            values: BTreeMap::new(),
            diagnostics: Diagnostics::new(),
        }
    }

    /// Read and parse a file; a missing file is an empty layer.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(source) => parse_settings(&source, path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::empty(path),
            Err(error) => {
                let mut diagnostics = Diagnostics::new();
                diagnostics.push(Diagnostic::error(
                    Code::CfgInvalidValue,
                    format!("cannot read {}: {error}", path.display()),
                ));
                Self {
                    path: path.to_path_buf(),
                    values: BTreeMap::new(),
                    diagnostics,
                }
            }
        }
    }
}

/// Parse one settings file. Unknown keys and invalid values are diagnostics;
/// valid values are still returned so `config list` can show them.
#[must_use]
pub fn parse_settings(source: &str, path: &Path) -> SettingsFile {
    let mut diagnostics = Diagnostics::new();
    let value = match serde_yaml_ng::from_str::<Value>(source) {
        Ok(value) => value,
        Err(error) => {
            diagnostics.push(crate::config::yaml_syntax_diagnostic(&error, path));
            return SettingsFile {
                path: path.to_path_buf(),
                values: BTreeMap::new(),
                diagnostics,
            };
        }
    };
    let Some(mapping) = value.as_mapping() else {
        if !source.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    Code::CfgInvalidValue,
                    "the config file must be a YAML mapping",
                )
                .with_span(Span::new(path, 1, 1, 0))
                .with_help("see docs/CONFIG.md for the file layout"),
            );
        }
        return SettingsFile {
            path: path.to_path_buf(),
            values: BTreeMap::new(),
            diagnostics,
        };
    };

    let lines: Vec<&str> = source.lines().collect();
    let mut leaves: Vec<(String, &Value)> = Vec::new();
    let root = Value::Mapping(mapping.clone());
    flatten("", &root, &mut leaves);
    let mut values = BTreeMap::new();
    for (key, leaf) in leaves {
        let Some(def) = definition(&key) else {
            let mut diag =
                Diagnostic::error(Code::CfgUnknownKey, format!("unknown key '{key}'")).with_span(
                    span_of(&lines, path, key.rsplit('.').next().unwrap_or(&key)),
                );
            if let Some(hint) = suggest(
                &key,
                &SETTINGS.iter().map(|def| def.key).collect::<Vec<_>>(),
            ) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
            continue;
        };
        if let Some(parsed) = parse_yaml_value(def, leaf, &lines, path, &mut diagnostics) {
            values.insert(key, parsed);
        }
    }
    SettingsFile {
        path: path.to_path_buf(),
        values,
        diagnostics,
    }
}

fn flatten<'a>(prefix: &str, value: &'a Value, out: &mut Vec<(String, &'a Value)>) {
    // `profiles:` is a structured, user-editable section owned by
    // `impresario profile`, not a scalar setting. It is validated there.
    if prefix == "profiles" {
        return;
    }
    if !prefix.is_empty() && definition(prefix).is_some() {
        out.push((prefix.to_string(), value));
        return;
    }
    if let Some(mapping) = value.as_mapping() {
        for (key, child) in mapping {
            let Some(key) = key.as_str() else {
                continue;
            };
            let dotted = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            flatten(&dotted, child, out);
        }
    } else if !prefix.is_empty() {
        out.push((prefix.to_string(), value));
    }
}

fn span_of(lines: &[&str], path: &Path, key: &str) -> Span {
    let needle = format!("{key}:");
    for (index, line) in lines.iter().enumerate() {
        if let Some(offset) = line.find(&needle) {
            let col = line
                .get(..offset)
                .map_or(1, |prefix| prefix.chars().count() + 1);
            return Span::new(path, index + 1, col, key.chars().count());
        }
    }
    Span::new(path, 1, 1, 0)
}

/// Which layer supplied a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerSource {
    /// `<workspace>/impresario.config`.
    Workspace(PathBuf),
    /// `~/.impresario/config.yaml`.
    Global(PathBuf),
    /// The schema default.
    Default,
}

impl std::fmt::Display for LayerSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayerSource::Workspace(path) => write!(f, "workspace ({})", path.display()),
            LayerSource::Global(path) => write!(f, "global ({})", path.display()),
            LayerSource::Default => f.write_str("built-in default"),
        }
    }
}

/// A resolved setting plus where it came from and what it shadows.
#[derive(Debug, Clone)]
pub struct Explained {
    /// The winning value.
    pub value: SettingValue,
    /// The winning layer.
    pub source: LayerSource,
    /// Lower-priority values that lost, highest first.
    pub shadowed: Vec<(LayerSource, SettingValue)>,
}

/// The merged, typed configuration.
#[derive(Debug, Clone)]
pub struct EffectiveSettings {
    values: BTreeMap<&'static str, SettingValue>,
    sources: BTreeMap<&'static str, LayerSource>,
    shadowed: BTreeMap<&'static str, Vec<(LayerSource, SettingValue)>>,
}

impl EffectiveSettings {
    /// Resolve every key: workspace beats global beats default.
    #[must_use]
    pub fn resolve(global: &SettingsFile, workspace: &SettingsFile) -> Self {
        let mut values = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut shadowed = BTreeMap::new();
        for def in SETTINGS {
            let mut layers = Vec::new();
            if let Some(value) = workspace.values.get(def.key) {
                layers.push((
                    LayerSource::Workspace(workspace.path.clone()),
                    value.clone(),
                ));
            }
            if let Some(value) = global.values.get(def.key) {
                layers.push((LayerSource::Global(global.path.clone()), value.clone()));
            }
            if !def.default.is_empty() {
                if let Ok(value) = parse_cli_value(def, def.default) {
                    layers.push((LayerSource::Default, value));
                }
            }
            let (source, value) = layers
                .first()
                .map(|(source, value)| (source.clone(), value.clone()))
                .unwrap_or_else(|| (LayerSource::Default, SettingValue::Text(String::new())));
            let rest: Vec<_> = layers.into_iter().skip(1).collect();
            if !rest.is_empty() {
                shadowed.insert(def.key, rest);
            }
            values.insert(def.key, value);
            sources.insert(def.key, source);
        }
        Self {
            values,
            sources,
            shadowed,
        }
    }

    /// The winning value for a key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.values.get(key)
    }

    /// The winning layer for a key.
    #[must_use]
    pub fn source(&self, key: &str) -> Option<&LayerSource> {
        self.sources.get(key)
    }

    /// Whether any layer set the key (as opposed to it being a built-in
    /// default). `workspace` has no default, so this distinguishes "unset".
    #[must_use]
    pub fn is_set(&self, key: &str) -> bool {
        !matches!(self.sources.get(key), Some(LayerSource::Default) | None)
    }

    /// The full explanation for `config get --explain`.
    #[must_use]
    pub fn explain(&self, key: &str) -> Option<Explained> {
        Some(Explained {
            value: self.values.get(key)?.clone(),
            source: self.sources.get(key)?.clone(),
            shadowed: self.shadowed.get(key).cloned().unwrap_or_default(),
        })
    }

    /// Every key with its winning value and layer, in schema order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &SettingValue, &LayerSource)> {
        SETTINGS.iter().filter_map(|def| {
            let value = self.values.get(def.key)?;
            let source = self.sources.get(def.key)?;
            Some((def.key, value, source))
        })
    }

    /// Build the runner-facing typed configuration.
    #[must_use]
    pub fn to_runner(&self, workspace_root: &Path) -> ImpresarioConfig {
        let text = |key: &str| self.values.get(key).map(SettingValue::display);
        let int = |key: &str| match self.values.get(key) {
            Some(SettingValue::Int(value)) => *value,
            _ => 0,
        };
        let boolean = |key: &str| matches!(self.values.get(key), Some(SettingValue::Bool(true)));
        let project = match self.sources.get("project") {
            Some(LayerSource::Default) | None => workspace_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("impresario")
                .to_string(),
            _ => text("project").unwrap_or_default(),
        };
        let reporters = match self.values.get("reporters") {
            Some(SettingValue::List(values)) => values
                .iter()
                .filter_map(|name| Reporter::ALL.iter().copied().find(|r| r.as_str() == name))
                .collect(),
            _ => vec![Reporter::Console, Reporter::Junit, Reporter::Json],
        };
        let app_reset = match text("app_reset").as_deref() {
            Some("per_play") => AppReset::PerPlay,
            Some("never") => AppReset::Never,
            _ => AppReset::PerAct,
        };
        ImpresarioConfig {
            project,
            timeout_default_ms: u64::try_from(int("timeout_default_ms")).unwrap_or(30_000),
            screenshot_on_failure: boolean("screenshot_on_failure"),
            report_output: PathBuf::from(
                text("report_output").unwrap_or_else(|| "./reports".into()),
            ),
            plays_dir: PathBuf::from(text("plays_dir").unwrap_or_else(|| "./plays".into())),
            scenes_dir: PathBuf::from(text("scenes_dir").unwrap_or_else(|| "./scenes".into())),
            theatres_dir: PathBuf::from(
                text("theatres_dir").unwrap_or_else(|| "./theatres".into()),
            ),
            retries: u32::try_from(int("retries")).unwrap_or(0),
            parallel_devices: u32::try_from(int("parallel_devices")).unwrap_or(4),
            app_reset,
            poll_interval_ms: u64::try_from(int("poll_interval_ms")).unwrap_or(100),
            scroll_max_attempts: u32::try_from(int("scroll_max_attempts")).unwrap_or(10),
            focus_max_steps: u32::try_from(int("focus_max_steps")).unwrap_or(50),
            agent_start_timeout_ms: u64::try_from(int("agent_start_timeout_ms")).unwrap_or(60_000),
            command_grace_ms: u64::try_from(int("command_grace_ms")).unwrap_or(2_000),
            reporters,
        }
    }
}

/// Resolve the effective settings and load them as a runner configuration.
///
/// The global file comes from [`crate::home::Home::resolve`]; when no home is
/// available (embedded invocations), only the workspace layer is read. Both
/// files are optional and every key has a built-in default (TASK-2 section
/// 4.1), so an empty workspace still yields a usable configuration.
#[must_use]
pub fn load_default_layers(
    config_path: &Path,
    workspace_root: &Path,
) -> crate::config::ConfigOutcome<ImpresarioConfig> {
    let global = match crate::home::Home::resolve() {
        Ok(home) => SettingsFile::load(&home.config_path()),
        Err(_) => SettingsFile::empty("<no global home>"),
    };
    load_layers(&global, config_path, workspace_root)
}

/// Like [`load_default_layers`] but with an explicit global layer.
#[must_use]
pub fn load_layers(
    global: &SettingsFile,
    config_path: &Path,
    workspace_root: &Path,
) -> crate::config::ConfigOutcome<ImpresarioConfig> {
    let workspace = SettingsFile::load(config_path);
    let mut diagnostics = global.diagnostics.clone();
    diagnostics.extend(workspace.diagnostics.iter().cloned());
    if diagnostics.has_errors() {
        return crate::config::ConfigOutcome {
            config: None,
            diagnostics,
        };
    }
    let effective = EffectiveSettings::resolve(global, &workspace);
    crate::config::ConfigOutcome {
        config: Some(effective.to_runner(workspace_root)),
        diagnostics,
    }
}

/// Set one key in `source`, preserving every other byte.
///
/// The returned text is validated again by the caller before it is written.
pub fn edit_set(source: &str, def: &SettingDef, value: &SettingValue) -> Result<String, String> {
    let file = YamlFile::from_str(source).map_err(|error| error.to_string())?;
    let document = file.ensure_document();
    match value {
        SettingValue::Text(text) | SettingValue::Enum(text) => {
            document
                .try_set_path(def.key, text.as_str())
                .map_err(|error| error.to_string())?;
        }
        SettingValue::Bool(value) => {
            document
                .try_set_path(def.key, *value)
                .map_err(|error| error.to_string())?;
        }
        SettingValue::Int(value) => {
            document
                .try_set_path(def.key, *value)
                .map_err(|error| error.to_string())?;
        }
        SettingValue::List(values) => {
            let sequence = values
                .iter()
                .map(|value| {
                    yaml_edit::YamlValue::Scalar(yaml_edit::ScalarValue::string(value.clone()))
                })
                .collect();
            // yaml-edit 0.3.2 serializes a replacement of an *indentless*
            // block sequence (`key:\n- item`) as `key: - item\n- item`, which
            // is not valid YAML. Removing the old node first makes it emit a
            // fresh block sequence instead.
            let _ = document.try_remove_path(def.key);
            document
                .try_set_path(def.key, yaml_edit::YamlValue::Sequence(sequence))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(file.to_string())
}

/// Remove one key from `source`, preserving every other byte.
pub fn edit_unset(source: &str, key: &str) -> Result<String, String> {
    let file = YamlFile::from_str(source).map_err(|error| error.to_string())?;
    let Some(document) = file.document() else {
        return Ok(source.to_string());
    };
    match document.try_remove_path(key) {
        Ok(_) => Ok(file.to_string()),
        // A missing key is not an error for `unset`.
        Err(_) => Ok(source.to_string()),
    }
}

/// The generated table for `docs/CONFIG.md`.
#[must_use]
pub fn docs_table() -> String {
    let mut out =
        String::from("| Key | Scope | Type | Default | Description |\n|---|---|---|---|---|\n");
    for def in SETTINGS {
        let default = if def.default.is_empty() {
            "(unset)".to_string()
        } else {
            format!("`{}`", def.default)
        };
        let scope = match def.scope {
            Scope::Both => "global + workspace",
            Scope::GlobalOnly => "global",
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            def.key,
            scope,
            def.kind.type_name(),
            default,
            def.doc
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> SettingsFile {
        parse_settings(source, Path::new("impresario.config"))
    }

    #[test]
    fn defaults_fill_every_key() {
        let effective = EffectiveSettings::resolve(
            &SettingsFile::empty("global.yaml"),
            &SettingsFile::empty("impresario.config"),
        );
        assert_eq!(
            effective.get("timeout_default_ms"),
            Some(&SettingValue::Int(30000))
        );
        assert_eq!(
            effective.get("reporters"),
            Some(&SettingValue::List(vec![
                "console".into(),
                "junit".into(),
                "json".into()
            ]))
        );
        assert_eq!(
            effective.get("ui.theme"),
            Some(&SettingValue::Text("auto".into()))
        );
        assert_eq!(effective.source("project"), Some(&LayerSource::Default));
    }

    #[test]
    fn workspace_beats_global_beats_default() {
        let global = parse_settings(
            "timeout_default_ms: 1000\nreport_output: ./global-reports\n",
            Path::new("global.yaml"),
        );
        let workspace =
            parse_settings("timeout_default_ms: 2000\n", Path::new("impresario.config"));
        let effective = EffectiveSettings::resolve(&global, &workspace);
        assert_eq!(
            effective.get("timeout_default_ms"),
            Some(&SettingValue::Int(2000))
        );
        assert!(matches!(
            effective.source("timeout_default_ms"),
            Some(LayerSource::Workspace(_))
        ));
        assert_eq!(
            effective.get("report_output"),
            Some(&SettingValue::Text("./global-reports".into()))
        );
        assert!(matches!(
            effective.source("report_output"),
            Some(LayerSource::Global(_))
        ));
        assert_eq!(effective.get("retries"), Some(&SettingValue::Int(0)));
    }

    #[test]
    fn nested_keys_are_flattened() {
        let file = parse("ui:\n  theme: dark\n  history_size: 500\n");
        assert!(!file.diagnostics.has_errors(), "{:?}", file.diagnostics);
        assert_eq!(
            file.values.get("ui.theme"),
            Some(&SettingValue::Text("dark".into()))
        );
        assert_eq!(
            file.values.get("ui.history_size"),
            Some(&SettingValue::Int(500))
        );
    }

    #[test]
    fn unknown_keys_and_bad_values_are_diagnostics() {
        let file = parse("unknown_key: 1\ntimeout_default_ms: 5\nreporters: [console, nope]\n");
        assert_eq!(
            file.diagnostics
                .errors()
                .filter(|d| d.code == Code::CfgUnknownKey)
                .count(),
            1
        );
        assert!(
            file.diagnostics.errors().count() >= 3,
            "{:?}",
            file.diagnostics
        );
    }

    #[test]
    fn cli_values_validate() {
        let timeout = definition("timeout_default_ms").expect("schema");
        assert!(parse_cli_value(timeout, "1000").is_ok());
        assert!(parse_cli_value(timeout, "0").is_err());
        assert!(parse_cli_value(timeout, "nope").is_err());
        let reporters = definition("reporters").expect("schema");
        assert_eq!(
            parse_cli_value(reporters, "console, json").expect("list"),
            SettingValue::List(vec!["console".into(), "json".into()])
        );
        assert!(parse_cli_value(reporters, "console,nope").is_err());
    }

    #[test]
    fn set_then_unset_is_byte_identical() {
        let source = "# workspace config\nproject: shop   # keep\nreporters:\n  - console\n  - junit\n\n# tail\n";
        let def = definition("timeout_default_ms").expect("schema");
        let edited = edit_set(source, def, &SettingValue::Int(45000)).expect("set");
        assert!(edited.contains("# workspace config"));
        assert!(edited.contains("timeout_default_ms: 45000"));
        assert!(edited.contains("project: shop   # keep"));
        let restored = edit_unset(&edited, "timeout_default_ms").expect("unset");
        assert_eq!(restored, source);
    }

    #[test]
    fn set_existing_key_keeps_its_comment_and_line_endings() {
        let source = "# head\r\ntimeout_default_ms: 30000   # tuned\r\nreporters: [console]\r\n";
        let def = definition("timeout_default_ms").expect("schema");
        let edited = edit_set(source, def, &SettingValue::Int(60000)).expect("set");
        assert!(
            edited.contains("timeout_default_ms: 60000   # tuned"),
            "{edited}"
        );
        assert!(edited.contains("\r\n"), "{edited}");
        assert!(
            !edited.contains('\n')
                || edited.matches("\r\n").count() == edited.matches('\n').count()
        );
    }

    #[test]
    fn set_list_creates_a_valid_sequence() {
        let def = definition("workspaces").expect("schema");
        let source = "workspace: /one\n";
        let edited = edit_set(
            source,
            def,
            &SettingValue::List(vec!["/two".into(), "/three".into()]),
        )
        .expect("set list");
        let parsed = parse_settings(&edited, Path::new("global.yaml"));
        assert!(
            !parsed.diagnostics.has_errors(),
            "edited YAML:\n{edited}\ndiagnostics: {:?}",
            parsed.diagnostics
        );
        assert!(edited.contains("/two"), "{edited}");
        assert!(edited.contains("workspace: /one"), "{edited}");
    }

    #[test]
    fn set_list_existing_indentless_sequence_keeps_a_valid_document() {
        let def = definition("workspaces").expect("schema");
        let source = "workspace: C:\\ws1\nworkspaces:\n- C:\\ws1\n";
        let edited = edit_set(
            source,
            def,
            &SettingValue::List(vec!["C:\\ws1".into(), "C:\\ws2".into()]),
        )
        .expect("set list");
        let parsed = parse_settings(&edited, Path::new("global.yaml"));
        assert!(
            !parsed.diagnostics.has_errors(),
            "edited YAML:\n{edited}\ndiagnostics: {:?}",
            parsed.diagnostics
        );
    }

    #[test]
    fn creating_a_nested_key_works() {
        let source = "project: shop\n";
        let def = definition("ui.theme").expect("schema");
        let edited = edit_set(source, def, &SettingValue::Text("dark".into())).expect("set");
        assert!(edited.contains("ui:"), "{edited}");
        assert!(edited.contains("theme: dark"), "{edited}");
        assert!(edited.contains("project: shop"), "{edited}");
    }

    #[test]
    fn runner_config_uses_workspace_name_when_project_is_unset() {
        let effective = EffectiveSettings::resolve(
            &SettingsFile::empty("global.yaml"),
            &SettingsFile::empty("impresario.config"),
        );
        let config = effective.to_runner(Path::new("D:/work/shop-tests"));
        assert_eq!(config.project, "shop-tests");
        assert_eq!(config.timeout_default_ms, 30000);
        assert_eq!(config.theatres_dir, Path::new("./theatres"));
    }

    #[test]
    fn docs_table_lists_every_key() {
        let table = docs_table();
        for def in SETTINGS {
            assert!(table.contains(&format!("`{}`", def.key)), "{}", def.key);
        }
    }
}
