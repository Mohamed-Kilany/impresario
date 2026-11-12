# The MCP server (`impresario mcp`)

`impresario mcp` serves a [Model Context Protocol](https://modelcontextprotocol.io)
interface on **stdio** so any MCP-capable AI agent can inspect the
workspace, author tests against the DSL reference, check devices, run plays,
debug failures, and inspect or edit the resident schedules and watchers —
without shelling out to the CLI.

It is a CLI addition, not part of the spec surface (decision I-80, the same
pattern as `mock-agent`, I-77).

```sh
cd my-workspace
impresario mcp        # serves until stdin closes; logs and diagnostics go to stderr
```

`--workspace`, `--config` and `--devices-config` select the workspace (they
default to the current directory and `impresario.config` /
`impresario.devices.config`, exactly like every other command).

## Connecting a client

Any client that can launch a stdio MCP server works. Examples:

```sh
# Claude Code, from the workspace directory
claude mcp add impresario -- impresario mcp

# Claude Code, pinned to a workspace
claude mcp add impresario -- impresario --workspace D:/Projects/my-app mcp
```

```jsonc
// Claude Desktop (claude_desktop_config.json); VS Code / Cursor use the
// same object under "servers" (VS Code, .vscode/mcp.json) or "mcpServers".
{
  "mcpServers": {
    "impresario": {
      "command": "impresario",
      "args": ["--workspace", "D:/Projects/my-app", "mcp"]
    }
  }
}
```

The server is **dual-era**: a 2026-07-28 client discovers it with
`server/discover` and sends `_meta.io.modelcontextprotocol/protocolVersion`
on every request (stateless); older clients negotiate with `initialize`
(handshake era, 2025-11-25 and back). Both get the same tools.

## Tools

All paths are workspace-relative unless absolute. Results carry a text
block (a summary line plus pretty JSON) and, where it matters,
`structuredContent`; screenshots come back as MCP **image** content so the
agent can look at them. Operation failures (a device is down, an artifact
does not exist, an argument is out of range) are tool errors
(`isError: true`), not protocol errors.

| Tool | Arguments | What it does |
| --- | --- | --- |
| `impresario_workspace_info` | — | Config defaults, devices **with each platform's action/check support matrix**, scenes, plays, acts, tags and diagnostics. Call this first. |
| `impresario_tags_list` | — | Every declared tag across plays, acts, theatres and stages: usage count, workspace-relative files and usage labels (`play X`, `act X / Y`, `theatre X`, `stage X.Y`), plus diagnostics. Same index as `impresario tags --json`; reads only the workspace config. Read-only. |
| `impresario_validate` | `paths?` | Parse/validate the workspace (or listed files/dirs) without touching a device. Returns core diagnostics with codes. |
| `impresario_devices_list` | - | The effective devices: name, exact type, IP, derived platform/transport and support matrix. No device contacted. |
| `impresario_devices_check` | `devices?` | Prepare → connect → handshake → `DeviceInfo` → teardown per device. Contacts hardware; takes seconds per device. |
| `impresario_probe_inspect` | `kind`, `device`, `app?`, `piece?` or `selector?`, `claims?`, `confidence?` | Run one live probe on exactly one device and return canonical evidence: per-claim verdicts/confidence, source/class/observation rows, observed values and the window. Writes `report.json` plus an `evidence.json` bundle under `<report_output>/<run_id>`; read it with `impresario_artifact_get`. Serialized with runs and device checks. |
| `impresario_run` | `plays?`, `devices?`, `tags?`, `exclude_tags?`, `acts?`, `vars?`, `args?`, `timeout_ms?`, `retries?`, `fail_fast?`, `reporters?` | Plan and execute; writes `report_output/<run_id>` and returns a compact summary (statuses, counts, failures with the last 10 log lines, `run_dir`). `plays` entries may be bare names or simple globs (resolved against the plays directory) as well as workspace-relative paths or globs; a refusal lists the underlying diagnostics, not just a count. |
| `impresario_report_get` | `run_id?` | The full `report.json` (Appendix B document). Default: the newest run. |
| `impresario_artifacts_list` | `run_id?` or `watcher`+`dump` | Files of a run with sizes and kinds (`report.json`, `junit.xml`, `report.html`, `screenshots/<device>/*.png`, `evidence.json`). With `watcher` and `dump` (both required, not with `run_id`) it lists one watcher failure-dump bundle under `reports/watch/<watcher>/<dump>` instead. |
| `impresario_artifact_get` | `path`, `run_id?` or `watcher`+`dump` | One artifact from a run or watcher failure dump. Images (PNG/JPEG/WebP) return as image content; text and evidence artifacts return as text (truncated at 2 MiB). Paths stay inside the run or dump directory. |
| `impresario_jobs_list` | `all?`, `limit?` | Background jobs newest first. Default: active (queued/running) only; `all: true` includes finished jobs. Stale jobs whose owner is gone are marked interrupted first, like `impresario jobs`. `limit` defaults to 50 (maximum 500); the result reports the filtered total, the returned count and the `all` flag. |
| `impresario_jobs_get` | `id` | One job record: status, exit code, duration, workspace, devices, plays, label and `run_dir`, plus `report_copy` (whether the completed `report.json` was copied into the job directory). Use `impresario_report_get` with the job id as `run_id` for that report. |
| `impresario_service_status` | — | Resident service state: running/paused, pid, workspace, start time, the persisted schedule/watcher counters, `runner_version`/`client_version`, `restart_required` and `heartbeat_stale`. Read-only. |
| `impresario_schedules_list` | — | Effective schedules joined with runs/failures and the next host-local fire. Disabled schedules have no next fire. |
| `impresario_schedule_add` | `name`, `schedule` or `yaml`, `layer?` | Add one schedule to the workspace (default) or global config. `name` is authoritative; the full file is validated before writing. |
| `impresario_schedule_remove` | `name`, `layer?` | Remove one schedule from that layer. A definition in the other layer is left untouched. |
| `impresario_schedule_run_now` | `name` | Run one schedule immediately through the normal pipeline, in this process. Shares the device lock with `impresario_run`; file reporters only. Does not update service `last_fire` counters or send schedule notifications. |
| `impresario_watchers_list` | — | Effective watchers with enabled/paused, lease mode (`gap`/`no_gap`), iterations/failures, last start/status/dump and next iteration. |
| `impresario_watcher_start` | `name`, `layer?` | Enable a watcher and clear its stop/pause control files. Does not start the resident service. |
| `impresario_watcher_stop` | `name`, `layer?` | Disable a watcher and write its stop control file; a running service ends after the in-flight iteration. |
| `impresario_watcher_dumps` | `name?` | Timestamped dump directories under `reports/watch`, workspace-relative. Default: every watcher. |

### Inspector tools

The inspector tools are thin clients of a running `impresario inspector`
process (see `docs/CLI.md`). That process owns the interactive device lease and
the live snapshot; the tools discover its loopback URL and token from
`~/.impresario/inspector.json`, so the UI and the agent always see the same
revision. Without a running inspector they fail with a message that names the
missing state file.

| Tool | Arguments | What it does |
| --- | --- | --- |
| `inspector_list_devices` | — | Configured devices with platform, support and whether the inspector holds one attached. |
| `inspector_attach` | `device` | Attach the inspector to one device (detaches the previous one) and start polling. |
| `inspector_detach` | — | Release the lease and stop polling. |
| `inspector_get_view` | `max_nodes?`, `max_depth?` | The current viewport as the compact, token-efficient tree with `[ref=eN]` handles. |
| `inspector_wait_for_change` | `since?`, `timeout_ms?` | Long-poll until the snapshot revision changes (default 20 s, maximum 120 s). |
| `inspector_get_screenshot` | — | The current frame as MCP image content. |
| `inspector_find` | `locator` | Resolve one locator against the live view; returns matches with refs. |
| `inspector_describe` | `ref` or `id` | Full node details, including raw attributes and geometry. |
| `inspector_suggest_locators` | `ref` or `id` | Ranked, uniqueness-checked locator candidates for one node. |
| `inspector_verify_locator` | `locator` | Match count and uniqueness for the attached device. |
| `inspector_tap` | `ref`/`id` or `x`+`y` | Tap one node or coordinate, then return the post-action view delta. |
| `inspector_scroll` | `ref`/`id?`, `direction?`, `count?` | Scroll a container (or the primary scroller) and return the delta. |
| `inspector_key` | `key`, `count?` | Press a remote key and return the delta. |
| `inspector_type` | `text`, `ref`/`id?` | Focus a field, insert text and return the delta. |
| `inspector_scene_list` | — | Scene files in the workspace with sizes. |
| `inspector_scene_create` | `path`, `name`, `platforms?` | Create an in-memory scene draft; commit it with the editor tools or the UI. |
| `inspector_scene_add_element` | `path`, `row`, `locator`, `column?`, `platform?`, `seed?`, `commit?`, `base_revision?` | Preview (and optionally commit) an add-element edit. |
| `inspector_scene_fill_cell` | `path`, `row`, `column`, `locator`, `platform?`, `commit?`, `base_revision?` | Fill an empty cell; refuses an occupied one. |
| `inspector_scene_replace_cell` | `path`, `row`, `column`, `locator`, `confirm?`, `commit?`, `base_revision?` | Replace an occupied cell; the diff is returned unless `confirm` is true. |
| `inspector_scene_validate` | `path` | Parse and validate one scene without writing it. |

Scene writes are byte-preserving, stay inside the workspace, use SHA-256
revisions and refuse to overwrite a file that changed since the preview. The
compact view is viewport-only; off-screen subtrees are pruned, focus is kept,
and truncation is reported explicitly. The optional MCP resource
`impresario://inspector/{device}/view` and its
`notifications/resources/updated` subscription are not implemented yet; use
`inspector_wait_for_change` for the same effect.

Notes:

- **The console reporter is not available over MCP**: it writes to stdout,
  which carries the protocol. Runs default to `json,junit`; pass
  `reporters: ["json","junit","html"]` for more files. Asking for
  `console` is refused with an explanation.
- **Act arguments** use the `args` object (`{"Login.user": "alice"}`), the
  MCP equivalent of repeatable `--arg 'ACT.PARAM=VALUE'`; precedence and
  validation are identical to the CLI.
- **One run or device check at a time**: both take a process-wide lock, so
  two agents (or two tool calls) can never drive one device concurrently.
  `impresario_schedule_run_now` takes the same device lock; schedule/watcher
  config edits take a separate resident lock, so they never race each other
  or a manual schedule run. Other tools stay available while a run is in
  flight, and responses may arrive out of order (they are correlated by
  JSON-RPC id, as the spec allows).
- **Resident state is shared**: the tools read `service/state.json` and the
  layered config used by the CLI and the TUI Resident screen. `schedules_list` and
  `watchers_list` are read-only; `schedule_add/remove` and
  `watcher_start/stop` use the same lossless YAML editor as the CLI. Starting
  or stopping a watcher does not start the service; a stopped service picks
  up the change the next time it runs.
- **A manual schedule run is not a scheduled fire**: it returns the normal
  run summary but does not advance `last_fire`, increment schedule counters,
  or emit `schedule_notify` events. Use the resident service when those
  effects matter.
- Runs respect the config's per-step timeouts and retries; there is no
  separate MCP-level timeout, so a long run is a long tool call.

## Resources

| URI | Content |
| --- | --- |
| `impresario://docs/dsl` | The DSL reference (`docs/DSL.md`), embedded at build time. |
| `impresario://docs/parity` | The platform support matrix (`docs/PARITY.md`), generated from the planner tables. |
| `impresario://docs/devices` | Device setup guide (`docs/DEVICES.md`): adb/Wi-Fi Android, Android TV, web, webOS/Tizen, emulators. |
| `impresario://docs/mcp` | This document. |
| `impresario://docs/scheduling` | Schedules, watchers, resident delivery and notification reference (`docs/SCHEDULING.md`). |
| `impresario://docs/probes` | Probe evidence model: kinds, evidence sources and confidence, per-platform reachability, fallback rungs and the in-app SDK contract (`docs/PROBES.md`). |
| `impresario://docs/web` | Embedded browser console (`docs/WEB.md`): guided flow, screens, JSON API, curated actions, security model and AI-agent workflow. |
| `impresario://workspace/info` | Live JSON of the workspace summary (same payload as `impresario_workspace_info`). |

Embedding the docs means the agent always reads the documentation that
matches its server build.

## A suggested loop for an agent

1. `impresario_workspace_info` — learn the devices and the existing scenes.
2. Read `impresario://docs/dsl` and `impresario://docs/parity` — authoring
   rules and what the target platform supports (e.g. `swipe` is
   unsupported on TVs; D-pad navigation uses `focus`/`keys`).
3. Write or edit `.scene`/`.play` files with your own file tools.
4. `impresario_validate` — fix diagnostics before touching hardware.
5. `impresario_devices_check` — is the target reachable (agent installed,
   screen on, adb connected)?
6. `impresario_run` — run the play; on failure the summary points at the
   failed step and screenshots.
7. `impresario_artifact_get` on the failure screenshot (you will receive an
   image), `impresario_report_get` for the full document, then iterate.

## Implementation notes

- Transport: newline-delimited JSON-RPC 2.0 on stdio (the MCP stdio
  binding), one message per line, `tools/call` answered on a worker thread.
- Stdout is protocol-only. Every log, diagnostic and progress note goes to
  stderr (`tracing`), including the CLI diagnostics the run pipeline
  renders.
- Errors: `-32700` parse, `-32600` invalid request, `-32601` unknown
  method, `-32602` invalid params (including unknown tools), `-32603`
  internal, `-32022` unsupported protocol version (2026-07-28 revision)
  with `data.supported`.
- Artifact reads are confined to the run directory: absolute paths, `..`
  components and symlinks that resolve outside are rejected.
- The server exits when stdin closes (the client hung up); an in-flight run
  is abandoned with the process.
- No new dependencies: the protocol layer is hand-rolled on `serde_json`,
  and device work reuses the same code paths as the CLI (`validate`,
  `device check`/`devices check`, `run`). `run` accepts an optional `app`
  argument that overrides the selected plays' `App:` headers.

## Security

Any process that can spawn `impresario mcp` can drive the configured
devices (that is the point). The server adds no authentication of its own —
wire it only into clients you trust, and remember that run reports are
redacted by the same rules as CLI runs (spec 5.6).
