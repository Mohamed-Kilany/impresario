# Schedules, watchers and resident delivery

`impresario` can keep running plays after the CLI or TUI exits. A per-user
resident service owns the clock, the device leases and the reports; the CLI,
TUI and MCP server are clients of the same persisted state.

```powershell
impresario service install        # logon autostart for this user
impresario service start          # detach the resident process
impresario service status --json
impresario service pause
impresario service resume
impresario service stop
impresario schedule list --json
impresario watch list --json
```

State, events and the service log live under `~/.impresario/service/`
(`state.json`, `events.jsonl`, `service.log`, with `service.log.1` after
rotation). `service status` reports the pid, pause state and persisted counters
even while the service is stopped. Restarting the runner in the same workspace
resumes those counters, so a restart neither repeats nor forgets a `once`
schedule; a different workspace starts with a clean state. A recurring
schedule's first fire is anchored when the runner first observes it. If the
schedule was configured while the service was off, the previous state's last
observation is the baseline, so an intervening fire still follows `on_missed`.
Editing a schedule's `when` block resets its runtime anchor.

## Configuration layers

Schedules, watchers and delivery settings are optional top-level sections of
the workspace config:

```yaml
schedules:
  - name: nightly
    when: { kind: daily, at: "02:30" }
    run: { plays: [smoke.play], tags: [smoke], actors: [phone] }
    on_missed: run_once
    on_busy: queue
    notify:
      on: [failure, recovery]
      channels: [toast, "webhook:ops"]

watchers:
  - name: soak
    run: { plays: [streaming.play], actors: [tv] }
    pacing: { mode: min_gap, gap: 30s }
    on_failure:
      dump: [report, screenshot, logs, device_state, evidence]
      throttle: { same_failure_window: 15m }
      notify: [toast, "webhook:ops"]
    device_health:
      treat_unreachable_as: infrastructure
      backoff: [10s, 30s, 2m, 10m]
    retention: { max_reports: 500, max_age: 14d, max_disk: 10GiB }

notifications:
  quiet_hours: { start: "22:00", end: "07:00" }
  rate_limits:
    toast: { max: 5, window: 1h }
    "webhook:ops": { max: 20, window: 1h }
  webhooks:
    ops: { url: "https://example.invalid/impresario" }
```

The same sections may live in `~/.impresario/config.yaml`. The global layer is
loaded first and the workspace layer overlays it by name; schedules, watchers
and webhooks are matched by name, while notification channel keys override the
same channel. A missing section is an empty layer. Mutating CLI commands edit
the workspace layer by default and accept `--global`.

Names are 1 to 64 ASCII letters, digits, `.`, `-` or `_`, and may not begin
with `.`. Duplicate names in the effective layer are rejected.

## Schedules

A schedule has a `when` block, a run selection, and optional policy:

| Field | Meaning |
|---|---|
| `when` | `once`, `daily`, `weekly`, `interval` or cron; see below |
| `run` | `plays`, `tags`, `exclude_tags`, `actors`, `app`, `retries`, `profile` |
| `on_missed` | `skip` (default) or `run_once` |
| `on_busy` | `queue` (default), `skip` or `fail` when a device lease is busy |
| `notify` | `on: [failure, unverifiable, device_unreachable, recovery]` and `channels` |
| `enabled` | defaults to `true` |

Wall-clock forms use the host's local timezone:

```yaml
when: { kind: once, at: "2026-11-06T09:00:00" }
when: { kind: daily, at: "02:30" }
when: { kind: weekly, days: [mon, fri], at: "07:30" }
when: { kind: interval, every: 1h }
when: { kind: cron, expr: "*/15 9-17 * * mon-fri" }
```

Cron accepts `*`, a number, `a-b`, comma lists and `/step`, with month and
weekday names. Intervals are measured from the previous fire. Schedule times
are host-local, including daylight-saving transitions: an ambiguous fall-back
time fires once at the first occurrence, and a spring-forward time that does
not exist is skipped. A past `once` is marked missed and disabled unless
`on_missed: run_once` asks for one catch-up run.

Each fire is a normal supervised run, so reports, logs and device leases are
unchanged; the job is labelled `schedule <name>` and appears in
`impresario jobs`. A schedule's `notify.on` decides which routed outcomes are
delivered: `failure`, `unverifiable`, `device_unreachable` and `recovery`; a
recovery after consecutive failures emits one recovery event.

## Watchers

A watcher repeats one run selection until stopped or paused:

| Field | Meaning |
|---|---|
| `run` | same selection fields as a schedule |
| `pacing` | `{ mode: back_to_back }` (default) or `{ mode: min_gap, gap: 30s }` |
| `lease` | `gap` (default) releases the devices between iterations; `no_gap` holds them for the watcher lifetime |
| `priority` | `normal` (default) or `low`; a low-priority watcher yields to queued same-device jobs |
| `on_failure.dump` | any of `report`, `screenshot`, `logs`, `device_state`, `evidence` |
| `on_failure.throttle.same_failure_window` | duplicate-failure window, default `15m` |
| `on_failure.notify` | `toast` and/or `webhook:NAME` channels |
| `device_health.treat_unreachable_as` | `infrastructure` (default) or `test`; `test` also keeps unreachable failures out of the device backoff |
| `device_health.backoff` | 1 to 16 positive spans, default `10s, 30s, 2m, 10m`; advances only on consecutive infrastructure failures |
| `retention` | `max_reports` (500), `max_age` (`14d`), `max_disk` (`10GiB`); covers failure dumps and the watcher's normal reports |
| `enabled` | defaults to `true` |

By default (`lease: gap`) the watcher holds a per-iteration device lease and
releases it between iterations, so another run can use the device while the
watcher is pacing. `lease: no_gap` is the explicit opposite: after planning the
first iteration the watcher takes the device lease and keeps it across
iterations, releasing it only on stop, pause, disable or when the mode changes
back to `gap`. No-gap mode can starve manual and scheduled runs, so it cannot
be combined with `priority: low`, and `watch list`, `watch status`,
`service status`, the MCP watcher list and the TUI Resident screen always show
the active lease mode. The layered config is re-read on each active iteration,
so edits to `pacing`, `run`, `device_health.backoff` and `on_failure.notify`
take effect without restarting the service; a change to `lease` takes effect on
the next iteration. A `priority: low` watcher checks for a live queued job on
the devices of its previous iteration and waits while one is present; a
normal-priority watcher never yields.
`device_health.backoff` follows infrastructure failures only: each consecutive
infrastructure-class iteration advances one step through the list, and a test
failure, an unverifiable iteration or a successful run resets the streak.
`treat_unreachable_as: test` reclassifies unreachable devices as test failures,
so they reset the streak instead of advancing it. The general consecutive
failure count in `watch status` still counts every failed or unverifiable
iteration, and a planning/profile error keeps its own conservative backoff.
`watch start|stop` edit the enabled flag and clear/write the per-watcher
control files; `watch pause|resume` write or clear only the pause marker, and
`watch run-now` runs the selection once without changing the loop. Each
iteration is a normal job labelled `watcher <name> · ...`.
Failure dumps are written under `reports/watch/<name>/<timestamp>/` and are
subject to the watcher's retention limits. The same limits cover the normal
run reports the watcher's iterations produce in `report_output`: the runner
keeps a per-workspace index under
`service/watchers/<workspace-hash>-<name>.reports.json`, seeded once from
watcher-labelled job records, and prunes dumps and indexed reports as one
newest-first pool at most once per minute. Reports from manual runs in the same
directory are never touched. Each `on_failure.dump` kind writes
only its own files: `report` copies `report.json`, `junit.xml` and
`report.html`; `screenshot` copies `screenshots/`; `logs`, `device_state` and
`evidence` write `logs.json`, `device_state.json` and `evidence.json` built
from the run report. `dump.json` records the requested kinds, and dumps that
land in the same millisecond get a `-1`, `-2`, ... suffix. Identical failure
signatures (device, play, act, stage, step and code, with example suffixes
ignored) inside `throttle.same_failure_window` produce one routed event and
then suppressed events; a recovered watcher emits one recovery event.

`watch status` reports the last start, last status, consecutive failures, the
next iteration time and the last dump, or run `watch status --json` for
scripting.

## Delivery

Delivery is best-effort and never fails a schedule or watcher. Every channel
attempt appends a `notify_delivery` event; delivery failures are recorded but
do not change the run result.

- `toast` uses the native OS notification path (`notify-rust`). Toast actions
  open the run report through the shared external opener where the platform
  exposes action callbacks; macOS delivery is asynchronous and action support
  is platform-dependent.
- `webhook:NAME` sends a JSON POST to the URL in `notifications.webhooks.NAME`,
  with an optional bearer token and a ten-second timeout. The token is read from
  `IMPRESARIO_WEBHOOK_<NAME>_SECRET` first, then from the OS keychain under
  service `impresario.notify`, account `webhook:<name>`. Secrets are never
  written to config, event payloads or logs.
- `quiet_hours` is a half-open host-local window `[start, end)` and may wrap
  midnight. A quiet-hour suppression records `notify_delivery` with
  `suppressed: quiet_hours`.
- `rate_limits` are independent moving windows per channel (`toast`,
  `webhook:NAME`) and persist in `service/state.json`, so a restart does not
  reset the count. A rate-limit suppression records
  `notify_delivery` with `suppressed: rate_limit`. Quiet hours take precedence
  when both apply.

Useful commands:

```powershell
impresario notify list --json --limit 20
impresario notify open <notification-id|job-id|report-path> --print
impresario notify secret set ops --stdin
impresario notify secret status ops
impresario notify secret clear ops
```

`notify secret set` requires `--stdin`, reads exactly one line and never prints
the secret. `notify open` checks the event log first, then the job record, and
opens the report (or prints its path with `--print`). `notify list` shows the
top-level routed `watch_notify` and `schedule_notify` events; delivery
suppressions appear as their own `notify_delivery` events.

## Heartbeat and version handshake

The runner writes `heartbeat_at` to `service/state.json` on every 500 ms loop
tick and stamps `runner_version` with the binary that wrote the file. Status
clients (`service status`, the MCP `impresario_service_status` tool and the TUI
Resident screen) report both:

- `heartbeat_stale` is true when a live runner's heartbeat (or, for a
  pre-handshake state file, `updated_at`) is older than 30 s. The runner keeps
  scheduling while the host is asleep; after wake the next tick refreshes the
  timestamp, so a stale report is transient unless the process is wedged.
- `restart_required` is true when a live runner reports a different
  `runner_version`. `service start` performs the handshake automatically: it
  asks the old runner to stop and starts the current binary, so upgrading the
  CLI never leaves an old runner scheduling jobs. `service restart` is the
  explicit form.

The state schema stays version 1: `heartbeat_at` and `runner_version` are
optional fields, and a missing heartbeat falls back to `updated_at`.

## Clients

The MCP server and the TUI Resident screen read the same
`service/state.json` and layered config. Status reads are side-effect free;
mutations go through the same lossless YAML helpers as the CLI:

- MCP: `impresario_service_status`, `impresario_schedules_list`,
  `impresario_schedule_add`, `impresario_schedule_remove`,
  `impresario_schedule_run_now`, `impresario_watchers_list`,
  `impresario_watcher_start`, `impresario_watcher_stop` and
  `impresario_watcher_dumps`. The `impresario://docs/scheduling` resource
  serves this document. Mutating tools are serialized with runs and
  config edits and never expose secrets.
- TUI: the **Resident** screen (read-only, refreshed every two seconds while
  open) shows service health, schedules with the next fire, watchers with live
  iteration status, and the last dump. Edits still go through the CLI or MCP.

See `docs/CLI.md` for the complete command list and `docs/MCP.md` for client
wiring and tool schemas.

## Current limits

The resident runner is implemented and covered by mock-driven tests. Physical
macOS/Linux autostart and toast action behavior remain hardware-unverified until
run on those hosts.
