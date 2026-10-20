//! App definitions (TASK-2 section 5, D-21): an app is separate from a device.
//!
//! `impresario.apps.config` is the workspace layer and
//! `~/.impresario/apps.yaml` the global one; the workspace wins on a name
//! collision. A play binds to an app with `App: <name>` (or `--app`), and the
//! planner resolves the app's entry for each device platform before any
//! hardware is touched.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

use crate::config::{
    device_ranges, read_optional_layer, span_of, top_level_line, valid_device_name,
};
use crate::diag::{suggest, Code, Diagnostic, Diagnostics};
use crate::home::Home;
use crate::platforms::Platform;
use crate::span::Span;
use crate::strings::free_text_ok;

const APP_KEYS: &[&str] = &["name", "description", "platforms", "options"];
const PLATFORM_KEYS: &[&str] = &["id", "package", "launch_activity", "url"];
const APP_OPTION_KEYS: &[&str] = &["clear_data", "launch_params"];

/// One platform entry of an app (TASK-2 section 5).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppPlatform {
    /// Android package, Apple bundle id, webOS app id or Tizen app id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Artifact to install/re-sign (optional: attach to an already-installed app).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// Android only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_activity: Option<String>,
    /// Web only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Per-app run options (the old device app options moved here).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppOptions {
    /// Wipe the app's data before a reset (I-18/I-82/I-84).
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_data: bool,
    /// Platform-specific launch parameters, passed to the executor.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub launch_params: BTreeMap<String, String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One app definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub platforms: BTreeMap<Platform, AppPlatform>,
    #[serde(default, skip_serializing_if = "AppOptions::is_default")]
    pub options: AppOptions,
}

impl AppOptions {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &AppOptions::default()
    }
}

impl AppConfig {
    /// The entry for `platform`, if the app supports it.
    #[must_use]
    pub fn platform(&self, platform: Platform) -> Option<&AppPlatform> {
        self.platforms.get(&platform)
    }

    /// The best entry for `platform`: an exact match, else the family parent
    /// (`androidtv` -> `android`, `tvos` -> `ios`, `webos`/`tizen` -> `web`).
    /// This mirrors the selector fallback chain (D-07), but an exact entry is
    /// always preferred.
    #[must_use]
    pub fn platform_with_fallback(&self, platform: Platform) -> Option<&AppPlatform> {
        self.platforms.get(&platform).or_else(|| {
            platform
                .selector_chain()
                .iter()
                .skip(1)
                .find_map(|candidate| self.platforms.get(candidate))
        })
    }
}

/// An apps file (`apps:` list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppsConfig {
    pub apps: Vec<AppConfig>,
}

/// Parse an apps file from source text. All errors are collected before the
/// typed value is produced; spans point into `path`.
#[must_use]
pub fn parse_apps(source: &str, path: &Path) -> crate::config::ConfigOutcome<AppsConfig> {
    let mut diagnostics = Diagnostics::new();
    let value = match serde_yaml_ng::from_str::<Value>(source) {
        Ok(value) => value,
        Err(err) => {
            diagnostics.push(crate::config::yaml_syntax_diagnostic(&err, path));
            return crate::config::ConfigOutcome {
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
                "the apps file must be a YAML mapping",
            )
            .with_span(Span::new(path, 1, 1, 0)),
        );
        return crate::config::ConfigOutcome {
            config: None,
            diagnostics,
        };
    };
    for key in map.keys().filter_map(Value::as_str) {
        if key != "apps" {
            let mut diag = Diagnostic::error(Code::CfgUnknownKey, format!("unknown key '{key}'"))
                .with_span(span_of_top(&lines, key, path))
                .with_help("the apps file only has a top-level 'apps' list");
            if let Some(hint) = suggest(key, &["apps"]) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }
    let Some(apps_value) = map.get("apps") else {
        diagnostics.push(
            Diagnostic::error(Code::CfgMissingField, "missing required key 'apps'")
                .with_span(Span::new(path, 1, 1, 0)),
        );
        return crate::config::ConfigOutcome {
            config: None,
            diagnostics,
        };
    };
    let Some(apps) = apps_value.as_sequence() else {
        diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                "'apps' must be a list of app definitions",
            )
            .with_span(span_of_top(&lines, "apps", path)),
        );
        return crate::config::ConfigOutcome {
            config: None,
            diagnostics,
        };
    };
    let ranges = top_level_line(&lines, "apps")
        .map(|line| device_ranges(&lines, line))
        .unwrap_or_default();
    let mut seen: HashSet<String> = HashSet::new();
    for (index, app) in apps.iter().enumerate() {
        let (from, until) = ranges.get(index).copied().unwrap_or((0, last_line));
        validate_app(&mut diagnostics, &lines, path, app, from, until, &mut seen);
    }
    if diagnostics.has_errors() {
        return crate::config::ConfigOutcome {
            config: None,
            diagnostics,
        };
    }
    match serde_yaml_ng::from_value::<AppsConfig>(value) {
        Ok(config) => crate::config::ConfigOutcome {
            config: Some(config),
            diagnostics,
        },
        Err(err) => {
            diagnostics.push(Diagnostic::error(
                Code::CfgInvalidValue,
                format!("invalid apps configuration: {err}"),
            ));
            crate::config::ConfigOutcome {
                config: None,
                diagnostics,
            }
        }
    }
}

/// Load the workspace apps file and the global `~/.impresario/apps.yaml`,
/// merging by name (workspace wins). Missing files are empty layers.
#[must_use]
pub fn load_apps_merged(workspace_path: &Path) -> crate::config::ConfigOutcome<AppsConfig> {
    let mut diagnostics = Diagnostics::new();
    let mut merged: Vec<AppConfig> = Vec::new();
    let global_path = Home::resolve().ok().map(|home| home.apps_path());
    if let Some(path) = global_path.as_deref() {
        if let Some(source) = read_optional_layer(path, &mut diagnostics) {
            let outcome = parse_apps(&source, path);
            diagnostics.extend(outcome.diagnostics);
            if let Some(config) = outcome.config {
                merge_apps(&mut merged, &config.apps);
            }
        }
    }
    if let Some(source) = read_optional_layer(workspace_path, &mut diagnostics) {
        let outcome = parse_apps(&source, workspace_path);
        diagnostics.extend(outcome.diagnostics);
        if let Some(config) = outcome.config {
            merge_apps(&mut merged, &config.apps);
        }
    }
    let config = (!diagnostics.has_errors()).then_some(AppsConfig { apps: merged });
    crate::config::ConfigOutcome {
        config,
        diagnostics,
    }
}

fn merge_apps(merged: &mut Vec<AppConfig>, layer: &[AppConfig]) {
    for app in layer {
        if let Some(existing) = merged.iter_mut().find(|item| item.name == app.name) {
            *existing = app.clone();
        } else {
            merged.push(app.clone());
        }
    }
}

fn span_of_top(lines: &[&str], key: &str, path: &Path) -> Span {
    top_level_line(lines, key).map_or_else(
        || Span::new(path, 1, 1, 0),
        |line| Span::new(path, line + 1, 1, key.chars().count()),
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_app(
    diagnostics: &mut Diagnostics,
    lines: &[&str],
    path: &Path,
    app: &Value,
    from: usize,
    until: usize,
    seen: &mut HashSet<String>,
) {
    let key_span = |key: &str| {
        span_of(lines, key, from, until, path).unwrap_or_else(|| Span::new(path, from + 1, 1, 0))
    };
    let Some(map) = app.as_mapping() else {
        diagnostics.push(
            Diagnostic::error(Code::CfgInvalidValue, "each app must be a YAML mapping")
                .with_span(key_span("name")),
        );
        return;
    };
    for key in map.keys().filter_map(Value::as_str) {
        if !APP_KEYS.contains(&key) {
            let mut diag =
                Diagnostic::error(Code::CfgUnknownKey, format!("unknown app key '{key}'"))
                    .with_span(key_span(key));
            if let Some(hint) = suggest(key, APP_KEYS) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }

    match map.get("name").and_then(Value::as_str) {
        Some(name) if valid_device_name(name) => {
            if !seen.insert(name.to_string()) {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgDuplicateApp,
                        format!("duplicate app name '{name}'"),
                    )
                    .with_span(key_span("name"))
                    .with_help("app names must be unique within the apps layers"),
                );
            }
        }
        Some(name) => diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                format!(
                    "invalid value for 'name': '{name}' must match ^[a-z0-9][a-z0-9_-]{{0,62}}$"
                ),
            )
            .with_span(key_span("name")),
        ),
        None => diagnostics.push(
            Diagnostic::error(Code::CfgMissingField, "missing required key 'name'")
                .with_span(key_span("name")),
        ),
    }

    if let Some(description) = map.get("description") {
        match description.as_str() {
            Some(text) if free_text_ok(text) => {}
            _ => diagnostics.push(
                Diagnostic::error(
                    Code::CfgInvalidValue,
                    "app description must be 1..=200 characters",
                )
                .with_span(key_span("description")),
            ),
        }
    }

    match map.get("platforms").and_then(Value::as_mapping) {
        Some(platforms) if !platforms.is_empty() => {
            for (key, entry) in platforms {
                let Some(text) = key.as_str() else {
                    diagnostics.push(
                        Diagnostic::error(Code::CfgInvalidValue, "platform keys must be strings")
                            .with_span(key_span("platforms")),
                    );
                    continue;
                };
                match text.parse::<Platform>() {
                    Ok(platform) => {
                        validate_platform_entry(diagnostics, platform, entry, &key_span)
                    }
                    Err(()) => {
                        let names: Vec<&str> = Platform::ALL.iter().map(|p| p.as_str()).collect();
                        let mut diag = Diagnostic::error(
                            Code::CfgInvalidValue,
                            format!("unknown app platform '{text}'"),
                        )
                        .with_span(key_span(text));
                        if let Some(hint) = suggest(text, &names) {
                            diag = diag.with_help(hint);
                        }
                        diagnostics.push(diag);
                    }
                }
            }
        }
        Some(_) => diagnostics.push(
            Diagnostic::error(
                Code::CfgMissingField,
                "an app needs at least one platform entry",
            )
            .with_span(key_span("platforms")),
        ),
        None => diagnostics.push(
            Diagnostic::error(Code::CfgMissingField, "missing required key 'platforms'")
                .with_span(key_span("platforms"))
                .with_help("for example: platforms: { android: { id: com.acme.shop } }"),
        ),
    }

    if let Some(options_value) = map.get("options") {
        match options_value.as_mapping() {
            Some(options) => {
                for key in options.keys().filter_map(Value::as_str) {
                    if !APP_OPTION_KEYS.contains(&key) {
                        let mut diag = Diagnostic::error(
                            Code::CfgUnknownKey,
                            format!("unknown app option '{key}'"),
                        )
                        .with_span(key_span(key));
                        if let Some(hint) = suggest(key, APP_OPTION_KEYS) {
                            diag = diag.with_help(hint);
                        }
                        diagnostics.push(diag);
                    }
                }
                if let Some(value) = options.get("clear_data") {
                    if value.as_bool().is_none() {
                        diagnostics.push(
                            Diagnostic::error(
                                Code::CfgInvalidValue,
                                "app option 'clear_data' must be true or false",
                            )
                            .with_span(key_span("clear_data")),
                        );
                    }
                }
                if let Some(value) = options.get("launch_params") {
                    match value.as_mapping() {
                        Some(params) => {
                            for (key, param) in params {
                                if key.as_str().is_none_or(|text| text.trim().is_empty())
                                    || param.as_str().is_none_or(|text| text.is_empty())
                                {
                                    diagnostics.push(
                                        Diagnostic::error(
                                            Code::CfgInvalidValue,
                                            "launch_params must map non-empty names to string values",
                                        )
                                        .with_span(key_span("launch_params")),
                                    );
                                }
                            }
                        }
                        None => diagnostics.push(
                            Diagnostic::error(
                                Code::CfgInvalidValue,
                                "app option 'launch_params' must be a mapping of strings",
                            )
                            .with_span(key_span("launch_params")),
                        ),
                    }
                }
            }
            None => diagnostics.push(
                Diagnostic::error(Code::CfgInvalidValue, "'options' must be a YAML mapping")
                    .with_span(key_span("options")),
            ),
        }
    }
}

fn validate_platform_entry(
    diagnostics: &mut Diagnostics,
    platform: Platform,
    entry: &Value,
    key_span: &impl Fn(&str) -> Span,
) {
    let Some(map) = entry.as_mapping() else {
        diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                format!("the '{}' entry must be a YAML mapping", platform.as_str()),
            )
            .with_span(key_span(platform.as_str())),
        );
        return;
    };
    for key in map.keys().filter_map(Value::as_str) {
        if !PLATFORM_KEYS.contains(&key) {
            let mut diag = Diagnostic::error(
                Code::CfgUnknownKey,
                format!(
                    "unknown key '{key}' in the '{}' app entry",
                    platform.as_str()
                ),
            )
            .with_span(key_span(key));
            if let Some(hint) = suggest(key, PLATFORM_KEYS) {
                diag = diag.with_help(hint);
            }
            diagnostics.push(diag);
        }
    }

    let id = map.get("id").and_then(Value::as_str);
    let package = map.get("package").and_then(Value::as_str);
    let launch_activity = map.get("launch_activity").and_then(Value::as_str);
    let url = map.get("url").and_then(Value::as_str);

    if platform == Platform::Web {
        match url {
            Some(url) if valid_web_url(url) => {}
            Some(url) => diagnostics.push(
                Diagnostic::error(
                    Code::CfgInvalidValue,
                    format!("invalid web app url '{url}'; expected http://, https:// or file://"),
                )
                .with_span(key_span("url")),
            ),
            None => diagnostics.push(
                Diagnostic::error(Code::CfgMissingField, "the web app entry needs 'url'")
                    .with_span(key_span("url")),
            ),
        }
        for forbidden in ["id", "package", "launch_activity"] {
            if map.contains_key(forbidden) {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgInvalidValue,
                        format!("'{forbidden}' is not valid for the web platform; use 'url'"),
                    )
                    .with_span(key_span(forbidden)),
                );
            }
        }
        return;
    }

    match id {
        Some(text) if valid_app_id(text) => {}
        Some(text) => diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                format!("invalid app id '{text}'; expected a dotted id such as com.acme.shop"),
            )
            .with_span(key_span("id")),
        ),
        None => diagnostics.push(
            Diagnostic::error(
                Code::CfgMissingField,
                format!("the '{}' app entry needs 'id'", platform.as_str()),
            )
            .with_span(key_span("id")),
        ),
    }
    if map.get("package").is_some() && package.is_none_or(|text| text.trim().is_empty()) {
        diagnostics.push(
            Diagnostic::error(Code::CfgInvalidValue, "'package' must be a non-empty path")
                .with_span(key_span("package")),
        );
    }
    if map.get("launch_activity").is_some() {
        if !matches!(platform, Platform::Android | Platform::AndroidTv) {
            diagnostics.push(
                Diagnostic::error(
                    Code::CfgInvalidValue,
                    "'launch_activity' is only valid on android/androidtv",
                )
                .with_span(key_span("launch_activity")),
            );
        } else if launch_activity.is_none_or(|text| text.trim().is_empty()) {
            diagnostics.push(
                Diagnostic::error(
                    Code::CfgInvalidValue,
                    "'launch_activity' must be a non-empty activity name",
                )
                .with_span(key_span("launch_activity")),
            );
        }
    }
    if url.is_some() {
        diagnostics.push(
            Diagnostic::error(
                Code::CfgInvalidValue,
                "'url' is only valid on the web platform",
            )
            .with_span(key_span("url")),
        );
    }
}

fn valid_web_url(text: &str) -> bool {
    text.starts_with("http://") || text.starts_with("https://") || text.starts_with("file://")
}

fn valid_app_id(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> &'static Path {
        Path::new("impresario.apps.config")
    }

    const GOOD: &str = "apps:\n  - name: shop\n    description: Customer shop app\n    platforms:\n      android: { id: com.acme.shop, package: ./build/app.apk }\n      androidtv: { id: com.acme.shop.tv }\n      web: { url: 'https://staging.acme.com' }\n    options:\n      clear_data: true\n      launch_params:\n        mode: staging\n";

    #[test]
    fn parses_good_apps() {
        let outcome = parse_apps(GOOD, path());
        assert!(
            !outcome.diagnostics.has_errors(),
            "{:?}",
            outcome.diagnostics
        );
        let apps = outcome.config.expect("apps");
        assert_eq!(apps.apps.len(), 1);
        let app = &apps.apps[0];
        assert_eq!(app.name, "shop");
        assert_eq!(
            app.platform(Platform::Android)
                .and_then(|p| p.id.as_deref()),
            Some("com.acme.shop")
        );
        assert!(app.options.clear_data);
        assert_eq!(
            app.options.launch_params.get("mode").map(String::as_str),
            Some("staging")
        );
    }

    #[test]
    fn unknown_app_key_and_bad_id_are_reported() {
        let source = "apps:\n  - name: shop\n    platform: { android: { id: com.acme.shop } }\n";
        let outcome = parse_apps(source, path());
        let codes: Vec<&str> = outcome
            .diagnostics
            .iter()
            .map(|diag| diag.code.as_str())
            .collect();
        assert!(codes.contains(&"IMP-CFG-001"));
        assert!(codes.contains(&"IMP-CFG-002"));
    }

    #[test]
    fn web_requires_url_and_rejects_package() {
        let source = "apps:\n  - name: site\n    platforms:\n      web: { package: ./site.zip }\n";
        let outcome = parse_apps(source, path());
        assert!(outcome.diagnostics.has_errors());
        let text = format!("{:?}", outcome.diagnostics);
        assert!(text.contains("url"));
        assert!(text.contains("package"));
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let source = "apps:\n  - name: shop\n    platforms: { web: { url: 'https://a' } }\n  - name: shop\n    platforms: { web: { url: 'https://b' } }\n";
        let outcome = parse_apps(source, path());
        assert!(outcome
            .diagnostics
            .iter()
            .any(|diag| diag.code == Code::CfgDuplicateApp));
    }

    #[test]
    fn app_id_validation() {
        assert!(valid_app_id("com.acme.shop"));
        assert!(valid_app_id("AbCdEf1234.Shop"));
        assert!(!valid_app_id("shop"));
        assert!(!valid_app_id("com..shop"));
        assert!(!valid_app_id(".shop"));
    }
}
