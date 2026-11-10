//! Read endpoints and routing for the embedded console.
//!
//! Reads never mutate workspace state. They either call the same library
//! functions the TUI uses or run the CLI with a read-only subcommand and
//! `--json`; no action is dispatched from a GET.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use impresario_core::apps;
use impresario_core::config;
use impresario_core::probe;
use impresario_core::settings;
use impresario_core::workspace;
use serde_json::{json, Value};

use super::{
    actions, artifact_json, assets, http, list_files, ok, read_text, resources, tail,
    workspace_file, ApiError, WebState,
};
use crate::definitions;
use crate::jobs::{self, JobStore};
use crate::resident;

/// Route one request.
pub fn handle(state: &WebState, request: &http::Request) -> http::Response {
    if !state.host_allowed(request.header("host")) {
        return ApiError::new(
            403,
            "host_not_allowed",
            if state.loopback || !state.allow_remote {
                "this console only answers loopback Host names; open it through 127.0.0.1 or localhost"
            } else {
                "the Host header is missing or empty"
            },
        )
        .response();
    }
    if request.method == "POST" {
        if !same_origin(request) {
            return ApiError::new(
                403,
                "origin_not_allowed",
                "the Origin header must match the Host header",
            )
            .response();
        }
        if let Some(content_type) = request.header("content-type") {
            let content_type = content_type.to_ascii_lowercase();
            if !content_type.starts_with("application/json") {
                return ApiError::new(
                    415,
                    "unsupported_media_type",
                    "mutations require Content-Type: application/json",
                )
                .response();
            }
        } else {
            return ApiError::new(
                415,
                "unsupported_media_type",
                "mutations require Content-Type: application/json",
            )
            .response();
        }
        let Some(name) = request.path.strip_prefix("/api/actions/") else {
            return ApiError::new(
                405,
                "method_not_allowed",
                "POST is only accepted on /api/actions/{action}",
            )
            .response();
        };
        if name.contains('/') || name.is_empty() {
            return ApiError::new(404, "unknown_action", "unknown action").response();
        }
        return actions::handle(state, name, &request.body);
    }
    if request.method != "GET" && request.method != "HEAD" {
        return ApiError::new(
            405,
            "method_not_allowed",
            format!("{} is not supported", request.method),
        )
        .response();
    }
    match route_get(state, request) {
        Ok(response) => response,
        Err(error) => error.response(),
    }
}

fn same_origin(request: &http::Request) -> bool {
    let Some(origin) = request.header("origin") else {
        // Non-browser clients do not send Origin; the JSON content type and
        // the Host check still apply.
        return true;
    };
    let Some(host) = request.header("host") else {
        return false;
    };
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    if authority.eq_ignore_ascii_case(host) {
        return true;
    }
    let default_port = if scheme.eq_ignore_ascii_case("https") {
        "443"
    } else {
        "80"
    };
    normalize_authority(authority, default_port)
        .eq_ignore_ascii_case(&normalize_authority(host, default_port))
}

fn normalize_authority(authority: &str, default_port: &str) -> String {
    match authority.rsplit_once(':') {
        Some((name, port)) if !name.contains(']') || name.ends_with(']') => {
            if port == default_port {
                name.to_string()
            } else {
                format!("{name}:{port}")
            }
        }
        _ => authority.to_string(),
    }
}

fn route_get(state: &WebState, request: &http::Request) -> Result<http::Response, ApiError> {
    let path = request.path.as_str();
    match path {
        "/" | "/index.html" => Ok(http::Response::text(
            200,
            "text/html; charset=utf-8",
            assets::INDEX_HTML,
        )),
        "/assets/app.css" => Ok(http::Response::text(
            200,
            "text/css; charset=utf-8",
            assets::APP_CSS,
        )),
        "/assets/app.js" => Ok(http::Response::text(
            200,
            "text/javascript; charset=utf-8",
            assets::APP_JS,
        )),
        "/favicon.svg" => Ok(http::Response::text(
            200,
            "image/svg+xml",
            assets::FAVICON_SVG,
        )),
        "/healthz" => Ok(ok(json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
        }))),
        "/api/meta" => Ok(ok(meta(state))),
        "/api/schema" => Ok(ok(schema())),
        "/api/guide" => Ok(ok(guide(state))),
        "/api/overview" => Ok(ok(overview(state)?)),
        "/api/workspaces" => Ok(ok(workspaces(state)?)),
        "/api/tests" => Ok(ok(tests_tree(state)?)),
        "/api/tests/file" => Ok(ok(test_file(state, request)?)),
        "/api/tests/show" => Ok(ok(test_show(state, request)?)),
        "/api/tests/grep" => Ok(ok(test_grep(state, request)?)),
        "/api/coverage" => Ok(ok(coverage(state)?)),
        "/api/probe/support" => Ok(ok(probe_support(state)?)),
        "/api/groups" => Ok(ok(groups(state)?)),
        "/api/devices" => Ok(ok(devices(state)?)),
        "/api/apps" => Ok(ok(apps_list(state)?)),
        "/api/config" => Ok(ok(config_values(state)?)),
        "/api/doctor" => Ok(ok(doctor(state)?)),
        "/api/resident" => Ok(ok(resident_status(state)?)),
        "/api/resident/log" => Ok(ok(resident_log(state, request)?)),
        "/api/dumps" => Ok(ok(dumps(state, request)?)),
        "/api/dumps/artifact" => Ok(ok(dump_artifact(state, request)?)),
        "/api/notifications" => Ok(ok(notifications(state, request)?)),
        "/api/jobs" => Ok(ok(jobs_list(state, request)?)),
        "/api/runs" => Ok(ok(runs_list(state, request)?)),
        "/api/docs" => Ok(ok(json!({ "docs": resources::list() }))),
        "/api/actions/history" => Ok(ok(json!({
            "actions": state.action_history(),
        }))),
        _ => {
            let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            match parts.as_slice() {
                ["api", "jobs", id] => Ok(ok(job_detail(state, id)?)),
                ["api", "jobs", id, "log"] => Ok(ok(job_log(state, request, id)?)),
                ["api", "jobs", id, "events"] => Ok(ok(job_events(state, request, id)?)),
                ["api", "jobs", id, "report"] => Ok(ok(job_report(state, id)?)),
                ["api", "jobs", id, "artifacts"] => Ok(ok(job_artifacts(state, id)?)),
                ["api", "jobs", id, "artifact"] => Ok(ok(job_artifact(state, request, id)?)),
                ["api", "runs", id] => Ok(ok(run_detail(state, id)?)),
                ["api", "runs", id, "artifacts"] => Ok(ok(run_artifacts(state, id)?)),
                ["api", "runs", id, "artifact"] => Ok(ok(run_artifact(state, request, id)?)),
                ["api", "docs", slug] => Ok(ok(doc_json(slug)?)),
                ["api", "docs", slug, "raw"] => doc_raw(slug),
                _ => Err(ApiError::new(404, "not_found", "no such endpoint")),
            }
        }
    }
}

fn meta(state: &WebState) -> Value {
    json!({
        "product": "impresario",
        "version": env!("CARGO_PKG_VERSION"),
        "workspace": state.base(),
        "workspace_name": state
            .base()
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default(),
        "config": state.config_path(),
        "devices_config": state.devices_path(),
        "apps_config": state.apps_path(),
        "server": {
            "bind": state.bind,
            "port": state.port,
            "loopback": state.loopback,
            "allow_remote": state.allow_remote,
            "started_at": state.started_at,
            "uptime_ms": state.started.elapsed().as_millis() as u64,
            "max_connections": super::MAX_CONNECTIONS,
            "max_body_bytes": super::MAX_BODY_BYTES,
        },
        "actions": actions::names(),
        "docs": resources::list(),
        "security": {
            "authentication": false,
            "tls": false,
            "remote": state.allow_remote && !state.loopback,
            "note": "Loopback by default. A non-loopback bind is trusted-network-only and must be opted into with --allow-remote.",
        },
    })
}

fn schema() -> Value {
    json!({
        "envelope": {
            "success": { "ok": true, "data": "the endpoint payload" },
            "failure": { "ok": false, "error": { "code": "stable code", "message": "human text", "details": "optional structured context" } },
        },
        "transport": "HTTP/1.1 over the console port; one request per connection; responses close the connection.",
        "security": {
            "host": "Loopback Host names by default; --allow-remote accepts any non-empty Host.",
            "mutations": "POST /api/actions/{action} with Content-Type: application/json and a matching Origin/Host pair.",
            "actions": "Curated allowlist; arbitrary command strings are never accepted and values starting with '-' are rejected.",
            "authentication": "None. Keep the default loopback bind unless the network is trusted.",
        },
        "actions": actions::schema(),
        "endpoints": endpoints(),
        "errors": {
            "bad_request": 400,
            "invalid_parameter": 400,
            "host_not_allowed": 403,
            "origin_not_allowed": 403,
            "path_outside_workspace": 403,
            "not_found": 404,
            "unknown_action": 404,
            "method_not_allowed": 405,
            "active_workspace": 409,
            "too_large": 413,
            "unsupported_media_type": 415,
            "command_failed": 422,
            "internal": 500,
            "busy": 503,
        },
        "docs": resources::list(),
    })
}

fn endpoints() -> Value {
    json!([
        { "method": "GET", "path": "/api/meta", "description": "Server, workspace, action and doc metadata." },
        { "method": "GET", "path": "/api/schema", "description": "This document: action parameters, endpoints and errors." },
        { "method": "GET", "path": "/api/guide", "description": "Guided first-run flow with concrete action bodies." },
        { "method": "GET", "path": "/api/overview", "description": "Workspace counts, validation, jobs and resident summary." },
        { "method": "GET", "path": "/api/workspaces", "description": "Active and remembered workspaces." },
        { "method": "GET", "path": "/api/tests", "description": "The tests --json tree (scenes, theatres, plays)." },
        { "method": "GET", "path": "/api/tests/file?path=...", "description": "Read one workspace DSL file." },
        { "method": "GET", "path": "/api/tests/show?name=...", "description": "Declaration source and references." },
        { "method": "GET", "path": "/api/tests/grep?q=...&ignore_case=1", "description": "Search workspace DSL files." },
        { "method": "GET", "path": "/api/coverage", "description": "Stage coverage matrix with missing devices." },
        { "method": "GET", "path": "/api/probe/support", "description": "Plan-time probe support per configured device." },
        { "method": "GET", "path": "/api/groups", "description": "Effective actor groups." },
        { "method": "GET", "path": "/api/devices", "description": "Effective device definitions." },
        { "method": "GET", "path": "/api/apps", "description": "Effective app definitions." },
        { "method": "GET", "path": "/api/config", "description": "Effective settings, their layers and the setting schema." },
        { "method": "GET", "path": "/api/doctor", "description": "The doctor report as JSON." },
        { "method": "GET", "path": "/api/resident", "description": "Resident runner status, schedules and watchers." },
        { "method": "GET", "path": "/api/resident/log?limit=...", "description": "Tail of the resident service log." },
        { "method": "GET", "path": "/api/dumps?watcher=...", "description": "Watcher failure-dump bundles." },
        { "method": "GET", "path": "/api/dumps/artifact?path=...", "description": "Read one file from a dump bundle." },
        { "method": "GET", "path": "/api/notifications?limit=...", "description": "Recent resident notifications." },
        { "method": "GET", "path": "/api/jobs?all=1", "description": "Job records, newest first." },
        { "method": "GET", "path": "/api/jobs/{id}", "description": "One job record." },
        { "method": "GET", "path": "/api/jobs/{id}/log?limit=...", "description": "Job console log tail." },
        { "method": "GET", "path": "/api/jobs/{id}/events?limit=...", "description": "Structured execution events." },
        { "method": "GET", "path": "/api/jobs/{id}/report", "description": "The job's report.json." },
        { "method": "GET", "path": "/api/jobs/{id}/artifacts", "description": "Files in the job run directory." },
        { "method": "GET", "path": "/api/jobs/{id}/artifact?path=...", "description": "One run artifact (text or base64 image)." },
        { "method": "GET", "path": "/api/runs?limit=...", "description": "Report directories under report_output." },
        { "method": "GET", "path": "/api/runs/{id}", "description": "One run's report.json." },
        { "method": "GET", "path": "/api/runs/{id}/artifacts", "description": "Files in a report directory." },
        { "method": "GET", "path": "/api/runs/{id}/artifact?path=...", "description": "One report artifact." },
        { "method": "GET", "path": "/api/docs", "description": "Embedded documentation index." },
        { "method": "GET", "path": "/api/docs/{slug}", "description": "One embedded document as JSON." },
        { "method": "GET", "path": "/api/docs/{slug}/raw", "description": "One embedded document as text/markdown." },
        { "method": "GET", "path": "/api/actions/history", "description": "Recent mutations executed by this console process." },
    ])
}

fn guide(state: &WebState) -> Value {
    let base = state.base().display().to_string();
    json!({
        "workspace": base.clone(),
        "steps": [
            {
                "id": "workspace",
                "title": "Choose a workspace",
                "body": "The console is a CLI client. Pick a remembered workspace or add the current directory.",
                "endpoint": "GET /api/workspaces",
                "action": { "name": "workspace_use", "body": { "path": base } },
            },
            {
                "id": "validate",
                "title": "Validate the workspace",
                "body": "Parse every scene, theatre and play before touching a device. Fix errors here first.",
                "endpoint": "GET /api/tests",
                "action": { "name": "validate", "body": {} },
            },
            {
                "id": "devices",
                "title": "Check the devices",
                "body": "Run transport prepare, handshake and DeviceInfo for the devices you plan to use.",
                "endpoint": "GET /api/devices",
                "action": { "name": "device_check", "body": {} },
            },
            {
                "id": "preview",
                "title": "Preview the run",
                "body": "Resolve devices, plays and acts without touching hardware. This is the fastest way to catch an audience or selector mismatch.",
                "endpoint": "GET /api/tests",
                "action": { "name": "run", "body": { "dry_run": true } },
            },
            {
                "id": "run",
                "title": "Start a detached run",
                "body": "Submit the run to the job supervisor and follow its status, log, events, report and artifacts.",
                "endpoint": "GET /api/jobs",
                "action": { "name": "run", "body": {} },
            },
            {
                "id": "inspect",
                "title": "Inspect the evidence",
                "body": "Open the report, failure screenshots, probe evidence and watcher dumps. The same files are exposed to AI agents through MCP.",
                "endpoint": "GET /api/runs",
                "action": { "name": "probe", "body": { "kind": "showing", "device": "<device>" } },
            },
        ],
        "notes": [
            "Mutations are serialized by one server-wide lock.",
            "A run without dry_run is always detached; follow it through /api/jobs.",
            "All action responses include exit_code, stdout and stderr.",
        ],
    })
}

fn overview(state: &WebState) -> Result<Value, ApiError> {
    let base = state.base();
    let config_path = state.config_path();
    let config_outcome = crate::cli::load_effective_config(&base, &config_path);
    let mut errors: Vec<String> = config_outcome
        .diagnostics
        .errors()
        .map(|diagnostic| diagnostic.one_line())
        .collect();
    let config = config_outcome.config;
    let mut scenes = 0usize;
    let mut theatres = 0usize;
    let mut plays = 0usize;
    if let Some(config) = &config {
        let loaded_scenes = workspace::load_scenes(&base.join(&config.scenes_dir));
        let loaded_theatres = workspace::load_theatres(&base.join(&config.theatres_dir));
        let loaded_plays = workspace::load_plays(&base.join(&config.plays_dir));
        scenes = loaded_scenes.workspace.scenes.len();
        theatres = loaded_theatres.workspace.theatres.len();
        plays = loaded_plays.workspace.plays.len();
        for diagnostic in loaded_scenes
            .diagnostics
            .errors()
            .chain(loaded_theatres.diagnostics.errors())
            .chain(loaded_plays.diagnostics.errors())
        {
            errors.push(diagnostic.one_line());
        }
    }
    let devices_outcome = config::load_devices_merged(&state.devices_path());
    let device_count = devices_outcome
        .config
        .as_ref()
        .map(|config| config.devices.len())
        .unwrap_or(0);
    for diagnostic in devices_outcome.diagnostics.errors() {
        errors.push(diagnostic.one_line());
    }
    let apps_outcome = apps::load_apps_merged(&state.apps_path());
    let app_count = apps_outcome
        .config
        .as_ref()
        .map(|config| config.apps.len())
        .unwrap_or(0);
    for diagnostic in apps_outcome.diagnostics.errors() {
        errors.push(diagnostic.one_line());
    }
    let group_rows =
        definitions::group_rows(&state.home, &state.devices_path()).unwrap_or_default();
    let group_count = group_rows.len();

    let store = JobStore::new(&state.home);
    store.recover_interrupted();
    let records = store.list();
    let mut running = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;
    for record in &records {
        match record.status {
            jobs::JobStatus::Running | jobs::JobStatus::Queued => running += 1,
            jobs::JobStatus::Passed => passed += 1,
            jobs::JobStatus::Failed
            | jobs::JobStatus::Error
            | jobs::JobStatus::Stopped
            | jobs::JobStatus::Interrupted => failed += 1,
        }
    }
    let resident = resident::status::collect(&state.home, &config_path);
    let (resident_value, resident_error) = match resident {
        Ok(status) => (
            json!({
                "running": status.running,
                "heartbeat_stale": status.heartbeat_stale,
                "restart_required": status.restart_required,
                "schedules": status.schedules.len(),
                "watchers": status.watchers.len(),
            }),
            Value::Null,
        ),
        Err(message) => (Value::Null, json!(message)),
    };
    Ok(json!({
        "workspace": base,
        "project": config.as_ref().map(|config| config.project.clone()).unwrap_or_default(),
        "counts": {
            "scenes": scenes,
            "theatres": theatres,
            "plays": plays,
            "devices": device_count,
            "apps": app_count,
            "groups": group_count,
        },
        "validation": {
            "ok": errors.is_empty(),
            "errors": errors,
        },
        "jobs": {
            "total": records.len(),
            "running": running,
            "passed": passed,
            "failed": failed,
            "latest": records.first(),
        },
        "resident": resident_value,
        "resident_error": resident_error,
        "paths": {
            "config": config_path,
            "devices_config": state.devices_path(),
            "apps_config": state.apps_path(),
        },
    }))
}

fn workspaces(state: &WebState) -> Result<Value, ApiError> {
    let active = state.base();
    let global = settings::SettingsFile::load(&state.home.config_path());
    let mut paths = vec![active.clone()];
    if let Some(settings::SettingValue::List(values)) = global.values.get("workspaces") {
        for value in values {
            let path = PathBuf::from(value);
            if !paths.iter().any(|seen| same_path(seen, &path)) {
                paths.push(path);
            }
        }
    }
    let entries: Vec<Value> = paths
        .iter()
        .map(|path| {
            json!({
                "path": path,
                "name": path.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default(),
                "exists": path.is_dir(),
                "active": same_path(path, &active),
            })
        })
        .collect();
    Ok(json!({
        "active": active,
        "workspaces": entries,
        "source": global.path,
    }))
}

fn tests_tree(state: &WebState) -> Result<Value, ApiError> {
    let output = cli_json(state, &["tests", "--json"])?;
    Ok(output)
}

fn test_file(state: &WebState, request: &http::Request) -> Result<Value, ApiError> {
    let path = request
        .param("path")
        .ok_or_else(|| ApiError::bad("'path' is required"))?;
    let file = workspace_file(state, path)?;
    let extension = file
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !matches!(extension, "scene" | "theatre" | "play") {
        return Err(ApiError::bad(
            "only .scene, .theatre and .play files can be read here",
        ));
    }
    let text = read_text(&file, 2 * 1024 * 1024)?;
    let relative = file
        .strip_prefix(state.base())
        .unwrap_or(&file)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(json!({
        "path": relative,
        "bytes": text.len(),
        "text": text,
    }))
}

fn test_show(state: &WebState, request: &http::Request) -> Result<Value, ApiError> {
    let name = request
        .param("name")
        .ok_or_else(|| ApiError::bad("'name' is required"))?;
    let output = super::run_cli(
        state,
        &["tests".to_string(), "show".to_string(), name.to_string()],
    )
    .map_err(|message| ApiError::new(500, "spawn_failed", message))?;
    if output.code != 0 {
        return Err(command_failed(output));
    }
    Ok(json!({
        "name": name,
        "text": output.stdout,
    }))
}

fn test_grep(state: &WebState, request: &http::Request) -> Result<Value, ApiError> {
    let query = request
        .param("q")
        .or_else(|| request.param("pattern"))
        .ok_or_else(|| ApiError::bad("'q' is required"))?;
    let ignore_case = request
        .param("ignore_case")
        .is_some_and(|value| matches!(value, "1" | "true" | "yes"));
    let mut args = vec!["tests".to_string(), "grep".to_string(), query.to_string()];
    if ignore_case {
        args.push("--ignore-case".to_string());
    }
    let output = super::run_cli(state, &args)
        .map_err(|message| ApiError::new(500, "spawn_failed", message))?;
    if output.code != 0 {
        return Err(command_failed(output));
    }
    let mut matches = Vec::new();
    for line in output.stdout.lines() {
        if line.starts_with("tests: no matches") {
            continue;
        }
        let Some((file, rest)) = line.split_once(':') else {
            continue;
        };
        let Some((number, text)) = rest.split_once("  ") else {
            continue;
        };
        if let Ok(line_number) = number.parse::<u64>() {
            matches.push(json!({
                "file": file,
                "line": line_number,
                "text": text,
            }));
        }
    }
    Ok(json!({
        "query": query,
        "ignore_case": ignore_case,
        "text": output.stdout,
        "matches": matches,
    }))
}

fn coverage(state: &WebState) -> Result<Value, ApiError> {
    let tree = cli_json(state, &["tests", "--json"])?;
    let devices = device_entries(state)?;
    let mut stages: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>, usize)> = BTreeMap::new();
    if let Some(theatres) = tree.get("theatres").and_then(Value::as_array) {
        for theatre in theatres {
            let theatre_name = theatre
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let Some(declarations) = theatre.get("stages").and_then(Value::as_array) else {
                continue;
            };
            for stage in declarations {
                let Some(name) = stage.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let entry = stages.entry(name.to_string()).or_default();
                entry.1.insert(theatre_name.clone());
                entry.2 += 1;
                if let Some(platforms) = stage.get("platforms").and_then(Value::as_array) {
                    for platform in platforms {
                        if let Some(platform) = platform.as_str() {
                            entry.0.insert(platform.to_string());
                        }
                    }
                }
            }
        }
    }
    let mut rows = Vec::new();
    for (stage, (platforms, theatres, implementations)) in stages {
        let missing: Vec<Value> = devices
            .iter()
            .filter(|device| {
                let platform = device
                    .get("platform")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                !platforms.contains(platform)
            })
            .map(|device| {
                json!({
                    "name": device.get("name").cloned().unwrap_or(Value::Null),
                    "platform": device.get("platform").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        rows.push(json!({
            "stage": stage,
            "theatres": theatres.into_iter().collect::<Vec<_>>(),
            "implementations": implementations,
            "platforms": platforms.into_iter().collect::<Vec<_>>(),
            "missing": missing,
        }));
    }
    Ok(json!({
        "legend": "platforms lists the stage audience; missing lists configured devices whose platform has no implementation of this stage",
        "rows": rows,
    }))
}

fn probe_support(state: &WebState) -> Result<Value, ApiError> {
    let devices = device_entries(state)?;
    let rows: Vec<Value> = devices
        .iter()
        .filter_map(|device| {
            let name = device.get("name")?.as_str()?.to_string();
            let platform_name = device.get("platform")?.as_str()?.to_string();
            let platform = platform_name
                .parse::<impresario_core::platforms::Platform>()
                .ok()?;
            let support = probe::support(platform);
            Some(json!({
                "device": name,
                "platform": platform_name,
                "showing": support.showing,
                "player": support.player,
                "drm": support.drm,
                "strongest": support.strongest_class().as_str(),
                "properties": support.properties,
                "providers": support.providers,
                "sources": support.sources,
            }))
        })
        .collect();
    Ok(json!({
        "legend": "Plan-time support. A live probe can still be unverifiable when no evidence source is reachable.",
        "devices": rows,
    }))
}

fn groups(state: &WebState) -> Result<Value, ApiError> {
    let rows = definitions::group_rows(&state.home, &state.devices_path())
        .map_err(|message| ApiError::new(500, "read_failed", message))?;
    let groups: Vec<Value> = rows
        .iter()
        .map(|group| {
            json!({
                "name": group.name,
                "members": group.members,
                "source": group.source.as_str(),
                "devices": group.devices.iter().map(|device| json!({
                    "name": device.name,
                    "platform": device.platform,
                })).collect::<Vec<_>>(),
                "error": group.error,
            })
        })
        .collect();
    Ok(json!({ "groups": groups }))
}

fn device_entries(state: &WebState) -> Result<Vec<Value>, ApiError> {
    let outcome = config::load_devices_merged(&state.devices_path());
    let Some(config) = outcome.config else {
        return Err(ApiError::new(
            422,
            "config_invalid",
            "the devices configuration has errors",
        )
        .with_details(json!({
            "errors": outcome.diagnostics.errors().map(|diagnostic| diagnostic.one_line()).collect::<Vec<_>>(),
        })));
    };
    let workspace_names = definitions::device_names(&state.devices_path());
    let global_names = definitions::device_names(&state.home.devices_path());
    Ok(config
        .devices
        .iter()
        .map(|device| {
            let source = if workspace_names.contains(&device.name) {
                "workspace"
            } else if global_names.contains(&device.name) {
                "global"
            } else {
                "effective"
            };
            json!({
                "name": device.name,
                "type": device.device_type.as_str(),
                "ip": device.ip,
                "platform": device.platform.as_str(),
                "transport": device.effective_transport().as_str(),
                "labels": device.labels,
                "source": source,
            })
        })
        .collect())
}

fn devices(state: &WebState) -> Result<Value, ApiError> {
    let devices = device_entries(state)?;
    Ok(json!({ "devices": devices }))
}

fn apps_list(state: &WebState) -> Result<Value, ApiError> {
    let outcome = apps::load_apps_merged(&state.apps_path());
    let Some(config) = outcome.config else {
        return Err(ApiError::new(
            422,
            "config_invalid",
            "the apps configuration has errors",
        )
        .with_details(json!({
            "errors": outcome.diagnostics.errors().map(|diagnostic| diagnostic.one_line()).collect::<Vec<_>>(),
        })));
    };
    let workspace_names = definitions::app_names(&state.apps_path());
    let global_names = definitions::app_names(&state.home.apps_path());
    let apps: Vec<Value> = config
        .apps
        .iter()
        .map(|app| {
            let source = if workspace_names.contains(&app.name) {
                "workspace"
            } else if global_names.contains(&app.name) {
                "global"
            } else {
                "effective"
            };
            let mut value = serde_json::to_value(app).unwrap_or(Value::Null);
            if let Value::Object(map) = &mut value {
                map.insert("source".to_string(), json!(source));
            }
            value
        })
        .collect();
    Ok(json!({
        "apps": apps,
        "path": state.apps_path(),
    }))
}

fn config_values(state: &WebState) -> Result<Value, ApiError> {
    let output = cli_json(state, &["config", "list", "--json"])?;
    let definitions: Vec<Value> = settings::SETTINGS
        .iter()
        .map(|definition| {
            json!({
                "key": definition.key,
                "default": definition.default,
                "scope": match definition.scope {
                    settings::Scope::Both => "workspace or global",
                    settings::Scope::GlobalOnly => "global only",
                },
                "type": definition.kind.type_name(),
                "doc": definition.doc,
            })
        })
        .collect();
    Ok(json!({
        "values": output,
        "definitions": definitions,
        "paths": {
            "global": state.home.config_path(),
            "workspace": state.config_path(),
        },
    }))
}

fn doctor(state: &WebState) -> Result<Value, ApiError> {
    let output = super::run_cli(state, &["doctor".to_string(), "--json".to_string()])
        .map_err(|message| ApiError::new(500, "spawn_failed", message))?;
    let report = serde_json::from_str::<Value>(output.stdout.trim()).unwrap_or(Value::Null);
    Ok(json!({
        "exit_code": output.code,
        "report": report,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

fn resident_status(state: &WebState) -> Result<Value, ApiError> {
    let status = resident::status::collect(&state.home, &state.config_path())
        .map_err(|message| ApiError::new(422, "resident_unavailable", message))?;
    let dumps = resident::status::dumps(&state.base(), None).unwrap_or_default();
    Ok(json!({
        "status": status,
        "dumps": dumps,
        "log_path": state.home.service_log_path(),
    }))
}

fn resident_log(state: &WebState, request: &http::Request) -> Result<Value, ApiError> {
    let limit = request
        .param("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20_000)
        .min(1_000_000);
    let path = state.home.service_log_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(ApiError::new(
                500,
                "read_failed",
                format!("cannot read {}: {error}", path.display()),
            ))
        }
    };
    Ok(json!({
        "path": path,
        "limit": limit,
        "text": tail(&text, limit),
    }))
}

fn dumps(state: &WebState, request: &http::Request) -> Result<Value, ApiError> {
    let watcher = request.param("watcher");
    let dumps = resident::status::dumps(&state.base(), watcher)
        .map_err(|message| ApiError::new(500, "read_failed", message))?;
    Ok(json!({ "dumps": dumps }))
}

fn dump_artifact(state: &WebState, request: &http::Request) -> Result<Value, ApiError> {
    let raw = request
        .param("path")
        .ok_or_else(|| ApiError::bad("'path' is required"))?;
    let file = workspace_file(state, raw)?;
    let relative = file
        .strip_prefix(state.base())
        .unwrap_or(&file)
        .to_string_lossy()
        .replace('\\', "/");
    if !relative.starts_with("reports/watch/") {
        return Err(ApiError::new(
            403,
            "path_outside_dumps",
            "dump artifacts must live under reports/watch/",
        ));
    }
    artifact_json(&file)
}

fn notifications(state: &WebState, request: &http::Request) -> Result<Value, ApiError> {
    let limit = request
        .param("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20)
        .min(200);
    let output = super::run_cli(
        state,
        &[
            "notify".to_string(),
            "list".to_string(),
            "--json".to_string(),
            "--limit".to_string(),
            limit.to_string(),
        ],
    )
    .map_err(|message| ApiError::new(500, "spawn_failed", message))?;
    if output.code != 0 {
        return Err(command_failed(output));
    }
    Ok(json!({
        "limit": limit,
        "notifications": output.json().unwrap_or(Value::Null),
        "stdout": output.stdout,
    }))
}

fn jobs_list(state: &WebState, request: &http::Request) -> Result<Value, ApiError> {
    let all = request
        .param("all")
        .is_none_or(|value| !matches!(value, "0" | "false" | "no"));
    let store = JobStore::new(&state.home);
    store.recover_interrupted();
    let records: Vec<jobs::JobRecord> = store
        .list()
        .into_iter()
        .filter(|record| all || !record.status.terminal())
        .collect();
    Ok(json!({
        "all": all,
        "jobs": records,
    }))
}

fn job_detail(state: &WebState, id: &str) -> Result<Value, ApiError> {
    let store = JobStore::new(&state.home);
    store.recover_interrupted();
    let record = store
        .read(id)
        .map_err(|message| ApiError::new(500, "read_failed", message))?
        .ok_or_else(|| ApiError::new(404, "not_found", format!("unknown job '{id}'")))?;
    Ok(json!({
        "job": record,
        "report_available": store.report_path(id).is_file(),
    }))
}

fn job_log(state: &WebState, request: &http::Request, id: &str) -> Result<Value, ApiError> {
    let limit = request
        .param("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20_000)
        .min(1_000_000);
    let store = JobStore::new(&state.home);
    if store
        .read(id)
        .map_err(|message| ApiError::new(500, "read_failed", message))?
        .is_none()
    {
        return Err(ApiError::new(
            404,
            "not_found",
            format!("unknown job '{id}'"),
        ));
    }
    let path = store.console_path(id);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    Ok(json!({
        "id": id,
        "path": path,
        "limit": limit,
        "text": tail(&text, limit),
    }))
}

fn job_events(state: &WebState, request: &http::Request, id: &str) -> Result<Value, ApiError> {
    let limit = request
        .param("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(500)
        .min(10_000);
    let store = JobStore::new(&state.home);
    if store
        .read(id)
        .map_err(|message| ApiError::new(500, "read_failed", message))?
        .is_none()
    {
        return Err(ApiError::new(
            404,
            "not_found",
            format!("unknown job '{id}'"),
        ));
    }
    let mut events = jobs::read_events(&store.events_path(id));
    if events.len() > limit {
        let keep_from = events.len().saturating_sub(limit);
        events.drain(..keep_from);
    }
    Ok(json!({
        "id": id,
        "events": events,
    }))
}

fn job_report(state: &WebState, id: &str) -> Result<Value, ApiError> {
    let store = JobStore::new(&state.home);
    let record = store
        .read(id)
        .map_err(|message| ApiError::new(500, "read_failed", message))?
        .ok_or_else(|| ApiError::new(404, "not_found", format!("unknown job '{id}'")))?;
    let report = store
        .read_report(id)
        .map_err(|message| ApiError::new(404, "report_unavailable", message))?;
    Ok(json!({
        "id": id,
        "job": record,
        "report": report,
    }))
}

fn job_artifacts(state: &WebState, id: &str) -> Result<Value, ApiError> {
    let store = JobStore::new(&state.home);
    let record = store
        .read(id)
        .map_err(|message| ApiError::new(500, "read_failed", message))?
        .ok_or_else(|| ApiError::new(404, "not_found", format!("unknown job '{id}'")))?;
    let run_dir = PathBuf::from(&record.run_dir);
    Ok(json!({
        "id": id,
        "run_dir": run_dir,
        "artifacts": list_files(&run_dir, &run_dir, 500),
    }))
}

fn job_artifact(state: &WebState, request: &http::Request, id: &str) -> Result<Value, ApiError> {
    let store = JobStore::new(&state.home);
    let record = store
        .read(id)
        .map_err(|message| ApiError::new(500, "read_failed", message))?
        .ok_or_else(|| ApiError::new(404, "not_found", format!("unknown job '{id}'")))?;
    let run_dir = PathBuf::from(&record.run_dir);
    let raw = request
        .param("path")
        .ok_or_else(|| ApiError::bad("'path' is required"))?;
    let file = confined_file(&run_dir, raw)?;
    artifact_json(&file)
}

fn runs_list(state: &WebState, request: &http::Request) -> Result<Value, ApiError> {
    let limit = request
        .param("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100)
        .min(500);
    let base = state.base();
    let config_path = state.config_path();
    let outcome = crate::cli::load_effective_config(&base, &config_path);
    let Some(config) = outcome.config else {
        return Err(ApiError::new(
            422,
            "config_invalid",
            "the workspace configuration has errors",
        )
        .with_details(json!({
            "errors": outcome.diagnostics.errors().map(|diagnostic| diagnostic.one_line()).collect::<Vec<_>>(),
        })));
    };
    let report_root = if config.report_output.is_absolute() {
        config.report_output.clone()
    } else {
        base.join(&config.report_output)
    };
    let mut runs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&report_root) {
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.chars()
                            .next()
                            .is_some_and(|first| first.is_ascii_digit())
                    })
            })
            .filter(|path| path.join("report.json").is_file())
            .collect();
        dirs.sort();
        dirs.reverse();
        for dir in dirs.into_iter().take(limit) {
            let name = dir
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut summary = json!({ "id": name, "path": dir });
            if let Ok(text) = std::fs::read_to_string(dir.join("report.json")) {
                if let Ok(report) = serde_json::from_str::<Value>(&text) {
                    if let Some(map) = summary.as_object_mut() {
                        map.insert(
                            "status".to_string(),
                            report.get("status").cloned().unwrap_or(Value::Null),
                        );
                        map.insert(
                            "exit_code".to_string(),
                            report.get("exit_code").cloned().unwrap_or(Value::Null),
                        );
                        map.insert(
                            "started_at".to_string(),
                            report.get("started_at").cloned().unwrap_or(Value::Null),
                        );
                        map.insert(
                            "duration_ms".to_string(),
                            report.get("duration_ms").cloned().unwrap_or(Value::Null),
                        );
                        map.insert(
                            "devices".to_string(),
                            report
                                .get("devices")
                                .and_then(Value::as_array)
                                .map_or(0, Vec::len)
                                .into(),
                        );
                        map.insert(
                            "plays".to_string(),
                            report
                                .get("devices")
                                .and_then(Value::as_array)
                                .map(|devices| {
                                    devices
                                        .iter()
                                        .map(|device| {
                                            device
                                                .get("plays")
                                                .and_then(Value::as_array)
                                                .map_or(0, Vec::len)
                                        })
                                        .sum::<usize>()
                                })
                                .unwrap_or(0)
                                .into(),
                        );
                    }
                }
            }
            runs.push(summary);
        }
    }
    Ok(json!({
        "report_output": report_root,
        "runs": runs,
    }))
}

fn run_dir(state: &WebState, id: &str) -> Result<PathBuf, ApiError> {
    if !safe_name(id) {
        return Err(ApiError::bad("'id' is not a valid run id"));
    }
    let base = state.base();
    let config_path = state.config_path();
    let outcome = crate::cli::load_effective_config(&base, &config_path);
    let Some(config) = outcome.config else {
        return Err(ApiError::new(
            422,
            "config_invalid",
            "the workspace configuration has errors",
        ));
    };
    let root = if config.report_output.is_absolute() {
        config.report_output
    } else {
        base.join(&config.report_output)
    };
    let dir = root.join(id);
    if !dir.is_dir() {
        return Err(ApiError::new(
            404,
            "not_found",
            format!("unknown run '{id}'"),
        ));
    }
    Ok(dir)
}

fn run_detail(state: &WebState, id: &str) -> Result<Value, ApiError> {
    let dir = run_dir(state, id)?;
    let text = std::fs::read_to_string(dir.join("report.json")).map_err(|error| {
        ApiError::new(
            404,
            "report_unavailable",
            format!("cannot read report.json: {error}"),
        )
    })?;
    let report = serde_json::from_str::<Value>(&text).map_err(|error| {
        ApiError::new(
            500,
            "report_invalid",
            format!("report.json is not valid JSON: {error}"),
        )
    })?;
    Ok(json!({
        "id": id,
        "path": dir,
        "report": report,
    }))
}

fn run_artifacts(state: &WebState, id: &str) -> Result<Value, ApiError> {
    let dir = run_dir(state, id)?;
    Ok(json!({
        "id": id,
        "run_dir": dir,
        "artifacts": list_files(&dir, &dir, 500),
    }))
}

fn run_artifact(state: &WebState, request: &http::Request, id: &str) -> Result<Value, ApiError> {
    let dir = run_dir(state, id)?;
    let raw = request
        .param("path")
        .ok_or_else(|| ApiError::bad("'path' is required"))?;
    let file = confined_file(&dir, raw)?;
    artifact_json(&file)
}

fn doc_json(slug: &str) -> Result<Value, ApiError> {
    let doc = resources::get(slug)
        .ok_or_else(|| ApiError::new(404, "not_found", format!("unknown document '{slug}'")))?;
    Ok(json!({
        "slug": doc.slug,
        "title": doc.title,
        "description": doc.description,
        "text": doc.text,
    }))
}

fn doc_raw(slug: &str) -> Result<http::Response, ApiError> {
    let doc = resources::get(slug)
        .ok_or_else(|| ApiError::new(404, "not_found", format!("unknown document '{slug}'")))?;
    Ok(http::Response::text(
        200,
        "text/markdown; charset=utf-8",
        doc.text,
    ))
}

fn cli_json(state: &WebState, args: &[&str]) -> Result<Value, ApiError> {
    let args: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
    let output = super::run_cli(state, &args)
        .map_err(|message| ApiError::new(500, "spawn_failed", message))?;
    if output.code != 0 {
        return Err(command_failed(output));
    }
    output.json().map_err(|message| {
        ApiError::new(500, "invalid_json", message).with_details(json!({
            "stdout": output.stdout,
            "stderr": output.stderr,
        }))
    })
}

fn command_failed(output: super::CliOutput) -> ApiError {
    ApiError::new(422, "command_failed", output.message()).with_details(json!({
        "exit_code": output.code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

fn confined_file(root: &Path, raw: &str) -> Result<PathBuf, ApiError> {
    if raw.trim().is_empty() {
        return Err(ApiError::bad("'path' is required"));
    }
    if raw.contains('\0') {
        return Err(ApiError::bad("the path contains a NUL byte"));
    }
    let root = root
        .canonicalize()
        .map_err(|error| ApiError::new(404, "not_found", error.to_string()))?;
    let candidate = Path::new(raw);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let resolved = candidate.canonicalize().map_err(|error| {
        ApiError::new(
            404,
            "not_found",
            format!("{}: {error}", candidate.display()),
        )
    })?;
    if !resolved.starts_with(&root) {
        return Err(ApiError::new(
            403,
            "path_outside_workspace",
            "the requested path is outside the run directory",
        ));
    }
    if !resolved.is_file() {
        return Err(ApiError::new(404, "not_found", "the path is not a file"));
    }
    Ok(resolved)
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('.')
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_accepts_the_request_host() {
        let request = http::Request {
            method: "POST".to_string(),
            path: "/api/actions/validate".to_string(),
            query: BTreeMap::new(),
            headers: BTreeMap::from([
                ("host".to_string(), "127.0.0.1:7810".to_string()),
                ("origin".to_string(), "http://127.0.0.1:7810".to_string()),
            ]),
            body: Vec::new(),
        };
        assert!(same_origin(&request));
    }

    #[test]
    fn same_origin_rejects_other_sites() {
        let request = http::Request {
            method: "POST".to_string(),
            path: "/api/actions/validate".to_string(),
            query: BTreeMap::new(),
            headers: BTreeMap::from([
                ("host".to_string(), "127.0.0.1:7810".to_string()),
                ("origin".to_string(), "https://evil.example".to_string()),
            ]),
            body: Vec::new(),
        };
        assert!(!same_origin(&request));
    }

    #[test]
    fn safe_names_are_single_segments() {
        assert!(safe_name("20261110-120000-abcd"));
        assert!(!safe_name("../escape"));
        assert!(!safe_name(".hidden"));
        assert!(!safe_name("a/b"));
    }

    #[test]
    fn host_names_strip_brackets_and_ports() {
        assert_eq!(super::super::host_name("127.0.0.1:7810"), "127.0.0.1");
        assert_eq!(super::super::host_name("[::1]:7810"), "::1");
        assert_eq!(super::super::host_name("localhost"), "localhost");
    }

    fn state_in(temp: &tempfile::TempDir) -> WebState {
        use impresario_core::home::Home;
        let base = temp.path().join("workspace");
        std::fs::create_dir_all(&base).expect("workspace");
        WebState::new(
            Home::at(temp.path().join("home")),
            base.clone(),
            base.join("impresario.config"),
            base.join("impresario.devices.config"),
            "127.0.0.1".to_string(),
            7810,
            true,
            false,
        )
    }

    fn request(method: &str, path: &str) -> http::Request {
        http::Request {
            method: method.to_string(),
            path: path.to_string(),
            query: BTreeMap::new(),
            headers: BTreeMap::from([("host".to_string(), "127.0.0.1:7810".to_string())]),
            body: Vec::new(),
        }
    }

    fn body(response: &http::Response) -> Value {
        serde_json::from_slice(&response.body).expect("json body")
    }

    #[test]
    fn serves_the_console_assets_and_read_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = state_in(&temp);

        let index = handle(&state, &request("GET", "/"));
        assert_eq!(index.status, 200);
        assert_eq!(index.content_type, "text/html; charset=utf-8");
        assert!(String::from_utf8_lossy(&index.body).contains("<title>Impresario console</title>"));

        let script = handle(&state, &request("GET", "/assets/app.js"));
        assert_eq!(script.status, 200);
        assert!(String::from_utf8_lossy(&script.body).contains("'use strict'"));

        let meta = body(&handle(&state, &request("GET", "/api/meta")));
        assert_eq!(meta["ok"], json!(true));
        assert_eq!(
            meta["data"]["workspace"],
            json!(state.base().display().to_string())
        );
        assert!(meta["data"]["actions"]
            .as_array()
            .is_some_and(|actions| !actions.is_empty()));
        assert_eq!(
            meta["data"]["server"]["max_connections"],
            json!(super::super::MAX_CONNECTIONS)
        );

        let schema = body(&handle(&state, &request("GET", "/api/schema")));
        assert!(schema["data"]["actions"]
            .as_array()
            .is_some_and(|actions| !actions.is_empty()));

        let doc = body(&handle(&state, &request("GET", "/api/docs/web")));
        assert_eq!(doc["data"]["slug"], json!("web"));
        assert!(doc["data"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("/api/actions/")));
    }

    #[test]
    fn rejects_foreign_hosts_and_unsafe_mutations() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = state_in(&temp);

        let mut foreign = request("GET", "/api/meta");
        foreign
            .headers
            .insert("host".to_string(), "evil.example".to_string());
        let response = handle(&state, &foreign);
        assert_eq!(response.status, 403);
        assert_eq!(body(&response)["error"]["code"], json!("host_not_allowed"));

        let mut cross = request("POST", "/api/actions/validate");
        cross
            .headers
            .insert("origin".to_string(), "https://evil.example".to_string());
        cross
            .headers
            .insert("content-type".to_string(), "application/json".to_string());
        let response = handle(&state, &cross);
        assert_eq!(response.status, 403);
        assert_eq!(
            body(&response)["error"]["code"],
            json!("origin_not_allowed")
        );

        let mut no_type = request("POST", "/api/actions/validate");
        no_type
            .headers
            .insert("origin".to_string(), "http://127.0.0.1:7810".to_string());
        let response = handle(&state, &no_type);
        assert_eq!(response.status, 415);

        let mut unknown = request("POST", "/api/actions/definitely_not_an_action");
        unknown
            .headers
            .insert("origin".to_string(), "http://127.0.0.1:7810".to_string());
        unknown
            .headers
            .insert("content-type".to_string(), "application/json".to_string());
        let response = handle(&state, &unknown);
        assert_eq!(response.status, 404);
        assert_eq!(body(&response)["error"]["code"], json!("unknown_action"));

        let mut wrong_path = request("POST", "/api/meta");
        wrong_path
            .headers
            .insert("origin".to_string(), "http://127.0.0.1:7810".to_string());
        wrong_path
            .headers
            .insert("content-type".to_string(), "application/json".to_string());
        let response = handle(&state, &wrong_path);
        assert_eq!(response.status, 405);
    }

    #[test]
    fn workspace_reads_and_writes_stay_inside_the_workspace() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = state_in(&temp);
        let base = state.base();
        std::fs::create_dir_all(base.join("plays")).expect("plays");
        std::fs::write(base.join("plays").join("One.play"), "Play: One\n").expect("play");
        std::fs::write(temp.path().join("outside.play"), "Play: Outside\n").expect("outside");

        let inside = super::super::workspace_file(&state, "plays/One.play").expect("inside");
        assert!(inside.is_file());

        let outside =
            super::super::workspace_file(&state, "../outside.play").expect_err("relative escape");
        assert_eq!(outside.status, 403);
        assert_eq!(outside.code, "path_outside_workspace");

        let absolute = temp.path().join("outside.play");
        let absolute = super::super::workspace_file(&state, &absolute.to_string_lossy())
            .expect_err("absolute escape");
        assert_eq!(absolute.status, 403);

        let missing =
            super::super::workspace_file(&state, "plays/Missing.play").expect_err("missing");
        assert_eq!(missing.status, 404);

        assert!(super::super::workspace_target(&state, "plays/New.play").is_ok());
        let traversal =
            super::super::workspace_target(&state, "plays/../escape.play").expect_err("traversal");
        assert_eq!(traversal.status, 403);
        let extension =
            super::super::workspace_target(&state, "plays/New.txt").expect_err("extension");
        assert_eq!(extension.status, 400);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_target_rejects_a_symlink_that_escapes() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("tempdir");
        let state = state_in(&temp);
        let base = state.base();
        std::fs::create_dir_all(base.join("plays")).expect("plays");
        let outside = temp.path().join("outside.play");
        std::fs::write(&outside, "Play: Outside\n").expect("outside");
        symlink(&outside, base.join("plays").join("link.play")).expect("symlink");

        let error = super::super::workspace_target(&state, "plays/link.play").expect_err("escape");
        assert_eq!(error.status, 403);
        assert_eq!(error.code, "path_outside_workspace");
    }
}
