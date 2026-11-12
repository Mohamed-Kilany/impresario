//! Tool definitions (`tools/list`) and the `tools/call` entry point.
//!
//! Every tool result carries a text block (pretty JSON, or a human summary
//! plus the JSON); tools that return a table also carry
//! `structuredContent`, and artifact reads may add an `image` block so an
//! agent can see failure screenshots.

use serde_json::{json, Map, Value};

use crate::mcp::handlers;
use crate::mcp::inspector as inspector_tools;
use crate::mcp::protocol::{RpcError, RpcResult, Server, INVALID_PARAMS};

/// The MCP tool list, in a fixed order (clients page `tools/list`, so the
/// order must not vary between calls).
pub(crate) fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "impresario_workspace_info",
            "title": "Workspace info",
            "description": "Summarize the workspace this server was started in: config \
        defaults (timeouts, retries, reporters, report output), configured devices with each \
        platform's action/check support matrix, scenes, plays with their acts and tags, and any \
        parse/validation diagnostics. Call this first to learn what can be run here.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "impresario_tags_list",
            "title": "List workspace tags",
            "description": "Index every declared tag across plays, acts, theatres and stages: \
        the tag, its usage count, the workspace-relative files that declare it and the usage \
        labels (play X, act X / Y, theatre X, stage X.Y). Parse/validation diagnostics are \
        included. Read-only; nothing is executed.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "impresario_validate",
            "title": "Validate workspace",
            "description": "Parse and validate the workspace (or the given files/directories) \
        without touching any device. Returns core diagnostics with their codes (IMP-DSL-*, \
        IMP-CFG-*) and workspace-relative positions. Use after writing or editing .scene/.play \
        files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files or directories to validate, relative to the \
        workspace. Default: the configured scenes and plays directories.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_devices_list",
            "title": "List devices",
            "description": "List the devices from impresario.devices.config (name, platform, \
        target endpoint, auth references and connector address). Apps are listed separately. \
        No device is contacted; use \
        impresario_devices_check to actually reach them.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "impresario_devices_check",
            "title": "Check devices",
            "description": "For each named device (default: all): prepare the transport, \
        connect through the on-device agent, handshake, read DeviceInfo and tear down. Takes \
        seconds per device and contacts the hardware. Serialized with runs (one device-side \
        operation at a time).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "devices": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Device names from the devices config. Default: all.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_probe_inspect",
            "title": "Inspect probe evidence",
            "description": "Run one live probe (showing, player or drm) against exactly one \
        device and return the canonical evidence: per-claim verdicts and confidence, the \
        source/class/observation table, observed values and the sampling window. Writes \
        report.json and evidence.json under <report_output>/<run_id>; read the bundle with \
        impresario_artifact_get. Requires a piece (Scene.Piece) or a raw selector, never \
        both. Serialized with runs and device checks (one device-side operation at a time).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["showing", "player", "drm"],
                        "description": "Probe family.",
                    },
                    "device": {
                        "type": "string",
                        "description": "Exactly one device name (or a group that resolves to one).",
                    },
                    "app": {
                        "type": "string",
                        "description": "App definition to launch before probing. Default: none.",
                    },
                    "piece": {
                        "type": "string",
                        "description": "Piece to probe as Scene.Piece, resolved through the \
        device platform's selector fallback chain.",
                    },
                    "selector": {
                        "type": "string",
                        "description": "Raw selector for the device platform (like a scene \
        selector value, without quotes).",
                    },
                    "claims": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Claim ids to assert (like --claim). Prefix with 'not ' \
        to invert. Showing always includes showing=present.",
                    },
                    "confidence": {
                        "type": "string",
                        "enum": ["none", "inferred", "corroborated", "authoritative"],
                        "description": "Minimum confidence required for a claim to pass. \
        Default: corroborated.",
                    },
                },
                "required": ["kind", "device"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_run",
            "title": "Run plays",
            "description": "Plan and execute plays on the selected devices through their \
        agents, then write reports to <report_output>/<run_id>. Returns a compact summary \
        (statuses, counts, failures with log tails, run_dir); fetch details with \
        impresario_report_get and impresario_artifact_get — failure screenshots come back as \
        images. Only file reporters are available over MCP (console output belongs to stdout, \
        which carries the protocol), default json+junit. One run at a time.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "plays": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Play names or simple globs (resolved against the \
        plays directory), workspace-relative paths or glob patterns (like --play). \
        Default: every play in the plays directory.",
                    },
                    "devices": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Device names to run on (like --device). Default: all \
        configured devices.",
                    },
                    "app": {
                        "type": "string",
                        "description": "App definition to bind (like --app); beats each play's \
        App: header. Default: the header, which must agree across selected plays.",
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only acts carrying one of these tags (like --tag).",
                    },
                    "exclude_tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Skip acts carrying one of these tags (like --exclude-tag).",
                    },
                    "acts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only acts with one of these names (like --act).",
                    },
                    "vars": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Variables that override scene pieces (name to value, \
        like --var name=value).",
                    },
                    "args": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Act parameters as ACT.PARAM to value (like \
        --arg ACT.PARAM=value).",
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 100,
                        "maximum": 600000,
                        "description": "Per-step timeout override (like --timeout).",
                    },
                    "retries": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 5,
                        "description": "Act retries (like --retries). Default: config.",
                    },
                    "fail_fast": {
                        "type": "boolean",
                        "description": "Cancel the rest of the run on the first failure (like \
        --fail-fast). Default: false.",
                    },
                    "reporters": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["json", "junit", "html"] },
                        "description": "File reporters to write. Default: json and junit. \
        Cheap step: the console reporter is not available over MCP.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_report_get",
            "title": "Get run report",
            "description": "Read a run's report.json (the complete Appendix-B run document: \
        statuses, steps, failures, screenshots, log tails). Without run_id the newest run under \
        the report directory is used.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": {
                        "type": "string",
                        "description": "Run id (a directory name under report_output). \
        Default: the newest run.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_artifacts_list",
            "title": "List run artifacts",
            "description": "List the files of a run (report.json, junit.xml, report.html, \
        screenshots/<device>/*.png, evidence.json) with sizes and kinds. Without run_id the newest \
        run is used. Pass watcher and dump together to list a watcher failure-dump bundle under \
        reports/watch/<watcher>/<dump> instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": {
                        "type": "string",
                        "description": "Run id. Default: the newest run. Mutually exclusive with \
        watcher+dump.",
                    },
                    "watcher": {
                        "type": "string",
                        "description": "Watcher name for a failure-dump bundle. Requires dump.",
                    },
                    "dump": {
                        "type": "string",
                        "description": "Dump directory name under reports/watch/<watcher>. \
        Requires watcher.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_artifact_get",
            "title": "Get run artifact",
            "description": "Read one artifact of a run or watcher dump. PNG/JPEG/WebP artifacts \
        (failure screenshots) come back as images so you can look at them; text artifacts come \
        back as text. Paths are relative to the run or dump directory and may not escape it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Artifact path relative to the run or dump directory, e.g. \
        report.json or screenshots/tv/failure-1.png.",
                    },
                    "run_id": {
                        "type": "string",
                        "description": "Run id. Default: the newest run. Mutually exclusive with \
        watcher+dump.",
                    },
                    "watcher": {
                        "type": "string",
                        "description": "Watcher name for a failure-dump bundle. Requires dump.",
                    },
                    "dump": {
                        "type": "string",
                        "description": "Dump directory name under reports/watch/<watcher>. \
        Requires watcher.",
                    },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_jobs_list",
            "title": "List background jobs",
            "description": "List the jobs recorded in this home, newest first. Default: active \
        (queued/running) only; pass all: true to include finished jobs. Stale jobs whose owner \
        process is gone are marked interrupted first, like `impresario jobs`. Bounded by limit \
        (default 50, maximum 500). Read-only apart from that stale-job recovery.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "all": {
                        "type": "boolean",
                        "description": "Include finished jobs. Default: false (active only).",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 500,
                        "description": "Maximum jobs to return. Default: 50.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_jobs_get",
            "title": "Get a background job",
            "description": "Read one job record by id: status, exit code, duration, workspace, \
        devices, plays, label and run_dir. report_copy says whether the completed report.json was \
        copied into the job directory; use impresario_report_get with the job id as run_id for \
        the run report. Read-only apart from stale-job recovery.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Job id, as printed by impresario_run or \
        impresario_jobs_list.",
                    },
                },
                "required": ["id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_service_status",
            "title": "Resident service status",
            "description": "Read the resident runner state: whether the service process is \
        alive, paused, its pid/workspace/start time, and the persisted per-schedule and \
        per-watcher counters from service/state.json. Read-only; it does not start the service.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "impresario_schedules_list",
            "title": "List resident schedules",
            "description": "List the effective schedules from the layered workspace and global \
        config, joined with persisted runs/failures and the next host-local fire time. A \
        disabled schedule has no next fire. Read-only.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "impresario_schedule_add",
            "title": "Add resident schedule",
            "description": "Add one schedule to the workspace (default) or global config. Give \
        either a 'schedule' object with the same fields as YAML (when, run, on_missed, on_busy, \
        notify, enabled) or a 'yaml' string. 'name' is authoritative; a mismatching name inside \
        the body is rejected. The resulting file is validated through the normal config loader.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Unique schedule name.",
                    },
                    "schedule": {
                        "type": "object",
                        "description": "Schedule body (without or with the same name).",
                    },
                    "yaml": {
                        "type": "string",
                        "description": "Schedule body as YAML; mutually exclusive with \
        'schedule'.",
                    },
                    "layer": {
                        "type": "string",
                        "enum": ["workspace", "global"],
                        "description": "Which config layer to edit. Default: workspace.",
                    },
                },
                "required": ["name"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_schedule_remove",
            "title": "Remove resident schedule",
            "description": "Remove one schedule by name from the workspace (default) or global \
        config. Fails if that layer has no such schedule; a definition in the other layer is \
        left untouched.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Schedule name.",
                    },
                    "layer": {
                        "type": "string",
                        "enum": ["workspace", "global"],
                        "description": "Which config layer to edit. Default: workspace.",
                    },
                },
                "required": ["name"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_schedule_run_now",
            "title": "Run a schedule now",
            "description": "Run one configured schedule immediately through the normal run \
        pipeline, in this process, serialized with impresario_run (one device-side run at a \
        time). The result carries the same summary as impresario_run. File reporters only. This \
        manual run does not update service last_fire counters and does not send schedule \
        notification events.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Schedule name.",
                    },
                },
                "required": ["name"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_watchers_list",
            "title": "List resident watchers",
            "description": "List the effective watchers, with enabled, per-watcher paused, \
        persisted iterations/failures, last start/status/dump and the next iteration time. \
        Read-only.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "impresario_watcher_start",
            "title": "Start resident watcher",
            "description": "Enable a watcher in the workspace (default) or global config and \
        clear its stop/pause control files. A running service picks it up; a stopped service \
        starts it the next time the service runs. Does not start the service itself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Watcher name.",
                    },
                    "layer": {
                        "type": "string",
                        "enum": ["workspace", "global"],
                        "description": "Which config layer to edit. Default: workspace.",
                    },
                },
                "required": ["name"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_watcher_stop",
            "title": "Stop resident watcher",
            "description": "Disable a watcher in the workspace (default) or global config and \
        write its stop control file, so a running service ends the current iteration loop after \
        the in-flight iteration. Does not stop the resident service.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Watcher name.",
                    },
                    "layer": {
                        "type": "string",
                        "enum": ["workspace", "global"],
                        "description": "Which config layer to edit. Default: workspace.",
                    },
                },
                "required": ["name"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "impresario_watcher_dumps",
            "title": "List watcher dumps",
            "description": "List timestamped dump directories under reports/watch for one \
        watcher, or for every watcher when 'name' is omitted, newest last. Paths are \
        workspace-relative. Read-only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Watcher name. Default: every watcher with dumps.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_list_devices",
            "title": "List inspector devices",
            "description": "List configured devices with their platform and whether the running \
        inspector currently holds one attached. Reads the loopback inspector discovered from \
        ~/.impresario/inspector.json; start one with `impresario inspector`.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "inspector_attach",
            "title": "Attach inspector device",
            "description": "Attach the running inspector to one configured device. The \
        inspector takes the interactive lease and starts polling snapshots; it detaches any \
        previous device first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device": {
                        "type": "string",
                        "description": "Device name from impresario.devices.config.",
                    },
                },
                "required": ["device"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_detach",
            "title": "Detach inspector device",
            "description": "Release the inspector's interactive lease and stop snapshot \
        polling. The Web UI keeps serving but reports no attachment.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "inspector_get_view",
            "title": "Get compact inspector view",
            "description": "Return the current viewport snapshot as token-efficient text \
        (one line per node with role, name, [ref=eN] and state hints), plus the revision and \
        truncation flags. Use refs with inspector_describe and the interaction tools.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "max_nodes": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum element lines. Default: 400.",
                    },
                    "max_depth": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum tree depth. Default: 32.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_wait_for_change",
            "title": "Wait for inspector change",
            "description": "Block until the inspector revision advances past 'since' (or the \
        timeout elapses) and return the new compact view. Omit 'since' to wait from the current \
        revision.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Revision to wait past. Default: the current revision.",
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum wait in milliseconds. Default: 20000, max 120000.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_get_screenshot",
            "title": "Get inspector screenshot",
            "description": "Return the current device frame as a PNG image block, scaled by \
        inspector.screenshot_scale. Acquisition failures are reported as tool errors.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "inspector_find",
            "title": "Find by locator",
            "description": "Resolve one locator against the current snapshot and return every \
        match with refs, roles, names and visibility. Supports the additive locator grammar: \
        role=, text=, name=, testid=, within(...), '>>' chains and focused.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "locator": {
                        "type": "string",
                        "description": "Locator expression, for example role=button[name=\"Play\"].",
                    },
                },
                "required": ["locator"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_describe",
            "title": "Describe inspector node",
            "description": "Return every known field of one node: role, name, text, value, \
        test id, native id, bounds, state, scroll info, CSS/XPath selectors and raw platform \
        data. Secure field values are masked.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": {
                        "type": "string",
                        "description": "Node reference such as e12.",
                    },
                    "id": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Numeric node id (alternative to ref).",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_suggest_locators",
            "title": "Suggest locators",
            "description": "Return ranked, verified locator candidates for one node, with \
        stability scores and notes. Prefer the first stable candidate when adding a scene cell.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Node reference such as e12." },
                    "id": { "type": "integer", "minimum": 1, "description": "Numeric node id." },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_verify_locator",
            "title": "Verify locator",
            "description": "Count matches for one locator and report whether it is unique, \
        visible and stable on the currently attached device.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "locator": { "type": "string", "description": "Locator expression." },
                },
                "required": ["locator"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_tap",
            "title": "Tap inspector target",
            "description": "Tap a node ref/id or logical screen coordinates through the \
        inspector's interactive lease. Returns the post-action revision and compact view.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Node reference such as e12." },
                    "id": { "type": "integer", "minimum": 1, "description": "Numeric node id." },
                    "x": { "type": "number", "description": "Logical x coordinate." },
                    "y": { "type": "number", "description": "Logical y coordinate." },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_scroll",
            "title": "Scroll inspector target",
            "description": "Scroll a node ref/id or logical screen coordinates in one \
        direction. Returns the post-action revision and compact view.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Node reference such as e12." },
                    "id": { "type": "integer", "minimum": 1, "description": "Numeric node id." },
                    "x": { "type": "number", "description": "Logical x coordinate." },
                    "y": { "type": "number", "description": "Logical y coordinate." },
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down", "left", "right"],
                        "description": "Scroll direction. Default: down.",
                    },
                    "count": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Number of steps. Default: 1.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_key",
            "title": "Press inspector key",
            "description": "Press one remote/hardware key through the inspector's interactive \
        lease. Key names are the DSL names (back, up, down, left, right, ok, play, red, ...).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Remote key name." },
                    "count": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Number of presses. Default: 1.",
                    },
                    "ref": { "type": "string", "description": "Optional focused node ref." },
                    "id": { "type": "integer", "minimum": 1, "description": "Optional node id." },
                },
                "required": ["key"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_type",
            "title": "Type into inspector target",
            "description": "Insert text into the focused field, or into one node ref/id. The \
        executor decides whether it can focus the field; failures carry the wire error code.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to insert." },
                    "ref": { "type": "string", "description": "Optional text field ref." },
                    "id": { "type": "integer", "minimum": 1, "description": "Optional node id." },
                },
                "required": ["text"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_scene_list",
            "title": "List scenes",
            "description": "List .scene files under the configured scenes directory with \
        workspace-relative paths and byte sizes.",
            "inputSchema": { "type": "object", "additionalProperties": false },
        }),
        json!({
            "name": "inspector_scene_create",
            "title": "Create scene",
            "description": "Create a new scene skeleton with one column per platform and write \
        it atomically. Fails if the file already exists.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative .scene path.",
                    },
                    "name": { "type": "string", "description": "Scene name." },
                    "platforms": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Platforms to create columns for (web, webOS, tizenOS, android, androidtv, iOS, tvOS).",
                    },
                },
                "required": ["path", "name"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_scene_add_element",
            "title": "Add scene element",
            "description": "Preview (default) or commit an additive scene edit: add one piece \
        row with a locator in the resolved platform column. The column is selected \
        automatically from 'platform' unless 'column' names it. Returns the diff, revision and \
        edit id; set commit=true to write it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative .scene path." },
                    "row": { "type": "string", "description": "Piece name. Default: suggested from seed." },
                    "column": { "type": "string", "description": "Exact column name (optional)." },
                    "platform": { "type": "string", "description": "Platform used to select the column." },
                    "locator": { "type": "string", "description": "Locator expression for the cell." },
                    "seed": { "type": "string", "description": "Seed used to suggest a piece name." },
                    "base_revision": { "type": "string", "description": "Revision the edit is based on." },
                    "commit": { "type": "boolean", "description": "Write the edit immediately." },
                },
                "required": ["path", "locator"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_scene_fill_cell",
            "title": "Fill scene cell",
            "description": "Preview (default) or commit filling an empty cell with a locator. \
        Fails when the cell is already occupied; use inspector_scene_replace_cell for that.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative .scene path." },
                    "row": { "type": "string", "description": "Existing piece name." },
                    "column": { "type": "string", "description": "Exact column name (optional)." },
                    "platform": { "type": "string", "description": "Platform used to select the column." },
                    "locator": { "type": "string", "description": "Locator expression for the cell." },
                    "base_revision": { "type": "string", "description": "Revision the edit is based on." },
                    "commit": { "type": "boolean", "description": "Write the edit immediately." },
                },
                "required": ["path", "row", "locator"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_scene_replace_cell",
            "title": "Replace scene cell",
            "description": "Preview (default) or commit replacing an occupied cell. The first \
        call without confirm=true returns SCENE_CONFIRM_REQUIRED and the diff; repeat with \
        confirm=true after reviewing it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative .scene path." },
                    "row": { "type": "string", "description": "Existing piece name." },
                    "column": { "type": "string", "description": "Exact column name (optional)." },
                    "platform": { "type": "string", "description": "Platform used to select the column." },
                    "locator": { "type": "string", "description": "Replacement locator expression." },
                    "confirm": { "type": "boolean", "description": "Confirm replacing an occupied cell." },
                    "base_revision": { "type": "string", "description": "Revision the edit is based on." },
                    "commit": { "type": "boolean", "description": "Write the edit immediately." },
                },
                "required": ["path", "row", "locator"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "inspector_scene_validate",
            "title": "Validate scene",
            "description": "Parse and validate one scene without writing it; returns the \
        diagnostics and the resolved columns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative .scene path." },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
    ]
}

/// `tools/call`: validate the envelope, run the tool, shape the result.
pub(crate) fn call(server: &Server, params: &Value) -> RpcResult {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "tools/call requires a string 'name'",
        ));
    };
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "tools/call 'arguments' must be an object",
            ));
        }
    };
    let output = match name {
        "impresario_workspace_info" => handlers::workspace_info(server, &arguments),
        "impresario_tags_list" => handlers::tags_list(server, &arguments),
        "impresario_validate" => handlers::validate(server, &arguments),
        "impresario_devices_list" => handlers::devices_list(server, &arguments),
        "impresario_devices_check" => handlers::devices_check(server, &arguments),
        "impresario_probe_inspect" => handlers::probe_inspect(server, &arguments),
        "impresario_run" => handlers::run(server, &arguments),
        "impresario_report_get" => handlers::report_get(server, &arguments),
        "impresario_artifacts_list" => handlers::artifacts_list(server, &arguments),
        "impresario_artifact_get" => handlers::artifact_get(server, &arguments),
        "impresario_jobs_list" => handlers::jobs_list(server, &arguments),
        "impresario_jobs_get" => handlers::jobs_get(server, &arguments),
        "impresario_service_status" => handlers::service_status(server, &arguments),
        "impresario_schedules_list" => handlers::schedules_list(server, &arguments),
        "impresario_schedule_add" => handlers::schedules_add(server, &arguments),
        "impresario_schedule_remove" => handlers::schedules_remove(server, &arguments),
        "impresario_schedule_run_now" => handlers::schedule_run_now(server, &arguments),
        "impresario_watchers_list" => handlers::watchers_list(server, &arguments),
        "impresario_watcher_start" => handlers::watcher_start(server, &arguments),
        "impresario_watcher_stop" => handlers::watcher_stop(server, &arguments),
        "impresario_watcher_dumps" => handlers::watcher_dumps(server, &arguments),
        "inspector_list_devices" => inspector_tools::list_devices(server, &arguments),
        "inspector_attach" => inspector_tools::attach(server, &arguments),
        "inspector_detach" => inspector_tools::detach(server, &arguments),
        "inspector_get_view" => inspector_tools::get_view(server, &arguments),
        "inspector_wait_for_change" => inspector_tools::wait_for_change(server, &arguments),
        "inspector_get_screenshot" => inspector_tools::get_screenshot(server, &arguments),
        "inspector_find" => inspector_tools::find(server, &arguments),
        "inspector_describe" => inspector_tools::describe(server, &arguments),
        "inspector_suggest_locators" => inspector_tools::suggest_locators(server, &arguments),
        "inspector_verify_locator" => inspector_tools::verify_locator(server, &arguments),
        "inspector_tap" => inspector_tools::tap(server, &arguments),
        "inspector_scroll" => inspector_tools::scroll(server, &arguments),
        "inspector_key" => inspector_tools::key(server, &arguments),
        "inspector_type" => inspector_tools::type_text(server, &arguments),
        "inspector_scene_list" => inspector_tools::scene_list(server, &arguments),
        "inspector_scene_create" => inspector_tools::scene_create(server, &arguments),
        "inspector_scene_add_element" => inspector_tools::scene_add_element(server, &arguments),
        "inspector_scene_fill_cell" => inspector_tools::scene_fill_cell(server, &arguments),
        "inspector_scene_replace_cell" => inspector_tools::scene_replace_cell(server, &arguments),
        "inspector_scene_validate" => inspector_tools::scene_validate(server, &arguments),
        other => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown tool '{other}'"),
            ));
        }
    };
    Ok(output.into_result())
}

/// What a tool produced. Failures of the *operation* (a device did not
/// answer, a path does not exist, an argument is out of range) are tool
/// execution errors (`isError: true` in the result), not JSON-RPC errors.
pub(crate) struct ToolOutput {
    pub text: String,
    pub structured: Option<Value>,
    /// `(base64, mime type)` for image results.
    pub image: Option<(String, String)>,
    pub is_error: bool,
}

impl ToolOutput {
    pub(crate) fn json(value: Value) -> Self {
        let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
        Self {
            text,
            structured: Some(value),
            image: None,
            is_error: false,
        }
    }

    pub(crate) fn json_with_prefix(value: Value, prefix: impl Into<String>) -> Self {
        let mut output = Self::json(value);
        output.text = format!("{}\n{}", prefix.into(), output.text);
        output
    }

    pub(crate) fn message(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            structured: None,
            image: None,
            is_error: false,
        }
    }

    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self {
            text: message.into(),
            structured: None,
            image: None,
            is_error: true,
        }
    }

    pub(crate) fn image(
        base64: String,
        mime: impl Into<String>,
        caption: impl Into<String>,
    ) -> Self {
        Self {
            text: caption.into(),
            structured: None,
            image: Some((base64, mime.into())),
            is_error: false,
        }
    }

    pub(crate) fn into_result(self) -> Value {
        let mut result = Map::new();
        let mut content = Vec::new();
        if let Some((data, mime_type)) = self.image {
            content.push(json!({ "type": "image", "data": data, "mimeType": mime_type }));
        }
        content.push(json!({ "type": "text", "text": self.text }));
        result.insert("content".to_string(), Value::Array(content));
        if let Some(structured) = self.structured {
            result.insert("structuredContent".to_string(), structured);
        }
        if self.is_error {
            result.insert("isError".to_string(), Value::Bool(true));
        }
        Value::Object(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_unique_and_schemaful() {
        let tools = definitions();
        assert_eq!(tools.len(), 41);
        let mut names = std::collections::BTreeSet::new();
        for tool in &tools {
            let name = tool["name"].as_str().expect("name").to_string();
            assert!(names.insert(name.clone()), "duplicate {name}");
            assert_eq!(
                tool["inputSchema"]["type"],
                json!("object"),
                "{name} schema"
            );
            assert!(
                tool["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{name} description"
            );
        }
    }

    #[test]
    fn tool_output_shapes_text_structured_and_image() {
        let json_output = ToolOutput::json(json!({ "a": 1 })).into_result();
        assert_eq!(json_output["structuredContent"]["a"], json!(1));
        assert_eq!(json_output["content"][0]["type"], json!("text"));
        assert!(json_output.get("isError").is_none());

        let error_output = ToolOutput::error("boom").into_result();
        assert_eq!(error_output["isError"], json!(true));

        let image_output = ToolOutput::image("AAA".to_string(), "image/png", "cap").into_result();
        assert_eq!(image_output["content"][0]["type"], json!("image"));
        assert_eq!(image_output["content"][0]["mimeType"], json!("image/png"));
        assert_eq!(image_output["content"][1]["type"], json!("text"));
    }
}
