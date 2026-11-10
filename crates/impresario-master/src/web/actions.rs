//! Curated mutation surface of the web console.
//!
//! Every action maps to one CLI invocation. Action names and parameters are
//! fixed here; the HTTP layer never accepts a command string from a client.
//! Values that could be mistaken for options are rejected, so an action cannot
//! smuggle an extra flag into the child command.

use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

use serde_json::{json, Map, Value};

use super::{http, now_rfc3339, ApiError, CliOutput, WebState};

struct ParamDef {
    name: &'static str,
    kind: &'static str,
    required: bool,
    values: &'static [&'static str],
    doc: &'static str,
}

struct ActionDef {
    name: &'static str,
    description: &'static str,
    params: &'static [ParamDef],
    mutates: bool,
}

const fn param(
    name: &'static str,
    kind: &'static str,
    required: bool,
    values: &'static [&'static str],
    doc: &'static str,
) -> ParamDef {
    ParamDef {
        name,
        kind,
        required,
        values,
        doc,
    }
}

const SCOPE: ParamDef = param(
    "scope",
    "string",
    false,
    &["workspace", "global"],
    "Write to the workspace layer (default) or the global layer.",
);
const NAME: ParamDef = param("name", "string", true, &[], "Definition name.");
const PATH: ParamDef = param("path", "string", true, &[], "Filesystem path.");
const OPTIONAL_PATH: ParamDef = param("path", "string", false, &[], "Filesystem path.");

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        name: "validate",
        description: "Parse the workspace (or the given files) without touching a device.",
        params: &[param(
            "paths",
            "string[]",
            false,
            &[],
            "Files or directories; default is the configured workspace directories.",
        )],
        mutates: false,
    },
    ActionDef {
        name: "run",
        description: "Preview a run, or submit a detached run to the job supervisor.",
        params: &[
            param(
                "play",
                "string[]",
                false,
                &[],
                "Play file, directory or glob.",
            ),
            param("app", "string", false, &[], "App definition to bind."),
            param("device", "string[]", false, &[], "Device, @group or all."),
            param(
                "platform",
                "string[]",
                false,
                &[],
                "Only devices on these platforms.",
            ),
            param("tag", "string[]", false, &[], "Only acts with these tags."),
            param(
                "exclude_tag",
                "string[]",
                false,
                &[],
                "Exclude acts with these tags.",
            ),
            param(
                "act",
                "string[]",
                false,
                &[],
                "Only acts with these exact names.",
            ),
            param("profile", "string", false, &[], "Saved run profile."),
            param(
                "only_failed",
                "boolean",
                false,
                &[],
                "Only acts that failed in the newest report.",
            ),
            param("var", "string[]", false, &[], "Variable override K=V."),
            param(
                "arg",
                "string[]",
                false,
                &[],
                "Act parameter override ACT.PARAM=VALUE.",
            ),
            param(
                "timeout",
                "integer",
                false,
                &[],
                "Default poll timeout in ms.",
            ),
            param(
                "retries",
                "integer",
                false,
                &[],
                "Retry failed acts up to n times.",
            ),
            param(
                "unverifiable",
                "string",
                false,
                &["fail", "warn"],
                "Probe policy.",
            ),
            param(
                "reporter",
                "string",
                false,
                &[],
                "Comma list: console, junit, json, html.",
            ),
            param(
                "fail_fast",
                "boolean",
                false,
                &[],
                "Stop at the first failure.",
            ),
            param(
                "dry_run",
                "boolean",
                false,
                &[],
                "Print the resolved plan and exit.",
            ),
            param(
                "on_busy",
                "string",
                false,
                &["wait", "fail", "queue"],
                "Device-lease policy.",
            ),
        ],
        mutates: true,
    },
    ActionDef {
        name: "probe",
        description: "Run one live probe (showing, player or drm) on exactly one device.",
        params: &[
            param(
                "kind",
                "string",
                true,
                &["showing", "player", "drm"],
                "Probe family.",
            ),
            param(
                "device",
                "string",
                true,
                &[],
                "Device or @group with one device.",
            ),
            param(
                "app",
                "string",
                false,
                &[],
                "App definition to launch first.",
            ),
            param("piece", "string", false, &[], "Piece as Scene.Piece."),
            param(
                "selector",
                "string",
                false,
                &[],
                "Raw selector for the device platform.",
            ),
            param(
                "claims",
                "string[]",
                false,
                &[],
                "Claim ids; prefix with 'not ' to invert.",
            ),
            param(
                "confidence",
                "string",
                false,
                &["none", "inferred", "corroborated", "authoritative"],
                "Minimum confidence.",
            ),
        ],
        mutates: true,
    },
    ActionDef {
        name: "device_check",
        description: "Transport prepare + handshake + DeviceInfo for the named devices.",
        params: &[param(
            "names",
            "string[]",
            false,
            &[],
            "Device names; default is all.",
        )],
        mutates: true,
    },
    ActionDef {
        name: "job_stop",
        description: "Ask a running job to stop at the next command boundary.",
        params: &[param("id", "string", true, &[], "Job id.")],
        mutates: true,
    },
    ActionDef {
        name: "job_remove",
        description: "Remove a finished job's state and logs.",
        params: &[param("id", "string", true, &[], "Job id.")],
        mutates: true,
    },
    ActionDef {
        name: "workspace_add",
        description: "Remember a workspace in the global workspace list.",
        params: &[OPTIONAL_PATH],
        mutates: true,
    },
    ActionDef {
        name: "workspace_use",
        description: "Make a workspace the global default and switch this console to it.",
        params: &[PATH],
        mutates: true,
    },
    ActionDef {
        name: "workspace_new",
        description: "Create a workspace skeleton and switch this console to it.",
        params: &[
            OPTIONAL_PATH,
            param(
                "example",
                "boolean",
                false,
                &[],
                "Also write the example scenes and play.",
            ),
            param(
                "force",
                "boolean",
                false,
                &[],
                "Overwrite files that already exist.",
            ),
        ],
        mutates: true,
    },
    ActionDef {
        name: "workspace_remove",
        description: "Forget a workspace path (the active workspace is refused).",
        params: &[PATH],
        mutates: true,
    },
    ActionDef {
        name: "group_add",
        description: "Add an actor group to one layer.",
        params: &[
            NAME,
            param(
                "members",
                "string[]",
                true,
                &[],
                "Device names or @group references.",
            ),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "group_set",
        description: "Create or replace an actor group in one layer.",
        params: &[
            NAME,
            param(
                "members",
                "string[]",
                true,
                &[],
                "Device names or @group references.",
            ),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "group_remove",
        description: "Remove an actor group from one layer.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "device_add",
        description: "Add a device definition.",
        params: &[
            NAME,
            param(
                "type",
                "string",
                true,
                &[
                    "web",
                    "webOS",
                    "tizenOS",
                    "android",
                    "androidtv",
                    "iOS",
                    "tvOS",
                ],
                "Exact device type.",
            ),
            param(
                "ip",
                "string",
                true,
                &[],
                "Device IPv4/IPv6 address; 127.0.0.1 for local web.",
            ),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "device_edit",
        description: "Replace an existing device definition.",
        params: &[
            NAME,
            param(
                "type",
                "string",
                true,
                &[
                    "web",
                    "webOS",
                    "tizenOS",
                    "android",
                    "androidtv",
                    "iOS",
                    "tvOS",
                ],
                "Exact device type.",
            ),
            param("ip", "string", true, &[], "Device IPv4/IPv6 address."),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "device_remove",
        description: "Remove a device from one layer.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "device_rename",
        description: "Rename a device in one layer.",
        params: &[
            NAME,
            param("new_name", "string", true, &[], "New device name."),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "app_add",
        description: "Add an app definition.",
        params: &[
            NAME,
            param("description", "string", false, &[], "Optional description."),
            param(
                "platforms",
                "string[]",
                false,
                &[],
                "PLATFORM=ID or web=URL entries.",
            ),
            param(
                "packages",
                "string[]",
                false,
                &[],
                "PLATFORM=PATH artifacts.",
            ),
            param(
                "clear_data",
                "boolean",
                false,
                &[],
                "Wipe app data before a reset.",
            ),
            param(
                "launch_activity",
                "string[]",
                false,
                &[],
                "PLATFORM=ACTIVITY entries.",
            ),
            param(
                "launch_params",
                "string[]",
                false,
                &[],
                "KEY=VALUE launch parameters.",
            ),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "app_edit",
        description: "Replace an existing app definition.",
        params: &[
            NAME,
            param("description", "string", false, &[], "Optional description."),
            param(
                "platforms",
                "string[]",
                false,
                &[],
                "PLATFORM=ID or web=URL entries.",
            ),
            param(
                "packages",
                "string[]",
                false,
                &[],
                "PLATFORM=PATH artifacts.",
            ),
            param(
                "clear_data",
                "boolean",
                false,
                &[],
                "Wipe app data before a reset.",
            ),
            param(
                "launch_activity",
                "string[]",
                false,
                &[],
                "PLATFORM=ACTIVITY entries.",
            ),
            param(
                "launch_params",
                "string[]",
                false,
                &[],
                "KEY=VALUE launch parameters.",
            ),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "app_remove",
        description: "Remove an app from one layer.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "app_rename",
        description: "Rename an app in one layer.",
        params: &[
            NAME,
            param("new_name", "string", true, &[], "New app name."),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "config_set",
        description: "Set and validate one setting in the workspace or global layer.",
        params: &[
            param("key", "string", true, &[], "Dotted setting key."),
            param(
                "value",
                "string",
                true,
                &[],
                "New value; lists are comma-separated.",
            ),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "config_unset",
        description: "Remove one setting from the workspace or global layer.",
        params: &[
            param("key", "string", true, &[], "Dotted setting key."),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "test_save",
        description: "Save a .scene/.theatre/.play file and roll it back when validation fails.",
        params: &[
            param("path", "string", true, &[], "Workspace-relative file path."),
            param("content", "string", true, &[], "Full UTF-8 file contents."),
        ],
        mutates: true,
    },
    ActionDef {
        name: "test_new",
        description: "Create a new scene, theatre or play file.",
        params: &[
            param(
                "kind",
                "string",
                true,
                &["scene", "theatre", "play"],
                "File kind.",
            ),
            param(
                "name",
                "string",
                true,
                &[],
                "UpperCamelCase declaration name.",
            ),
            param(
                "force",
                "boolean",
                false,
                &[],
                "Overwrite an existing file.",
            ),
        ],
        mutates: true,
    },
    ActionDef {
        name: "test_rename",
        description: "Rename a declaration and every identifier-boundary reference.",
        params: &[
            param(
                "kind",
                "string",
                true,
                &["scene", "theatre", "play", "stage", "act", "piece", "any"],
                "Declaration kind.",
            ),
            param("old", "string", true, &[], "Current name."),
            param("new", "string", true, &[], "New name."),
            param(
                "dry_run",
                "boolean",
                false,
                &[],
                "Show changes without writing.",
            ),
        ],
        mutates: true,
    },
    ActionDef {
        name: "service_start",
        description: "Start the detached resident runner for this workspace.",
        params: &[],
        mutates: true,
    },
    ActionDef {
        name: "service_stop",
        description: "Ask the resident runner to stop.",
        params: &[],
        mutates: true,
    },
    ActionDef {
        name: "service_restart",
        description: "Stop and start the resident runner.",
        params: &[],
        mutates: true,
    },
    ActionDef {
        name: "service_pause",
        description: "Pause schedule and watcher execution.",
        params: &[],
        mutates: true,
    },
    ActionDef {
        name: "service_resume",
        description: "Resume schedule and watcher execution.",
        params: &[],
        mutates: true,
    },
    ActionDef {
        name: "service_install",
        description: "Install the per-user logon autostart hook.",
        params: &[],
        mutates: true,
    },
    ActionDef {
        name: "service_uninstall",
        description: "Remove the logon hook (history and config stay unless purge).",
        params: &[param(
            "purge",
            "boolean",
            false,
            &[],
            "Also purge resident history and config.",
        )],
        mutates: true,
    },
    ActionDef {
        name: "schedule_add",
        description: "Add a resident schedule.",
        params: &[
            NAME,
            param(
                "when",
                "string",
                true,
                &[],
                "Short when mapping, e.g. kind: daily, at: \"02:30\".",
            ),
            param("play", "string[]", false, &[], "Only these plays."),
            param("tag", "string[]", false, &[], "Only acts with these tags."),
            param(
                "exclude_tag",
                "string[]",
                false,
                &[],
                "Exclude acts with these tags.",
            ),
            param("actor", "string[]", false, &[], "Devices or @groups."),
            param("app", "string", false, &[], "App definition."),
            param("retries", "integer", false, &[], "Retry failed acts."),
            param("profile", "string", false, &[], "Saved run profile."),
            param(
                "on_missed",
                "string",
                false,
                &["skip", "run-once"],
                "Missed-fire policy.",
            ),
            param(
                "on_busy",
                "string",
                false,
                &["queue", "skip", "fail"],
                "Busy-device policy.",
            ),
            param("disabled", "boolean", false, &[], "Create it disabled."),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "schedule_remove",
        description: "Remove a resident schedule.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "schedule_enable",
        description: "Enable a schedule in its layer.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "schedule_disable",
        description: "Disable a schedule in its layer.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "schedule_run_now",
        description: "Run one schedule immediately.",
        params: &[NAME],
        mutates: true,
    },
    ActionDef {
        name: "watcher_add",
        description: "Add an always-on watcher.",
        params: &[
            NAME,
            param("play", "string[]", false, &[], "Only these plays."),
            param("tag", "string[]", false, &[], "Only acts with these tags."),
            param(
                "exclude_tag",
                "string[]",
                false,
                &[],
                "Exclude acts with these tags.",
            ),
            param("actor", "string[]", false, &[], "Devices or @groups."),
            param("app", "string", false, &[], "App definition."),
            param("retries", "integer", false, &[], "Retry failed acts."),
            param("profile", "string", false, &[], "Saved run profile."),
            param(
                "gap",
                "string",
                false,
                &[],
                "Minimum gap between iterations, e.g. 30s.",
            ),
            param(
                "lease",
                "string",
                false,
                &["gap", "no-gap"],
                "Device lease mode.",
            ),
            param("disabled", "boolean", false, &[], "Create it disabled."),
            SCOPE,
        ],
        mutates: true,
    },
    ActionDef {
        name: "watcher_remove",
        description: "Remove a watcher.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "watcher_enable",
        description: "Enable a watcher in its layer.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "watcher_disable",
        description: "Disable a watcher in its layer.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "watcher_start",
        description: "Start a watcher in its layer.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "watcher_stop",
        description: "Stop a watcher in its layer.",
        params: &[NAME, SCOPE],
        mutates: true,
    },
    ActionDef {
        name: "watcher_pause",
        description: "Pause a watcher.",
        params: &[NAME],
        mutates: true,
    },
    ActionDef {
        name: "watcher_resume",
        description: "Resume a watcher.",
        params: &[NAME],
        mutates: true,
    },
    ActionDef {
        name: "watcher_run_now",
        description: "Run a watcher iteration immediately.",
        params: &[NAME],
        mutates: true,
    },
];

/// The machine-readable action schema.
#[must_use]
pub fn schema() -> Value {
    Value::Array(
        ACTIONS
            .iter()
            .map(|action| {
                let mut params = Map::new();
                for param in action.params {
                    params.insert(
                        param.name.to_string(),
                        json!({
                            "type": param.kind,
                            "required": param.required,
                            "values": param.values,
                            "description": param.doc,
                        }),
                    );
                }
                json!({
                    "name": action.name,
                    "method": "POST",
                    "path": format!("/api/actions/{}", action.name),
                    "description": action.description,
                    "mutates": action.mutates,
                    "params": Value::Object(params),
                })
            })
            .collect(),
    )
}

/// Action names, for `/api/meta`.
#[must_use]
pub fn names() -> Vec<&'static str> {
    ACTIONS.iter().map(|action| action.name).collect()
}

fn find(name: &str) -> Option<&'static ActionDef> {
    ACTIONS.iter().find(|action| action.name == name)
}

/// Execute one action and wrap it in the API envelope.
pub fn handle(state: &WebState, name: &str, body: &[u8]) -> http::Response {
    let Some(_definition) = find(name) else {
        return ApiError::new(404, "unknown_action", format!("unknown action '{name}'")).response();
    };
    let request = if body.is_empty() {
        Map::new()
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(Value::Object(map)) => map,
            Ok(Value::Null) => Map::new(),
            Ok(_) => {
                return ApiError::bad("the request body must be a JSON object").response();
            }
            Err(error) => {
                return ApiError::bad(format!("the request body is not valid JSON: {error}"))
                    .response();
            }
        }
    };
    let guard = match state.mutation.lock() {
        Ok(guard) => guard,
        Err(poison) => poison.into_inner(),
    };
    let result = execute(state, name, &request, &guard);
    let (passed, exit_code, message) = match &result {
        Ok(executed) => (
            executed.output.code == 0,
            Some(executed.output.code),
            executed.output.message(),
        ),
        Err(error) => (false, None, error.message.clone()),
    };
    state.record_action(super::ActionRecord {
        at: now_rfc3339(),
        action: name.to_string(),
        passed,
        exit_code,
        message: truncate(&message, 500),
    });
    drop(guard);
    match result {
        Ok(executed) => {
            let mut data = Map::new();
            data.insert("action".to_string(), json!(name));
            data.insert("exit_code".to_string(), json!(executed.output.code));
            data.insert("passed".to_string(), json!(executed.output.code == 0));
            data.insert("stdout".to_string(), json!(executed.output.stdout));
            data.insert("stderr".to_string(), json!(executed.output.stderr));
            if let Value::Object(extra) = executed.extra {
                for (key, value) in extra {
                    data.insert(key, value);
                }
            }
            super::ok(Value::Object(data))
        }
        Err(error) => error.response(),
    }
}

struct Executed {
    output: CliOutput,
    extra: Value,
}

impl Executed {
    fn plain(output: CliOutput) -> Self {
        Self {
            output,
            extra: json!({}),
        }
    }
}

fn execute(
    state: &WebState,
    name: &str,
    params: &Map<String, Value>,
    _guard: &MutexGuard<'_, ()>,
) -> Result<Executed, ApiError> {
    let base = state.base();
    match name {
        "validate" => {
            let mut args = vec!["validate".to_string()];
            args.extend(strings(params, "paths")?);
            run(state, args)
        }
        "run" => {
            let mut args = vec!["run".to_string()];
            for play in strings(params, "play")? {
                args.push("--play".to_string());
                args.push(play);
            }
            for device in strings(params, "device")? {
                args.push("--device".to_string());
                args.push(device);
            }
            for platform in strings(params, "platform")? {
                args.push("--platform".to_string());
                args.push(platform);
            }
            for tag in strings(params, "tag")? {
                args.push("--tag".to_string());
                args.push(tag);
            }
            for tag in strings(params, "exclude_tag")? {
                args.push("--exclude-tag".to_string());
                args.push(tag);
            }
            for act in strings(params, "act")? {
                args.push("--act".to_string());
                args.push(act);
            }
            push_option(&mut args, "--app", string(params, "app")?);
            push_option(&mut args, "--profile", string(params, "profile")?);
            if bool_value(params, "only_failed")?.unwrap_or(false) {
                args.push("--only-failed".to_string());
            }
            for var in strings(params, "var")? {
                require_pair("var", &var)?;
                args.push("--var".to_string());
                args.push(var);
            }
            for value in strings(params, "arg")? {
                require_pair("arg", &value)?;
                args.push("--arg".to_string());
                args.push(value);
            }
            if let Some(timeout) = integer(params, "timeout", 1, 3_600_000)? {
                args.push("--timeout".to_string());
                args.push(timeout.to_string());
            }
            if let Some(retries) = integer(params, "retries", 0, 100)? {
                args.push("--retries".to_string());
                args.push(retries.to_string());
            }
            push_option(
                &mut args,
                "--unverifiable",
                enumeration(params, "unverifiable", &["fail", "warn"])?,
            );
            push_option(&mut args, "--reporter", string(params, "reporter")?);
            if bool_value(params, "fail_fast")?.unwrap_or(false) {
                args.push("--fail-fast".to_string());
            }
            push_option(
                &mut args,
                "--on-busy",
                enumeration(params, "on_busy", &["wait", "fail", "queue"])?,
            );
            let dry_run = bool_value(params, "dry_run")?.unwrap_or(false);
            args.push(if dry_run { "--dry-run" } else { "--detach" }.to_string());
            let output = run(state, args)?;
            let job_id = parse_job_id(&output.output.stdout);
            let mut extra = Map::new();
            if let Some(id) = job_id {
                extra.insert("job_id".to_string(), json!(id));
            }
            Ok(Executed {
                output: output.output,
                extra: Value::Object(extra),
            })
        }
        "probe" => {
            let kind = enumeration(params, "kind", &["showing", "player", "drm"])?
                .ok_or_else(|| ApiError::bad("'kind' is required"))?;
            let device = required_string(params, "device")?;
            let piece = string(params, "piece")?;
            let selector = string(params, "selector")?;
            if piece.is_some() && selector.is_some() {
                return Err(ApiError::bad("'piece' and 'selector' cannot be combined"));
            }
            let mut args = vec![
                "probe".to_string(),
                kind,
                "--device".to_string(),
                device,
                "--json".to_string(),
            ];
            push_option(&mut args, "--app", string(params, "app")?);
            push_option(&mut args, "--piece", piece);
            push_option(&mut args, "--selector", selector);
            for claim in strings(params, "claims")? {
                args.push("--claim".to_string());
                args.push(claim);
            }
            push_option(
                &mut args,
                "--confidence",
                enumeration(
                    params,
                    "confidence",
                    &["none", "inferred", "corroborated", "authoritative"],
                )?,
            );
            let output = run(state, args)?;
            let mut extra = Map::new();
            if let Ok(value) = output.output.json() {
                extra.insert("probe".to_string(), value);
            }
            Ok(Executed {
                output: output.output,
                extra: Value::Object(extra),
            })
        }
        "device_check" => {
            let mut args = vec!["device".to_string(), "check".to_string()];
            args.extend(strings(params, "names")?);
            run(state, args)
        }
        "job_stop" => run(state, vec!["stop".to_string(), job_id(params)?]),
        "job_remove" => run(state, vec!["rm".to_string(), job_id(params)?]),
        "workspace_add" => {
            let path = optional_path(state, params)?;
            let mut args = vec!["workspace".to_string(), "add".to_string()];
            if let Some(path) = path {
                args.push(path.display().to_string());
            }
            run(state, args)
        }
        "workspace_use" => {
            let path = required_path(state, params)?;
            let output = run(
                state,
                vec![
                    "workspace".to_string(),
                    "use".to_string(),
                    path.display().to_string(),
                ],
            )?;
            if output.output.code == 0 {
                state.set_base(path.canonicalize().unwrap_or(path));
            }
            Ok(Executed {
                output: output.output,
                extra: json!({ "workspace": state.base() }),
            })
        }
        "workspace_new" => {
            let path = optional_path(state, params)?;
            let mut args = vec!["workspace".to_string(), "new".to_string()];
            if let Some(path) = &path {
                args.push(path.display().to_string());
            }
            if bool_value(params, "example")?.unwrap_or(false) {
                args.push("--example".to_string());
            }
            if bool_value(params, "force")?.unwrap_or(false) {
                args.push("--force".to_string());
            }
            let output = run(state, args)?;
            let switched = path.and_then(|path| {
                if output.output.code == 0 {
                    Some(path.canonicalize().unwrap_or(path))
                } else {
                    None
                }
            });
            if let Some(path) = &switched {
                state.set_base(path.clone());
            }
            Ok(Executed {
                output: output.output,
                extra: json!({ "workspace": state.base() }),
            })
        }
        "workspace_remove" => {
            let path = required_path(state, params)?;
            let same_directory = matches!(
                (path.canonicalize(), base.canonicalize()),
                (Ok(target), Ok(active)) if target == active
            );
            if path == base || same_directory {
                return Err(ApiError::new(
                    409,
                    "active_workspace",
                    "the active workspace cannot be removed",
                ));
            }
            run(
                state,
                vec![
                    "workspace".to_string(),
                    "remove".to_string(),
                    path.display().to_string(),
                ],
            )
        }
        "group_add" | "group_set" => {
            let verb = if name == "group_add" { "add" } else { "set" };
            let mut args = vec![
                "group".to_string(),
                verb.to_string(),
                required_string(params, "name")?,
            ];
            args.extend(strings(params, "members")?);
            args.extend(scope(params)?);
            run(state, args)
        }
        "group_remove" => {
            let mut args = vec![
                "group".to_string(),
                "remove".to_string(),
                required_string(params, "name")?,
            ];
            args.extend(scope(params)?);
            run(state, args)
        }
        "device_add" | "device_edit" => {
            let verb = if name == "device_add" { "add" } else { "edit" };
            let mut args = vec![
                "device".to_string(),
                verb.to_string(),
                "--name".to_string(),
                required_string(params, "name")?,
                "--type".to_string(),
                enumeration(
                    params,
                    "type",
                    &[
                        "web",
                        "webOS",
                        "tizenOS",
                        "android",
                        "androidtv",
                        "iOS",
                        "tvOS",
                    ],
                )?
                .ok_or_else(|| ApiError::bad("'type' is required"))?,
                "--ip".to_string(),
                required_string(params, "ip")?,
            ];
            args.extend(scope(params)?);
            run(state, args)
        }
        "device_remove" => {
            let mut args = vec![
                "device".to_string(),
                "remove".to_string(),
                required_string(params, "name")?,
            ];
            args.extend(scope(params)?);
            run(state, args)
        }
        "device_rename" => {
            let mut args = vec![
                "device".to_string(),
                "rename".to_string(),
                required_string(params, "name")?,
                required_string(params, "new_name")?,
            ];
            args.extend(scope(params)?);
            run(state, args)
        }
        "app_add" | "app_edit" => {
            let verb = if name == "app_add" { "add" } else { "edit" };
            let mut args = vec![
                "app".to_string(),
                verb.to_string(),
                "--name".to_string(),
                required_string(params, "name")?,
            ];
            push_option(&mut args, "--description", string(params, "description")?);
            for platform in strings(params, "platforms")? {
                require_pair("platforms", &platform)?;
                args.push("--platform".to_string());
                args.push(platform);
            }
            for package in strings(params, "packages")? {
                require_pair("packages", &package)?;
                args.push("--package".to_string());
                args.push(package);
            }
            if bool_value(params, "clear_data")?.unwrap_or(false) {
                args.push("--clear-data".to_string());
            }
            for activity in strings(params, "launch_activity")? {
                require_pair("launch_activity", &activity)?;
                args.push("--launch-activity".to_string());
                args.push(activity);
            }
            for launch in strings(params, "launch_params")? {
                require_pair("launch_params", &launch)?;
                args.push("--launch-param".to_string());
                args.push(launch);
            }
            args.extend(scope(params)?);
            run(state, args)
        }
        "app_remove" => {
            let mut args = vec![
                "app".to_string(),
                "remove".to_string(),
                required_string(params, "name")?,
            ];
            args.extend(scope(params)?);
            run(state, args)
        }
        "app_rename" => {
            let mut args = vec![
                "app".to_string(),
                "rename".to_string(),
                required_string(params, "name")?,
                required_string(params, "new_name")?,
            ];
            args.extend(scope(params)?);
            run(state, args)
        }
        "config_set" => {
            let mut args = vec![
                "config".to_string(),
                "set".to_string(),
                required_string(params, "key")?,
                required_string(params, "value")?,
            ];
            args.extend(config_scope(params)?);
            run(state, args)
        }
        "config_unset" => {
            let mut args = vec![
                "config".to_string(),
                "unset".to_string(),
                required_string(params, "key")?,
            ];
            args.extend(config_scope(params)?);
            run(state, args)
        }
        "test_save" => save_test(state, params),
        "test_new" => {
            let mut args = vec![
                "tests".to_string(),
                "new".to_string(),
                enumeration(params, "kind", &["scene", "theatre", "play"])?
                    .ok_or_else(|| ApiError::bad("'kind' is required"))?,
                required_string(params, "name")?,
            ];
            if bool_value(params, "force")?.unwrap_or(false) {
                args.push("--force".to_string());
            }
            run(state, args)
        }
        "test_rename" => {
            let mut args = vec![
                "tests".to_string(),
                "rename".to_string(),
                enumeration(
                    params,
                    "kind",
                    &["scene", "theatre", "play", "stage", "act", "piece", "any"],
                )?
                .ok_or_else(|| ApiError::bad("'kind' is required"))?,
                required_string(params, "old")?,
                required_string(params, "new")?,
            ];
            if bool_value(params, "dry_run")?.unwrap_or(false) {
                args.push("--dry-run".to_string());
            }
            run(state, args)
        }
        "service_start" | "service_stop" | "service_restart" | "service_pause"
        | "service_resume" | "service_install" => {
            let verb = name.trim_start_matches("service_");
            run(state, vec!["service".to_string(), verb.to_string()])
        }
        "service_uninstall" => {
            let mut args = vec!["service".to_string(), "uninstall".to_string()];
            if bool_value(params, "purge")?.unwrap_or(false) {
                args.push("--purge".to_string());
            }
            run(state, args)
        }
        "schedule_add" => {
            let mut args = vec![
                "schedule".to_string(),
                "add".to_string(),
                required_string(params, "name")?,
                "--when".to_string(),
                required_string(params, "when")?,
            ];
            push_run_filters(&mut args, params)?;
            if let Some(retries) = integer(params, "retries", 0, 100)? {
                args.push("--retries".to_string());
                args.push(retries.to_string());
            }
            push_option(&mut args, "--profile", string(params, "profile")?);
            push_option(
                &mut args,
                "--on-missed",
                enumeration(params, "on_missed", &["skip", "run-once"])?,
            );
            push_option(
                &mut args,
                "--on-busy",
                enumeration(params, "on_busy", &["queue", "skip", "fail"])?,
            );
            if bool_value(params, "disabled")?.unwrap_or(false) {
                args.push("--disabled".to_string());
            }
            args.extend(layer_scope(params)?);
            run(state, args)
        }
        "schedule_remove" | "schedule_enable" | "schedule_disable" => {
            let verb = match name {
                "schedule_remove" => "rm",
                "schedule_enable" => "enable",
                _ => "disable",
            };
            let mut args = vec![
                "schedule".to_string(),
                verb.to_string(),
                required_string(params, "name")?,
            ];
            args.extend(layer_scope(params)?);
            run(state, args)
        }
        "schedule_run_now" => run(
            state,
            vec![
                "schedule".to_string(),
                "run-now".to_string(),
                required_string(params, "name")?,
            ],
        ),
        "watcher_add" => {
            let mut args = vec![
                "watch".to_string(),
                "add".to_string(),
                required_string(params, "name")?,
            ];
            push_run_filters(&mut args, params)?;
            if let Some(retries) = integer(params, "retries", 0, 100)? {
                args.push("--retries".to_string());
                args.push(retries.to_string());
            }
            push_option(&mut args, "--profile", string(params, "profile")?);
            push_option(&mut args, "--gap", string(params, "gap")?);
            push_option(
                &mut args,
                "--lease",
                enumeration(params, "lease", &["gap", "no-gap"])?,
            );
            if bool_value(params, "disabled")?.unwrap_or(false) {
                args.push("--disabled".to_string());
            }
            args.extend(layer_scope(params)?);
            run(state, args)
        }
        "watcher_remove" | "watcher_enable" | "watcher_disable" | "watcher_start"
        | "watcher_stop" => {
            let verb = match name {
                "watcher_remove" => "rm",
                "watcher_enable" => "enable",
                "watcher_disable" => "disable",
                "watcher_start" => "start",
                _ => "stop",
            };
            let mut args = vec![
                "watch".to_string(),
                verb.to_string(),
                required_string(params, "name")?,
            ];
            args.extend(layer_scope(params)?);
            run(state, args)
        }
        "watcher_pause" | "watcher_resume" | "watcher_run_now" => {
            let verb = match name {
                "watcher_pause" => "pause",
                "watcher_resume" => "resume",
                _ => "run-now",
            };
            run(
                state,
                vec![
                    "watch".to_string(),
                    verb.to_string(),
                    required_string(params, "name")?,
                ],
            )
        }
        other => Err(ApiError::new(
            404,
            "unknown_action",
            format!("unknown action '{other}'"),
        )),
    }
}

fn save_test(state: &WebState, params: &Map<String, Value>) -> Result<Executed, ApiError> {
    const CONTENT_CAP: usize = 1024 * 1024;
    let raw_path = required_string(params, "path")?;
    let content = match params.get("content") {
        Some(Value::String(content)) => content.clone(),
        Some(_) => return Err(ApiError::bad("'content' must be a string")),
        None => return Err(ApiError::bad("'content' is required")),
    };
    if content.len() > CONTENT_CAP {
        return Err(ApiError::new(
            413,
            "too_large",
            format!("the file is larger than {CONTENT_CAP} bytes"),
        ));
    }
    let target = super::workspace_target(state, &raw_path)?;
    let previous = std::fs::read_to_string(&target).ok();
    std::fs::write(&target, &content).map_err(|error| {
        ApiError::new(
            500,
            "write_failed",
            format!("cannot write {}: {error}", target.display()),
        )
    })?;
    let output = run(
        state,
        vec!["validate".to_string(), target.display().to_string()],
    )?;
    if output.output.code != 0 {
        if let Some(previous) = previous {
            let _ = std::fs::write(&target, previous);
        } else {
            let _ = std::fs::remove_file(&target);
        }
    }
    let relative = target
        .strip_prefix(state.base())
        .unwrap_or(&target)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(Executed {
        output: output.output,
        extra: json!({
            "path": relative,
            "rolled_back": output.output.code != 0,
        }),
    })
}

fn push_run_filters(args: &mut Vec<String>, params: &Map<String, Value>) -> Result<(), ApiError> {
    for play in strings(params, "play")? {
        args.push("--play".to_string());
        args.push(play);
    }
    for tag in strings(params, "tag")? {
        args.push("--tag".to_string());
        args.push(tag);
    }
    for tag in strings(params, "exclude_tag")? {
        args.push("--exclude-tag".to_string());
        args.push(tag);
    }
    for actor in strings(params, "actor")? {
        args.push("--actor".to_string());
        args.push(actor);
    }
    push_option(args, "--app", string(params, "app")?);
    Ok(())
}

fn run(state: &WebState, args: Vec<String>) -> Result<Executed, ApiError> {
    super::run_cli(state, &args)
        .map(Executed::plain)
        .map_err(|message| ApiError::new(500, "spawn_failed", message))
}

fn string(params: &Map<String, Value>, key: &str) -> Result<Option<String>, ApiError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim().to_string();
            if value.is_empty() {
                return Ok(None);
            }
            if value.starts_with('-') {
                return Err(ApiError::bad(format!("'{key}' must not start with '-'")));
            }
            if value.len() > 4000 {
                return Err(ApiError::bad(format!("'{key}' is too long")));
            }
            Ok(Some(value))
        }
        Some(_) => Err(ApiError::bad(format!("'{key}' must be a string"))),
    }
}

fn required_string(params: &Map<String, Value>, key: &str) -> Result<String, ApiError> {
    string(params, key)?.ok_or_else(|| ApiError::bad(format!("'{key}' is required")))
}

fn strings(params: &Map<String, Value>, key: &str) -> Result<Vec<String>, ApiError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => {
            if values.len() > 200 {
                return Err(ApiError::bad(format!("'{key}' has more than 200 entries")));
            }
            let mut result = Vec::new();
            for value in values {
                let Some(value) = value.as_str() else {
                    return Err(ApiError::bad(format!("'{key}' must contain only strings")));
                };
                let value = value.trim();
                if value.is_empty() {
                    continue;
                }
                if value.starts_with('-') {
                    return Err(ApiError::bad(format!(
                        "'{key}' must not contain values starting with '-'"
                    )));
                }
                if value.len() > 4000 {
                    return Err(ApiError::bad(format!(
                        "'{key}' contains a value that is too long"
                    )));
                }
                result.push(value.to_string());
            }
            Ok(result)
        }
        Some(_) => Err(ApiError::bad(format!(
            "'{key}' must be an array of strings"
        ))),
    }
}

fn bool_value(params: &Map<String, Value>, key: &str) -> Result<Option<bool>, ApiError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(ApiError::bad(format!("'{key}' must be a boolean"))),
    }
}

fn integer(
    params: &Map<String, Value>,
    key: &str,
    min: u64,
    max: u64,
) -> Result<Option<u64>, ApiError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => {
            let Some(value) = value.as_u64() else {
                return Err(ApiError::bad(format!(
                    "'{key}' must be a non-negative integer"
                )));
            };
            if value < min || value > max {
                return Err(ApiError::bad(format!(
                    "'{key}' must be between {min} and {max}"
                )));
            }
            Ok(Some(value))
        }
        Some(_) => Err(ApiError::bad(format!("'{key}' must be an integer"))),
    }
}

fn enumeration(
    params: &Map<String, Value>,
    key: &str,
    allowed: &[&str],
) -> Result<Option<String>, ApiError> {
    let Some(value) = string(params, key)? else {
        return Ok(None);
    };
    if allowed.contains(&value.as_str()) {
        Ok(Some(value))
    } else {
        Err(ApiError::bad(format!(
            "'{key}' must be one of: {}",
            allowed.join(", ")
        )))
    }
}

fn require_pair(key: &str, value: &str) -> Result<(), ApiError> {
    if value.contains('=') {
        Ok(())
    } else {
        Err(ApiError::bad(format!("'{key}' entries must be KEY=VALUE")))
    }
}

fn job_id(params: &Map<String, Value>) -> Result<String, ApiError> {
    let id = required_string(params, "id")?;
    if id.len() > 128
        || id.starts_with('.')
        || id.contains(['/', '\\', ':'])
        || !id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'))
    {
        return Err(ApiError::bad("'id' is not a valid job id"));
    }
    Ok(id)
}

fn scope(params: &Map<String, Value>) -> Result<Vec<String>, ApiError> {
    match enumeration(params, "scope", &["workspace", "global"])?.as_deref() {
        None | Some("workspace") => Ok(vec!["--workspace".to_string()]),
        Some("global") => Ok(Vec::new()),
        Some(_) => Err(ApiError::bad("'scope' must be workspace or global")),
    }
}

/// Schedules and watchers default to the workspace layer and take only
/// `--global`; unlike device/group/app definitions they have no `--workspace`
/// flag.
fn layer_scope(params: &Map<String, Value>) -> Result<Vec<String>, ApiError> {
    match enumeration(params, "scope", &["workspace", "global"])?.as_deref() {
        None | Some("workspace") => Ok(Vec::new()),
        Some("global") => Ok(vec!["--global".to_string()]),
        Some(_) => Err(ApiError::bad("'scope' must be workspace or global")),
    }
}

fn config_scope(params: &Map<String, Value>) -> Result<Vec<String>, ApiError> {
    match enumeration(params, "scope", &["workspace", "global"])?.as_deref() {
        None | Some("workspace") => Ok(vec!["--workspace".to_string()]),
        Some("global") => Ok(vec!["--global".to_string()]),
        Some(_) => Err(ApiError::bad("'scope' must be workspace or global")),
    }
}

fn resolve_path(state: &WebState, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        state.base().join(path)
    }
}

fn optional_path(
    state: &WebState,
    params: &Map<String, Value>,
) -> Result<Option<PathBuf>, ApiError> {
    Ok(string(params, "path")?.map(|raw| resolve_path(state, &raw)))
}

fn required_path(state: &WebState, params: &Map<String, Value>) -> Result<PathBuf, ApiError> {
    optional_path(state, params)?.ok_or_else(|| ApiError::bad("'path' is required"))
}

fn push_option(args: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(value) = value {
        args.push(flag.to_string());
        args.push(value);
    }
}

fn parse_job_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("job ") {
            if let Some(id) = rest.strip_suffix(" submitted") {
                return Some(id.trim().to_string());
            }
        }
        if let Some(rest) = line.strip_prefix("submitted job ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut result: String = text.chars().take(limit).collect();
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_lists_every_action_and_rejects_duplicates() {
        let names = names();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len());
        let schema = schema();
        let actions = schema.as_array().expect("schema array");
        assert_eq!(actions.len(), names.len());
        assert!(names.contains(&"run"));
        assert!(names.contains(&"probe"));
        assert!(names.contains(&"test_save"));
    }

    #[test]
    fn parameter_helpers_reject_option_smuggling() {
        let mut params = Map::new();
        params.insert("device".to_string(), json!("-evil"));
        assert!(string(&params, "device").is_err());
        params.insert("device".to_string(), json!(["-evil"]));
        assert!(strings(&params, "device").is_err());
        params.insert("device".to_string(), json!([1, 2]));
        assert!(strings(&params, "device").is_err());
        params.insert("retries".to_string(), json!(1000));
        assert!(integer(&params, "retries", 0, 100).is_err());
        params.insert("kind".to_string(), json!("other"));
        assert!(enumeration(&params, "kind", &["showing"]).is_err());
    }

    #[test]
    fn job_ids_are_confined_to_one_path_segment() {
        let mut params = Map::new();
        params.insert("id".to_string(), json!("20261110-120000-abcd"));
        assert!(job_id(&params).is_ok());
        params.insert("id".to_string(), json!("../escape"));
        assert!(job_id(&params).is_err());
        params.insert("id".to_string(), json!(".hidden"));
        assert!(job_id(&params).is_err());
    }

    #[test]
    fn parses_detached_job_ids() {
        assert_eq!(
            parse_job_id("job 20261110-120000-abcd submitted\n").as_deref(),
            Some("20261110-120000-abcd")
        );
        assert_eq!(parse_job_id("nothing").as_deref(), None);
    }

    #[test]
    fn action_names_are_allowlisted_path_segments() {
        for name in names() {
            assert!(find(name).is_some());
            assert!(!name.is_empty());
            assert!(name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
        }
        assert!(find("run --detach").is_none());
        assert!(find("../escape").is_none());
        assert!(find("").is_none());
    }
}
