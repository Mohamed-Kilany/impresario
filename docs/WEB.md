# Web console

`impresario web` serves a browser console on a dedicated port. It is the third
front end of the same binary, next to the CLI and the TUI, and it follows the
same rule: **the console is a client of the CLI**. Reads call the shared
library functions or the CLI's JSON output; every mutation spawns the current
`impresario` executable with the workspace, config and devices flags the TUI
uses. Validation, exit codes, diagnostics, redaction, device leases and
resident locks therefore have exactly one implementation (I-192).

Everything the console needs — the single-page app, stylesheet, script and
favicon — is compiled into the binary. There is no npm, no CDN, no external
font and no network request except to the console itself.

## Quick start

```text
impresario web                       # http://127.0.0.1:7810/
impresario web --port 0              # any free port; the URL is printed
impresario web --open                # also open the default browser
impresario web --bind 0.0.0.0 --allow-remote   # trusted networks only
```

The server prints the URL on stdout and the workspace on stderr before
accepting connections:

```text
impresario web: listening on http://127.0.0.1:7810/
impresario web: workspace D:\work\my-tests
```

`--port 0` is intended for tests and for callers that cannot reserve a port in
advance. A non-loopback `--bind` is refused unless `--allow-remote` is passed;
remote mode has no TLS and no authentication (see **Security model**).

`impresario web` exits with the usual codes: `2` for an invalid bind address or
a missing `--allow-remote`, `3` when the port cannot be bound.

## Interface

The console is light-first: a warm off-white canvas, white cards, a teal accent
and the system font stack. The topbar toggle switches to a dark theme and back;
the choice is stored in `localStorage` under `impresario.theme`. A fresh
browser starts light regardless of the operating-system preference, and the
toggle keeps `aria-pressed`, its label/title and `meta[name="theme-color"]` in
sync. The theme belongs to the browser profile; it is not a workspace setting
and has no API endpoint.

Everything remains embedded and CSP-compatible: no inline scripts or styles, no
CDN font and no new network request. The brand mark and navigation icons are
inline SVG, while select chevrons are CSS `data:` images permitted by
`img-src`. The sticky chrome groups the workspace picker, server status,
refresh and theme controls; every screen uses the same editorial page heading
and card system.

The Runs screen uses a full-width jobs table with separate selected-job and
reports cards, so long logs and artifact lists keep the full width. Tables
scroll horizontally on narrow screens, forms collapse to one column below
720 px, and `prefers-reduced-motion` disables the animations. Focus styles,
`aria-current` navigation and the existing `aria-live` regions remain in place.

## The guided flow

The Start screen is the default route and mirrors the recommended order. Each
step is one button and one API action:

1. **Choose a workspace.** The picker lists the active workspace and every
   path in the global `workspaces` setting. `workspace_use` makes one the
   global default and switches the running console to it without a restart.
2. **Validate.** `validate` parses every configured scene, theatre and play.
   Fix DSL and config errors here before touching a device.
3. **Check devices.** `device_check` runs transport prepare, handshake and
   `DeviceInfo` for the selected devices.
4. **Preview the run.** `run` with `dry_run: true` resolves plays, devices and
   acts and prints the plan without touching hardware.
5. **Start a detached run.** `run` without `dry_run` submits the job to the
   supervisor. The response carries `job_id`; the Runs screen follows its log,
   events, report and artifacts.
6. **Inspect the evidence.** Reports, failure screenshots, probe evidence and
   watcher dumps are read from the same files the CLI and MCP server expose.

The Start screen shows the checklist state, a quick-run form (play, devices,
app, preview toggle), validation errors and links to the other screens.

## Screens

| Route | What it shows |
|---|---|
| `#/start` | First-run checklist, quick run, console output, next steps |
| `#/runs` | Supervisor jobs and report directories, with log, events, report and artifacts |
| `#/tests` | Scene/theatre/play tree, source editor, save + validate, grep, create file |
| `#/coverage` | Stage coverage matrix and configured devices missing an implementation |
| `#/probe` | Plan-time probe support per device and one live `showing`/`player`/`drm` probe with evidence |
| `#/groups` | Effective actor groups, resolved members, source layer, add/set/remove |
| `#/devices` | Effective devices, source layer, add/edit/rename/remove |
| `#/apps` | Effective apps, source layer, add/rename/remove |
| `#/config` | Every setting with value, source layer, schema and set/unset |
| `#/resident` | Service state, schedules, watchers, dumps, notifications |
| `#/docs` | The Markdown documentation compiled into this binary |
| `#/api` | Machine-readable schema, guided flow, endpoints and copyable `curl` |

The Tests editor saves through `test_save`: the file is written, `validate`
runs on it, and a failing validation restores the previous contents (or
removes a newly created file). The response reports `rolled_back`. A
pre-existing error elsewhere in the workspace can also fail validation, so the
editor reports the full validator output instead of assuming the edited file
is at fault.

Group, device and app rows show their source layer. Set, edit, rename and
remove actions send that layer explicitly, so changing a global definition
does not silently create a workspace shadow copy.

The TUI's home/editor scope is the equivalent browse-and-edit surface. The web
console adds the same browse, edit, validate, run and evidence loop through the
browser, and both remain CLI clients with no duplicated validation.

## Architecture

```text
browser ──HTTP──> impresario web (this process)
                       │
                       ├─ reads: shared library calls or `impresario <read> --json`
                       └─ mutations: `impresario --workspace … --config … \
                                      --devices-config … <command>`
```

- **One mutation at a time.** A server-wide mutex serializes actions. Reads may
  run concurrently.
- **Curated allowlist.** Action names and parameters are fixed in the binary.
  The HTTP layer never accepts a command string, and a value beginning with
  `-` is rejected so an action cannot smuggle an extra flag into the child
  process.
- **Same environment as the TUI.** Children receive `--workspace`, `--config`,
  `--devices-config`, `--no-tui`, `--plain`, `--no-color` and
  `IMPRESARIO_NO_PATH_INSTALL=1`; on Windows they run with `CREATE_NO_WINDOW`.
- **Active workspace is mutable.** `workspace_use` and `workspace_new` switch
  the console's base only after the child exits 0. Relative paths in later
  requests resolve against the new base.
- **Action history.** The newest 100 executed actions are kept in memory and
  exposed at `/api/actions/history`; history is not persisted.
- **No secrets in responses.** Reads use the same redaction paths as the CLI
  and MCP server. Action responses contain the child's stdout/stderr after the
  CLI's own redaction.

## API

All responses use one envelope:

```json
{ "ok": true, "data": { } }
```

```json
{
  "ok": false,
  "error": {
    "code": "invalid_parameter",
    "message": "human-readable text",
    "details": { }
  }
}
```

`details` is optional and carries structured context (validator output,
`exit_code`, stdout/stderr).

### Reads

| Method | Path | Purpose |
|---|---|---|
| GET | `/healthz` | Liveness and product version |
| GET | `/api/meta` | Server, workspace, actions, docs, security mode |
| GET | `/api/schema` | This API: envelope, actions, endpoints, error codes |
| GET | `/api/guide` | Guided first-run flow with concrete action bodies |
| GET | `/api/overview` | Counts, validation, job summary, resident summary |
| GET | `/api/workspaces` | Active and remembered workspaces |
| GET | `/api/tests` | The `tests --json` tree |
| GET | `/api/tests/file?path=…` | One workspace DSL file |
| GET | `/api/tests/show?name=…` | Declaration source and references |
| GET | `/api/tests/grep?q=…&ignore_case=1` | Search workspace DSL files |
| GET | `/api/coverage` | Stage coverage and missing devices |
| GET | `/api/probe/support` | Plan-time probe support per device |
| GET | `/api/groups` | Effective actor groups |
| GET | `/api/devices` | Effective device definitions |
| GET | `/api/apps` | Effective app definitions |
| GET | `/api/config` | Effective settings, layers and schema |
| GET | `/api/doctor` | The doctor report |
| GET | `/api/resident` | Resident status, schedules, watchers, dumps |
| GET | `/api/resident/log?limit=…` | Resident service log tail |
| GET | `/api/dumps?watcher=…` | Watcher failure-dump bundles |
| GET | `/api/dumps/artifact?path=…` | One file under `reports/watch/` |
| GET | `/api/notifications?limit=…` | Recent resident notifications |
| GET | `/api/jobs?all=1` | Job records, newest first |
| GET | `/api/jobs/{id}` | One job record |
| GET | `/api/jobs/{id}/log?limit=…` | Job console log tail |
| GET | `/api/jobs/{id}/events?limit=…` | Structured execution events |
| GET | `/api/jobs/{id}/report` | The job's `report.json` |
| GET | `/api/jobs/{id}/artifacts` | Files in the job run directory |
| GET | `/api/jobs/{id}/artifact?path=…` | One run artifact |
| GET | `/api/runs?limit=…` | Report directories under `report_output` |
| GET | `/api/runs/{id}` | One run's `report.json` |
| GET | `/api/runs/{id}/artifacts` | Files in a report directory |
| GET | `/api/runs/{id}/artifact?path=…` | One report artifact |
| GET | `/api/docs` | Embedded documentation index |
| GET | `/api/docs/{slug}` | One document as JSON |
| GET | `/api/docs/{slug}/raw` | One document as Markdown |
| GET | `/api/actions/history` | Recent mutations in this console process |

Text artifacts are returned as `encoding: "text"` with a `text` field. Images
are returned as `encoding: "base64"` with a `mime` and base64 `text`. Other
binary files are base64 with `application/octet-stream`. Text is capped at
2 MiB; images at 8 MiB.

### Mutations

`POST /api/actions/{action}` with `Content-Type: application/json` and a JSON
object body. Unknown actions are `404 unknown_action`; invalid parameters are
`400 invalid_parameter`; spawn failures are `500 spawn_failed`. A child that
runs but exits non-zero is still `ok: true` and reports `passed: false`,
`exit_code`, `stdout` and `stderr` — the same contract as running the CLI.

Action groups:

- **Workspace:** `workspace_add`, `workspace_use`, `workspace_new`,
  `workspace_remove`.
- **Validation and runs:** `validate`, `run`, `probe`, `device_check`,
  `job_stop`, `job_remove`.
- **Definitions:** `group_add`, `group_set`, `group_remove`, `device_add`,
  `device_edit`, `device_remove`, `device_rename`, `app_add`, `app_edit`,
  `app_remove`, `app_rename`.
- **Settings:** `config_set`, `config_unset`.
- **Test files:** `test_save`, `test_new`, `test_rename`.
- **Resident:** `service_start`, `service_stop`, `service_restart`,
  `service_pause`, `service_resume`, `service_install`, `service_uninstall`,
  `schedule_add`, `schedule_remove`, `schedule_enable`, `schedule_disable`,
  `schedule_run_now`, `watcher_add`, `watcher_remove`, `watcher_enable`,
  `watcher_disable`, `watcher_start`, `watcher_stop`, `watcher_pause`,
  `watcher_resume`, `watcher_run_now`.

`GET /api/schema` is the authoritative list with every parameter name, type,
required flag and allowed value. Highlights:

```json
{ "kind": "showing", "device": "pixel", "claims": ["video_playing"] }
```

```json
{ "play": ["plays/Smoke.play"], "device": ["@phones"], "dry_run": true }
```

```json
{ "path": "plays/Smoke.play", "content": "Play: Smoke\n…" }
```

`run` always detaches unless `dry_run` is true. `probe` uses the CLI's
`probe --json` output, so the response carries a `probe` object with verdicts,
evidence, confidence, observed state and the run directory.

### Example

```bash
curl -s http://127.0.0.1:7810/api/overview

curl -s -X POST http://127.0.0.1:7810/api/actions/validate \
  -H 'Content-Type: application/json' \
  -d '{}'

curl -s -X POST http://127.0.0.1:7810/api/actions/run \
  -H 'Content-Type: application/json' \
  -d '{"dry_run":true}'
```

## Security model

- **Loopback by default.** The default bind is `127.0.0.1:7810`. Every request
  must carry a `Host` naming `localhost`, `127.0.0.1` or `::1` (any port),
  which blocks DNS-rebinding reads.
- **Remote mode is opt-in.** `--bind <non-loopback>` requires `--allow-remote`.
  In that mode any non-empty `Host` is accepted because the operator chose to
  expose the port. There is **no TLS and no authentication**; only use it on a
  trusted network.
- **CSRF protection.** A POST must be JSON and, when the browser sends an
  `Origin`, it must match the `Host` (default ports are normalised). Non-browser
  clients may omit `Origin`; the JSON content type and Host check still apply.
- **Path confinement.** File reads resolve through `canonicalize` and must stay
  under the workspace, a run directory, or `reports/watch/`. Writes must end in
  `.scene`, `.theatre` or `.play`, must not contain `..`, and their parent must
  resolve inside the workspace.
- **No arbitrary commands.** Only the curated action list exists. Parameter
  values are length-limited and may not begin with `-`.
- **Security headers.** Every response carries a strict CSP
  (`default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'
  data:; connect-src 'self'; base-uri 'none'; form-action 'none';
  frame-ancestors 'none'`), `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`,
  `Permissions-Policy`, `Cache-Control: no-store` and `Connection: close`.
- **Limits.** 64 concurrent connections (excess gets `503 busy`), 64 KiB of
  request headers, 2 MiB request bodies, and a 15-second read/write timeout.
  One request per connection; keep-alive and chunked bodies are not supported.

## For AI agents

An agent that can reach the port needs no browser:

1. `GET /api/schema` for the envelope, action catalogue, parameter types,
   endpoint list and error codes.
2. `GET /api/guide` for the recommended flow with ready-made action bodies.
3. `GET /api/overview` and `GET /api/tests` to see the workspace state.
4. `POST /api/actions/{action}` for mutations, then follow `job_id` through
   `GET /api/jobs/{id}`, `/log`, `/events`, `/report` and `/artifacts`.
5. `GET /api/docs/{slug}` for the same Markdown the MCP server serves at
   `impresario://docs/...`; `GET /api/docs/web` is this page.

The MCP server remains the preferred integration when the agent can launch a
stdio child process. The web API is the alternative when only HTTP is
available. Both expose the same workspace and the same CLI-backed operations.

## Limits and non-goals

- No authentication, TLS, multi-user sessions or remote job upload. The console
  is a local operator tool.
- No live push channel: screens refresh on navigation or the Refresh button.
  Jobs are followed by re-reading the log and event files.
- Action history is in memory and disappears with the process.
- The web console does not replace the TUI's full keyboard workflow, the LSP,
  or the MCP server; it is a browser view over the same CLI.
- Hardware evidence still requires reachable devices. The console reports
  `unverifiable` exactly as the CLI does and never turns missing evidence into
  a pass.
