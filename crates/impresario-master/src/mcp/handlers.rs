//! Tool implementations: workspace introspection, validation, device
//! checks, runs and run artifacts.
//!
//! Device-touching tools (`impresario_devices_check`, `impresario_run`)
//! serialize on a process-wide lock: two operations must never drive one
//! device at the same time. Everything else is read-only against the
//! workspace or the report directory.

use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use base64::Engine as _;
use impresario_clients::agent_client::DeviceInfo;
use impresario_core::apps::{self, AppConfig};
use impresario_core::config::{self, DeviceConfig, ImpresarioConfig};
use impresario_core::diag::Diagnostics;
use impresario_core::platforms::Platform;
use impresario_core::settings;
use impresario_core::support::{self, CheckVerb, Verb};
use impresario_core::workspace;
use serde_json::{json, Map, Value};

use crate::cli;
use crate::mcp::protocol::Server;
use crate::mcp::tools::ToolOutput;
use crate::report::{ActReport, AttemptReport, DeviceReport, FailureReport, PlayReport, RunReport};
use crate::reporters;

/// Text artifacts larger than this are truncated in the response.
const TEXT_CAP: usize = 2 * 1024 * 1024;
/// Image artifacts larger than this are refused (16 MiB).
const IMAGE_CAP: u64 = 16 * 1024 * 1024;
/// Default file reporters for MCP runs: the console reporter writes to
/// stdout, which is the protocol channel.
const DEFAULT_REPORTERS: &str = "json,junit";

/// Serializes `impresario_devices_check` and `impresario_run`.
static DEVICE_LOCK: Mutex<()> = Mutex::new(());

fn lock_devices() -> MutexGuard<'static, ()> {
    DEVICE_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

// ---------------------------------------------------------------------------
// Small argument/JSON helpers
// ---------------------------------------------------------------------------

fn diagnostics_text(diagnostics: &Diagnostics) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.one_line())
        .collect::<Vec<_>>()
        .join("\n")
}

fn diagnostics_json(diagnostics: &Diagnostics) -> Value {
    Value::Array(
        diagnostics
            .iter()
            .map(|diagnostic| {
                let mut value = serde_json::to_value(diagnostic).unwrap_or(Value::Null);
                // Agents act on the stable code string (IMP-DSL-008), not
                // the serde enum name (DslUnknownScene).
                if let Some(object) = value.as_object_mut() {
                    object.insert("code".to_string(), json!(diagnostic.code.as_str()));
                }
                value
            })
            .collect(),
    )
}

fn arg_strings(arguments: &Value, key: &str) -> Result<Vec<String>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(text) => values.push(text.to_string()),
                    None => return Err(format!("'{key}' must be an array of strings")),
                }
            }
            Ok(values)
        }
        Some(_) => Err(format!("'{key}' must be an array of strings")),
    }
}

fn arg_string(arguments: &Value, key: &str) -> Result<Option<String>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("'{key}' must be a string")),
    }
}

fn arg_u64(arguments: &Value, key: &str) -> Result<Option<u64>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => match number.as_u64() {
            Some(value) => Ok(Some(value)),
            None => Err(format!("'{key}' must be a non-negative integer")),
        },
        Some(_) => Err(format!("'{key}' must be a non-negative integer")),
    }
}

fn arg_bool(arguments: &Value, key: &str) -> Result<Option<bool>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("'{key}' must be a boolean")),
    }
}

fn relative_to(base: &Path, path: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => path.display().to_string(),
    }
}

/// A bare play name or simple glob (no directory component) is resolved
/// against the configured plays directory before meeting the CLI's
/// root-relative `--play` semantics: the name `impresario_workspace_info`
/// reports should just work when handed back to `impresario_run`.
fn qualify_play(pattern: &str, plays_dir: &Path) -> String {
    let normalized = pattern.replace('\\', "/");
    let simple = !normalized.is_empty() && !normalized.contains('/') && !normalized.contains(':');
    if !simple {
        return pattern.to_string();
    }
    let dir = plays_dir.to_string_lossy().replace('\\', "/");
    let dir = dir.trim_end_matches('/');
    let dir = dir.strip_prefix("./").unwrap_or(dir);
    if dir.is_empty() || dir == "." {
        normalized
    } else {
        format!("{dir}/{normalized}")
    }
}

fn qualify_plays(server: &Server, plays: Vec<String>) -> Result<Vec<String>, String> {
    if plays.is_empty() {
        return Ok(plays);
    }
    let config_outcome = settings::load_default_layers(&server.config_path, &server.base);
    let Some(config) = config_outcome.config else {
        return Err(format!(
            "the workspace is not usable:\n{}",
            diagnostics_text(&config_outcome.diagnostics)
        ));
    };
    Ok(plays
        .into_iter()
        .map(|play| qualify_play(&play, &config.plays_dir))
        .collect())
}

struct WorkspaceConfig {
    config: ImpresarioConfig,
    devices: Vec<DeviceConfig>,
    apps: Vec<AppConfig>,
}

fn apps_path(server: &Server) -> PathBuf {
    server.base.join("impresario.apps.config")
}

fn load_workspace_config(server: &Server) -> Result<WorkspaceConfig, String> {
    let config_outcome = settings::load_default_layers(&server.config_path, &server.base);
    let devices_outcome = config::load_devices_merged(&server.devices_path);
    let apps_outcome = apps::load_apps_merged(&apps_path(server));
    let mut diagnostics = Diagnostics::new();
    diagnostics.extend(config_outcome.diagnostics.iter().cloned());
    diagnostics.extend(devices_outcome.diagnostics.iter().cloned());
    diagnostics.extend(apps_outcome.diagnostics.iter().cloned());
    let (Some(config), Some(devices), Some(apps)) = (
        config_outcome.config,
        devices_outcome.config,
        apps_outcome.config,
    ) else {
        return Err(format!(
            "the workspace is not usable:\n{}",
            diagnostics_text(&diagnostics)
        ));
    };
    Ok(WorkspaceConfig {
        config,
        devices: devices.devices,
        apps: apps.apps,
    })
}

fn device_json(device: &DeviceConfig) -> Value {
    json!({
        "name": device.name,
        "platform": device.platform.as_str(),
        "transport": device.effective_transport().as_str(),
        "connection": {
            "host": device.connection.host,
            "port": device.connection.port,
            "serial": device.connection.serial,
            "udid": device.connection.udid,
            "endpoint": device.connection.endpoint,
        },
        "auth": {
            "key": device.auth.key,
            "identity": device.auth.identity,
            "profile": device.auth.profile,
        },
        "labels": device.labels,
        "connector_address": device.connector_address(),
        "target_endpoint": device.target_endpoint(),
        "support": platform_support(device.platform),
    })
}

fn platform_support(platform: Platform) -> Value {
    let mut actions = Map::new();
    for verb in Verb::ALL {
        actions.insert(
            verb.as_str().to_string(),
            json!(support::action_support(platform, *verb).glyph()),
        );
    }
    let mut checks = Map::new();
    for check in CheckVerb::ALL {
        checks.insert(
            check.as_str().to_string(),
            json!(support::check_support(platform, *check).glyph()),
        );
    }
    json!({
        "legend": "✅ supported · ◐ limited (see impresario://docs/parity) · ✖ unsupported",
        "actions": Value::Object(actions),
        "checks": Value::Object(checks),
    })
}

// ---------------------------------------------------------------------------
// impresario_workspace_info
// ---------------------------------------------------------------------------

pub(crate) fn workspace_info(server: &Server, _arguments: &Value) -> ToolOutput {
    match workspace_info_value(server) {
        Ok(value) => ToolOutput::json(value),
        Err(message) => ToolOutput::error(message),
    }
}

/// The workspace summary; shared with the `impresario://workspace/info`
/// resource.
pub(crate) fn workspace_info_value(server: &Server) -> Result<Value, String> {
    let WorkspaceConfig {
        config,
        devices,
        apps: app_list,
    } = load_workspace_config(server)?;
    let mut diagnostics = Diagnostics::new();
    let scenes = workspace::load_scenes(&server.base.join(&config.scenes_dir));
    let plays = workspace::load_plays(&server.base.join(&config.plays_dir));
    diagnostics.extend(scenes.diagnostics.iter().cloned());
    diagnostics.extend(plays.diagnostics.iter().cloned());

    let device_values: Vec<Value> = devices.iter().map(device_json).collect();
    let app_values: Vec<Value> = app_list
        .iter()
        .map(|app| {
            json!({
                "name": app.name,
                "description": app.description,
                "platforms": app.platforms.keys().map(|platform| platform.as_str()).collect::<Vec<_>>(),
                "clear_data": app.options.clear_data,
            })
        })
        .collect();

    let scene_values: Vec<Value> = scenes
        .workspace
        .scenes
        .iter()
        .map(|scene| {
            json!({
                "name": scene.name,
                "file": relative_to(&server.base, &scene.path),
                "pieces": scene
                    .pieces
                    .iter()
                    .map(|piece| piece.name.clone())
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    let play_values: Vec<Value> = plays
        .workspace
        .plays
        .iter()
        .map(|play| {
            json!({
                "name": play.name,
                "file": relative_to(&server.base, &play.path),
                "description": play.description,
                "app": play.app.as_ref().map(|app| app.name.clone()),
                "acts": play
                    .acts
                    .iter()
                    .map(|act| {
                        json!({
                            "name": act.name,
                            "tags": act
                                .tags
                                .iter()
                                .map(|tag| tag.name.clone())
                                .collect::<Vec<_>>(),
                            "platforms": act
                                .platforms
                                .iter()
                                .map(|platform| platform.name.clone())
                                .collect::<Vec<_>>(),
                        })
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    // Union of every act tag, sorted for stable output.
    let mut tags: Vec<&str> = Vec::new();
    for play in &plays.workspace.plays {
        for act in &play.acts {
            for tag in &act.tags {
                if !tags.contains(&tag.name.as_str()) {
                    tags.push(tag.name.as_str());
                }
            }
        }
    }
    tags.sort_unstable();

    Ok(json!({
        "workspace": server.base.display().to_string(),
        "config_file": server.config_path.display().to_string(),
        "devices_file": server.devices_path.display().to_string(),
        "apps_file": apps_path(server).display().to_string(),
        "project": config.project,
        "scenes_dir": server.base.join(&config.scenes_dir).display().to_string(),
        "plays_dir": server.base.join(&config.plays_dir).display().to_string(),
        "report_output": server.base.join(&config.report_output).display().to_string(),
        "timeout_default_ms": config.timeout_default_ms,
        "retries": config.retries,
        "parallel_devices": config.parallel_devices,
        "screenshot_on_failure": config.screenshot_on_failure,
        "reporters": config
            .reporters
            .iter()
            .map(|reporter| reporter.as_str())
            .collect::<Vec<_>>(),
        "devices": device_values,
        "apps": app_values,
        "scenes": scene_values,
        "plays": play_values,
        "tags": tags,
        "diagnostics": diagnostics_json(&diagnostics),
    }))
}

// ---------------------------------------------------------------------------
// impresario_validate
// ---------------------------------------------------------------------------

pub(crate) fn validate(server: &Server, arguments: &Value) -> ToolOutput {
    let paths = match arg_strings(arguments, "paths") {
        Ok(paths) => paths,
        Err(message) => return ToolOutput::error(message),
    };
    let resolved: Vec<PathBuf> = paths
        .iter()
        .map(|path| {
            let path = Path::new(path);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                server.base.join(path)
            }
        })
        .collect();
    let outcome = cli::validate_workspace(
        &server.base,
        &server.config_path,
        &server.devices_path,
        &resolved,
    );
    let errors = outcome.diagnostics.errors().count();
    let warnings = outcome.diagnostics.warnings().count();
    let ok = outcome.config_ok && outcome.devices_ok && errors == 0;
    let headline = if !outcome.config_ok || !outcome.devices_ok {
        "validate: the workspace configuration is not readable".to_string()
    } else if ok {
        format!(
            "validate: ok ({} scene(s), {} theatre(s), {} play(s), {} device(s), {warnings} warning(s))",
            outcome.scenes, outcome.theatres, outcome.plays, outcome.devices
        )
    } else {
        format!("validate: {errors} error(s), {warnings} warning(s)")
    };
    let value = json!({
        "ok": ok,
        "errors": errors,
        "warnings": warnings,
        "scenes": outcome.scenes,
        "theatres": outcome.theatres,
        "plays": outcome.plays,
        "devices": outcome.devices,
        "diagnostics": diagnostics_json(&outcome.diagnostics),
    });
    ToolOutput::json_with_prefix(value, headline)
}

// ---------------------------------------------------------------------------
// impresario_devices_list / impresario_devices_check
// ---------------------------------------------------------------------------

pub(crate) fn devices_list(server: &Server, _arguments: &Value) -> ToolOutput {
    let outcome = config::load_devices_merged(&server.devices_path);
    let Some(devices) = outcome.config else {
        return ToolOutput::error(format!(
            "configuration errors in {}:\n{}",
            server.devices_path.display(),
            diagnostics_text(&outcome.diagnostics)
        ));
    };
    let values: Vec<Value> = devices.devices.iter().map(device_json).collect();
    let headline = format!("{} device(s) configured", values.len());
    ToolOutput::json_with_prefix(
        json!({
            "devices": values,
            "diagnostics": diagnostics_json(&outcome.diagnostics),
        }),
        headline,
    )
}

pub(crate) fn devices_check(server: &Server, arguments: &Value) -> ToolOutput {
    let requested = match arg_strings(arguments, "devices") {
        Ok(devices) => devices,
        Err(message) => return ToolOutput::error(message),
    };
    let loaded = match load_workspace_config(server) {
        Ok(loaded) => loaded,
        Err(message) => return ToolOutput::error(message),
    };
    let selected = match cli::select_devices(&loaded.devices, &requested) {
        Ok(selected) => selected,
        Err(message) => return ToolOutput::error(message),
    };
    if selected.is_empty() {
        return ToolOutput::error(format!(
            "no devices configured (see {})",
            server.devices_path.display()
        ));
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return ToolOutput::error(format!("cannot start the async runtime: {err}")),
    };
    let token = cli::agent_token();
    let master_version = env!("CARGO_PKG_VERSION").to_string();
    let timeout_ms = loaded.config.agent_start_timeout_ms;
    let guard = lock_devices();
    let results: Vec<Result<DeviceInfo, String>> = runtime.block_on(async {
        let mut results = Vec::new();
        for device in &selected {
            results.push(
                cli::check_device(device, None, timeout_ms, token.as_deref(), &master_version)
                    .await,
            );
        }
        results
    });
    drop(guard);

    let mut failed = 0usize;
    let rows: Vec<Value> = selected
        .iter()
        .zip(results)
        .map(|(device, result)| match result {
            Ok(info) => json!({
                "name": device.name,
                "platform": device.platform.as_str(),
                "ok": true,
                "device_info": {
                    "model": info.model,
                    "os_name": info.os_name,
                    "os_version": info.os_version,
                    "runtime": info.runtime,
                    "screen_width_px": info.screen_width_px,
                    "screen_height_px": info.screen_height_px,
                    "pixel_ratio": info.pixel_ratio,
                },
            }),
            Err(message) => {
                failed += 1;
                json!({
                    "name": device.name,
                    "platform": device.platform.as_str(),
                    "ok": false,
                    "error": message,
                })
            }
        })
        .collect();
    let headline = format!(
        "devices check: {} of {} device(s) ok",
        selected.len() - failed,
        selected.len()
    );
    ToolOutput::json_with_prefix(
        json!({
            "ok": failed == 0,
            "failed": failed,
            "total": selected.len(),
            "devices": rows,
        }),
        headline,
    )
}

// ---------------------------------------------------------------------------
// impresario_run
// ---------------------------------------------------------------------------

pub(crate) fn run(server: &Server, arguments: &Value) -> ToolOutput {
    let plays = match arg_strings(arguments, "plays") {
        Ok(plays) => plays,
        Err(message) => return ToolOutput::error(message),
    };
    let plays = match qualify_plays(server, plays) {
        Ok(plays) => plays,
        Err(message) => return ToolOutput::error(message),
    };
    let devices = match arg_strings(arguments, "devices") {
        Ok(devices) => devices,
        Err(message) => return ToolOutput::error(message),
    };
    let tags = match arg_strings(arguments, "tags") {
        Ok(tags) => tags,
        Err(message) => return ToolOutput::error(message),
    };
    let exclude_tags = match arg_strings(arguments, "exclude_tags") {
        Ok(tags) => tags,
        Err(message) => return ToolOutput::error(message),
    };
    let acts = match arg_strings(arguments, "acts") {
        Ok(acts) => acts,
        Err(message) => return ToolOutput::error(message),
    };
    let vars: Vec<String> = match arguments.get("vars") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => {
            let mut vars = Vec::with_capacity(map.len());
            for (key, value) in map {
                match value.as_str() {
                    Some(value) => vars.push(format!("{key}={value}")),
                    None => return ToolOutput::error("'vars' must map names to string values"),
                }
            }
            vars
        }
        Some(_) => return ToolOutput::error("'vars' must be an object of string values"),
    };
    let act_args: Vec<String> = match arguments.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => {
            let mut args = Vec::with_capacity(map.len());
            for (key, value) in map {
                match value.as_str() {
                    Some(value) => args.push(format!("{key}={value}")),
                    None => return ToolOutput::error("'args' must map ACT.PARAM to strings"),
                }
            }
            args
        }
        Some(_) => return ToolOutput::error("'args' must be an object of string values"),
    };
    let timeout = match arg_u64(arguments, "timeout_ms") {
        Ok(timeout) => timeout,
        Err(message) => return ToolOutput::error(message),
    };
    if let Some(timeout) = timeout {
        if !plan_bounds_timeout(timeout) {
            return ToolOutput::error("'timeout_ms' must be between 100 and 600000");
        }
    }
    let retries = match arg_u64(arguments, "retries") {
        Ok(None) => None,
        Ok(Some(value)) if value <= u32::MAX as u64 => Some(value as u32),
        Ok(Some(_)) => return ToolOutput::error("'retries' is out of range"),
        Err(message) => return ToolOutput::error(message),
    };
    if let Some(retries) = retries {
        if !plan_bounds_retries(retries) {
            return ToolOutput::error("'retries' must be between 0 and 5");
        }
    }
    let fail_fast = match arg_bool(arguments, "fail_fast") {
        Ok(value) => value.unwrap_or(false),
        Err(message) => return ToolOutput::error(message),
    };
    let reporter_list = match arg_strings(arguments, "reporters") {
        Ok(reporters) => reporters,
        Err(message) => return ToolOutput::error(message),
    };
    let app = match arguments.get("app") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if !name.trim().is_empty() => Some(name.clone()),
        Some(_) => return ToolOutput::error("'app' must be a non-empty string"),
    };
    let reporter_spec = if reporter_list.is_empty() {
        DEFAULT_REPORTERS.to_string()
    } else {
        if reporter_list.iter().any(|name| name.trim() == "console") {
            return ToolOutput::error(
                "the console reporter writes to stdout, which is the MCP protocol channel; \
use json, junit or html — every reporter still lands in the run directory",
            );
        }
        let spec = reporter_list.join(",");
        if let Err(message) = reporters::parse_list(&spec) {
            return ToolOutput::error(message);
        }
        spec
    };

    let args = cli::RunArgs {
        play: plays,
        app,
        device: devices,
        tag: tags,
        exclude_tag: exclude_tags,
        act: acts,
        var: vars,
        arg: act_args,
        timeout,
        retries,
        reporter: Some(reporter_spec),
        fail_fast,
    };
    let guard = lock_devices();
    let outcome = cli::run_workspace(
        &server.base,
        &server.config_path,
        &server.devices_path,
        &args,
        false,
        false,
    );
    drop(guard);

    match (&outcome.report, &outcome.run_dir, &outcome.run_id) {
        (Some(report), Some(run_dir), Some(run_id)) => {
            let summary = report.summary();
            let headline = format!(
                "run {run_id}: {} (exit {}) — {} act(s) passed, {} failed, {} skipped; run dir: {}",
                report.status.as_str(),
                outcome.exit_code,
                summary.acts_passed,
                summary.acts_failed,
                summary.acts_skipped,
                run_dir.display()
            );
            ToolOutput::json_with_prefix(run_summary(report, run_dir), headline)
        }
        _ => {
            let mut lines = outcome.messages.clone();
            if let Some(detail) = &outcome.detail {
                lines.push(detail.clone());
            }
            let detail = if lines.is_empty() {
                "no report was produced".to_string()
            } else {
                lines.join("\n")
            };
            ToolOutput::error(format!("the run did not execute:\n{detail}"))
        }
    }
}

fn plan_bounds_timeout(timeout: u64) -> bool {
    (100..=600_000).contains(&timeout)
}

fn plan_bounds_retries(retries: u32) -> bool {
    retries <= 5
}

fn run_summary(report: &RunReport, run_dir: &Path) -> Value {
    let summary = report.summary();
    json!({
        "run_id": report.run_id,
        "status": report.status.as_str(),
        "exit_code": report.exit_code,
        "started_at": report.started_at,
        "duration_ms": report.duration_ms,
        "run_dir": run_dir.display().to_string(),
        "summary": {
            "acts_passed": summary.acts_passed,
            "acts_failed": summary.acts_failed,
            "acts_skipped": summary.acts_skipped,
            "acts_aborted": summary.acts_aborted,
            "steps_total": summary.steps_total,
            "steps_failed": summary.steps_failed,
        },
        "devices": report.devices.iter().map(device_summary).collect::<Vec<_>>(),
    })
}

fn device_summary(device: &DeviceReport) -> Value {
    json!({
        "name": device.name,
        "platform": device.platform,
        "error": device.error.as_ref().map(|error| json!({
            "code": error.code,
            "message": error.message,
            "details": error.details,
        })),
        "plays": device.plays.iter().map(play_summary).collect::<Vec<_>>(),
    })
}

fn play_summary(play: &PlayReport) -> Value {
    json!({
        "name": play.name,
        "file": play.file,
        "acts": play.acts.iter().map(act_summary).collect::<Vec<_>>(),
    })
}

fn act_summary(act: &ActReport) -> Value {
    json!({
        "name": act.name,
        "tags": act.tags,
        "status": act.status.as_str(),
        "flaky": act.flaky,
        "attempts": act.attempts.len(),
        "duration_ms": reporters::act_duration(act),
        "failures": act
            .attempts
            .iter()
            .filter_map(|attempt| attempt.failure.as_ref().map(|failure| failure_summary(attempt, failure)))
            .collect::<Vec<_>>(),
    })
}

fn failure_summary(attempt: &AttemptReport, failure: &FailureReport) -> Value {
    // The last few log lines are usually enough to see why a step failed;
    // the full tail is in report.json.
    let tail_start = failure.log_tail.len().saturating_sub(10);
    let log_tail = failure
        .log_tail
        .get(tail_start..)
        .map(<[String]>::to_vec)
        .unwrap_or_default();
    json!({
        "attempt": attempt.n,
        "step_index": failure.step_index,
        "code": failure.code,
        "message": failure.message,
        "details": failure.details,
        "screenshot": failure.screenshot,
        "log_tail": log_tail,
    })
}

// ---------------------------------------------------------------------------
// impresario_report_get / impresario_artifacts_list / impresario_artifact_get
// ---------------------------------------------------------------------------

fn report_output_dir(server: &Server) -> Result<PathBuf, String> {
    let outcome = settings::load_default_layers(&server.config_path, &server.base);
    let Some(config) = outcome.config else {
        return Err(format!(
            "configuration errors in {}:\n{}",
            server.config_path.display(),
            diagnostics_text(&outcome.diagnostics)
        ));
    };
    Ok(server.base.join(&config.report_output))
}

/// The named run directory, or the newest one (run ids start with a
/// timestamp, so a name sort is a time sort).
fn resolve_run_dir(report_dir: &Path, run_id: Option<&str>) -> Result<PathBuf, String> {
    let Some(run_id) = run_id else {
        let entries = std::fs::read_dir(report_dir)
            .map_err(|err| format!("cannot list {}: {err}", report_dir.display()))?;
        let mut runs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        runs.sort();
        return runs
            .pop()
            .ok_or_else(|| format!("no runs found under {}", report_dir.display()));
    };
    if run_id.is_empty()
        || run_id.contains(['/', '\\'])
        || run_id.contains("..")
        || run_id.starts_with('.')
    {
        return Err(format!("invalid run id '{run_id}'"));
    }
    let candidate = report_dir.join(run_id);
    if candidate.is_dir() {
        Ok(candidate)
    } else {
        Err(format!("no run '{run_id}' under {}", report_dir.display()))
    }
}

pub(crate) fn report_get(server: &Server, arguments: &Value) -> ToolOutput {
    let run_id = match arg_string(arguments, "run_id") {
        Ok(run_id) => run_id,
        Err(message) => return ToolOutput::error(message),
    };
    let report_dir = match report_output_dir(server) {
        Ok(dir) => dir,
        Err(message) => return ToolOutput::error(message),
    };
    let run_dir = match resolve_run_dir(&report_dir, run_id.as_deref()) {
        Ok(dir) => dir,
        Err(message) => return ToolOutput::error(message),
    };
    let path = run_dir.join("report.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            return ToolOutput::error(format!(
                "cannot read {}: {err} — the run may have been written without the json reporter",
                path.display()
            ));
        }
    };
    let report: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(err) => {
            return ToolOutput::error(format!("{} is not valid JSON: {err}", path.display()));
        }
    };
    let name = run_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    ToolOutput::json_with_prefix(
        json!({
            "run_id": name,
            "run_dir": run_dir.display().to_string(),
            "report": report,
        }),
        format!("report.json of run {name}"),
    )
}

pub(crate) fn artifacts_list(server: &Server, arguments: &Value) -> ToolOutput {
    let run_id = match arg_string(arguments, "run_id") {
        Ok(run_id) => run_id,
        Err(message) => return ToolOutput::error(message),
    };
    let report_dir = match report_output_dir(server) {
        Ok(dir) => dir,
        Err(message) => return ToolOutput::error(message),
    };
    let run_dir = match resolve_run_dir(&report_dir, run_id.as_deref()) {
        Ok(dir) => dir,
        Err(message) => return ToolOutput::error(message),
    };
    let mut artifacts: Vec<Value> = Vec::new();
    collect_artifacts(&run_dir, &run_dir, &mut artifacts);
    artifacts.sort_by_key(|artifact| {
        artifact
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    let headline = format!(
        "{} artifact(s) under {}",
        artifacts.len(),
        run_dir.display()
    );
    ToolOutput::json_with_prefix(
        json!({
            "run_dir": run_dir.display().to_string(),
            "artifacts": artifacts,
        }),
        headline,
    )
}

fn collect_artifacts(root: &Path, dir: &Path, out: &mut Vec<Value>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::debug!("mcp: cannot list {}: {err}", dir.display());
            return;
        }
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_artifacts(root, &path, out);
            continue;
        }
        out.push(json!({
            "path": relative_to(root, &path),
            "bytes": metadata.len(),
            "kind": artifact_kind(&path),
        }));
    }
}

fn artifact_kind(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png" | "jpg" | "jpeg" | "webp") => "image",
        Some("json" | "xml" | "html" | "htm" | "txt" | "log" | "md" | "yaml" | "yml" | "csv") => {
            "text"
        }
        _ => "binary",
    }
}

pub(crate) fn artifact_get(server: &Server, arguments: &Value) -> ToolOutput {
    let relative = match arg_string(arguments, "path") {
        Ok(Some(path)) if !path.trim().is_empty() => path,
        Ok(_) => {
            return ToolOutput::error("'path' is required (relative to the run directory)");
        }
        Err(message) => return ToolOutput::error(message),
    };
    let run_id = match arg_string(arguments, "run_id") {
        Ok(run_id) => run_id,
        Err(message) => return ToolOutput::error(message),
    };
    let report_dir = match report_output_dir(server) {
        Ok(dir) => dir,
        Err(message) => return ToolOutput::error(message),
    };
    let run_dir = match resolve_run_dir(&report_dir, run_id.as_deref()) {
        Ok(dir) => dir,
        Err(message) => return ToolOutput::error(message),
    };
    let target = match safe_join(&run_dir, &relative) {
        Ok(path) => path,
        Err(message) => return ToolOutput::error(message),
    };
    let canonical_run = match run_dir.canonicalize() {
        Ok(path) => path,
        Err(err) => {
            return ToolOutput::error(format!("cannot resolve {}: {err}", run_dir.display()))
        }
    };
    let canonical = match target.canonicalize() {
        Ok(path) => path,
        Err(err) => {
            return ToolOutput::error(format!(
                "no artifact '{relative}' in {}: {err}",
                run_dir.display()
            ));
        }
    };
    if !canonical.starts_with(&canonical_run) {
        return ToolOutput::error(format!("'{relative}' resolves outside the run directory"));
    }
    let metadata = match std::fs::metadata(&canonical) {
        Ok(metadata) => metadata,
        Err(err) => {
            return ToolOutput::error(format!("cannot stat {}: {err}", canonical.display()));
        }
    };
    if metadata.is_dir() {
        return ToolOutput::error(format!(
            "'{relative}' is a directory; use impresario_artifacts_list"
        ));
    }
    let name = canonical
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| relative.clone());
    match artifact_kind(&canonical) {
        "image" => {
            if metadata.len() > IMAGE_CAP {
                return ToolOutput::error(format!(
                    "'{relative}' is {} bytes; the image cap is {IMAGE_CAP} bytes",
                    metadata.len()
                ));
            }
            let bytes = match std::fs::read(&canonical) {
                Ok(bytes) => bytes,
                Err(err) => {
                    return ToolOutput::error(format!(
                        "cannot read {}: {err}",
                        canonical.display()
                    ));
                }
            };
            let mime = match canonical
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("webp") => "image/webp",
                _ => "image/png",
            };
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            ToolOutput::image(
                encoded,
                mime,
                format!("{name} ({relative}, {} bytes)", bytes.len()),
            )
        }
        "text" => {
            let bytes = match std::fs::read(&canonical) {
                Ok(bytes) => bytes,
                Err(err) => {
                    return ToolOutput::error(format!(
                        "cannot read {}: {err}",
                        canonical.display()
                    ));
                }
            };
            let truncated = bytes.len() > TEXT_CAP;
            let shown = bytes.get(..TEXT_CAP).unwrap_or(bytes.as_slice());
            let mut text = String::from_utf8_lossy(shown).into_owned();
            if truncated {
                text.push_str(&format!(
                    "\n[truncated: showing {TEXT_CAP} of {} bytes]",
                    bytes.len()
                ));
            }
            ToolOutput::message(text)
        }
        kind => ToolOutput::error(format!(
            "'{relative}' is a {kind} artifact ({} bytes); only text and image artifacts are readable",
            metadata.len()
        )),
    }
}

/// Join a client-supplied relative path under `root` without letting it
/// escape (no absolute paths, no `..`, no prefixes).
fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute() {
        return Err(format!(
            "'{relative}' must be relative to the run directory"
        ));
    }
    let mut out = root.to_path_buf();
    for component in relative_path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("'{relative}' must not contain '{component:?}'"));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_confines_paths_to_the_run_directory() {
        let root = Path::new("run");
        let joined = safe_join(root, "screenshots/d1/failure-1.png").expect("nested path");
        assert!(joined.ends_with("screenshots/d1/failure-1.png"));
        assert!(safe_join(root, "./report.json")
            .expect("dot prefix")
            .ends_with("report.json"));
        for escape in ["../impresario.config", "a/../../secret", "/etc/passwd"] {
            assert!(
                safe_join(root, escape).is_err(),
                "'{escape}' must not escape"
            );
        }
    }

    #[test]
    fn bare_play_names_and_simple_globs_resolve_in_the_plays_directory() {
        let plays = Path::new("./plays");
        assert_eq!(
            qualify_play("mobile_smoke.play", plays),
            "plays/mobile_smoke.play"
        );
        assert_eq!(qualify_play("*.play", plays), "plays/*.play");
        assert_eq!(qualify_play("plays/x.play", plays), "plays/x.play");
        assert_eq!(qualify_play("sub\\x.play", plays), "sub\\x.play");
        assert_eq!(qualify_play("C:/abs/x.play", plays), "C:/abs/x.play");
        assert_eq!(qualify_play("x.play", Path::new(".")), "x.play");
        assert_eq!(qualify_play("x.play", Path::new("specs/")), "specs/x.play");
    }

    #[test]
    fn artifact_kinds_cover_reports_and_screenshots() {
        assert_eq!(artifact_kind(Path::new("a/b/report.json")), "text");
        assert_eq!(artifact_kind(Path::new("junit.xml")), "text");
        assert_eq!(artifact_kind(Path::new("report.html")), "text");
        assert_eq!(artifact_kind(Path::new("shots/failure-1.PNG")), "image");
        assert_eq!(artifact_kind(Path::new("blob.bin")), "binary");
    }

    #[test]
    fn resolve_run_dir_picks_the_newest_and_rejects_traversal() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(temp.path().join("20261001-000000-aaaa")).expect("run 1");
        std::fs::create_dir(temp.path().join("20261002-000000-bbbb")).expect("run 2");
        let latest = resolve_run_dir(temp.path(), None).expect("latest");
        assert!(latest.ends_with("20261002-000000-bbbb"));
        let named = resolve_run_dir(temp.path(), Some("20261001-000000-aaaa")).expect("named");
        assert!(named.ends_with("20261001-000000-aaaa"));
        assert!(resolve_run_dir(temp.path(), Some("../escape")).is_err());
        assert!(resolve_run_dir(temp.path(), Some("missing")).is_err());
    }
}
