# CLI and TUI

`impresario` is one binary with two front ends:

- a scriptable, non-interactive CLI for CI and humans who want flags;
- a full-screen TUI that is a **client of that same CLI**. Every command it
  runs is a real `impresario` subprocess, so exit codes, diagnostics, redaction
  and reporters have exactly one implementation (I-125).

Run `impresario <command> --help` for the generated flag reference. This page
documents the workflows, the job model and the interactive keys.

## Starting

```text
impresario                 # TUI when stdin and stdout are terminals, else the plain dashboard
impresario --no-tui        # plain dashboard, never the TUI
impresario --plain         # scriptable output, never the TUI
impresario doctor          # non-interactive diagnosis
```

The TUI starts only when **all** of these are true: stdin and stdout are
terminals, `CI` is unset/empty/`0`/`false`, and neither `--no-tui` nor
`--plain` was passed. Otherwise `impresario` prints the workspace summary and
the next commands, then exits 0 without reading stdin. A missing required
value is a usage error (exit 2), never a prompt in a non-interactive session.

On a normal `impresario` start (no subcommand) the binary also makes the
`impresario` name convenient from a fresh shell: it copies the current
executable to `~/.impresario/bin` and adds that directory to the **user**
`PATH` (Windows `HKCU\Environment`, broadcast to open windows; Unix the first
existing/created profile file). Subcommands (`doctor`, `lsp`, `mcp`, tests,
scripts) never rewrite the environment. The operation is idempotent and prints
a restart notice only when it changes `PATH`. It is skipped when
`IMPRESARIO_NO_PATH_INSTALL` is set, and when `IMPRESARIO_HOME` is set (tests
and portable homes must not mutate the real user environment).
The current process `PATH` is updated too, so a same-session `impresario` can
resolve.

`--workspace <DIR>` is a top-level option and must appear **before** the
subcommand (`impresario --workspace . run`). After a `config` subcommand the
same flag selects the config *scope* instead (I-100).

## Exit codes

| Code | Meaning |
|---|---|
| 0 | passed / command succeeded / detached job submitted |
| 1 | test failure |
| 2 | usage, configuration or DSL error (nothing executed when the plan cannot be built) |
| 3 | infrastructure failure (daemon, reporter, missing state) |
| 130 | interrupted / stopped |

`wait <id>` and `attach <id>` mirror the job's code; `attach` on an already
finished job prints the tail and returns that code.

## The everyday loop

```powershell
impresario workspace new . --example     # split config, plays/, scenes/, theatres/
impresario validate
impresario fmt --check
impresario tests                         # tree of scenes, theatres, plays, acts, stages, tags
impresario tests --tag smoke --platform android
impresario run --dry-run                 # act x device matrix, skipped acts and reasons
impresario run --device web1 --app shop  # attached, runs in this process
impresario run --detach --tag smoke      # job id, returns immediately
impresario jobs                          # active jobs (add --all)
impresario attach <id>                   # follow; Ctrl-C detaches, --stop stops
impresario logs <id> -f                  # console log; --events for events.jsonl
impresario wait <id>                     # blocking; exit code mirrors the job
impresario report <id> --json            # report after the job is terminal
impresario stop <id>                     # asks the job to stop at a command boundary
impresario rm <id>                       # remove a finished job's state and logs
```

**Selectors** are repeatable and combine as documented in `docs/DSL.md` /
`docs/CONFIG.md`: `--play` (file, directory, glob or bare name), `--act`,
`--tag`, `--exclude-tag`, `--device` (visible alias `--actor`; repeatable,
`all`, a configured device name, or `@group`), `--platform`, `--app`,
`--var K=V`, `--arg ACT.PARAM=VALUE`, `--timeout`, `--retries`,
`--unverifiable fail|warn`, `--reporter`, `--fail-fast`, `--only-failed`.
`--exclude-tag` wins over `--tag`. Inline platform filtering treats an act
stage with no explicit platform restriction as applicable (I-129).

`@group` references expand recursively from the top-level `groups:` map in
`impresario.devices.config`; unknown groups and cycles fail before any device
is leased. The play-level/act-level `Actors:` list and `Perform For/Except`
use the same expansion. Selecting a group does not force every member to run:
each play still intersects the selected devices with its effective actors.

`--unverifiable` overrides `unverifiable_policy` for one run. Probes (`Check:
Showing`, `Check: Player`) are tri-state: a claim that cannot reach the required
confidence is `unverifiable`. The default `fail` makes the step fail; `warn`
records the status and keeps the run green. Executor-supplied verdicts are
recomputed by the master. See `docs/PROBES.md` for the DSL, evidence sources,
fallback rungs and the in-app SDK contract.

`--dry-run` (also `/plan` in the TUI) resolves the plan without touching a
device and prints the act × device matrix, parameter bindings (secrets masked)
and every skipped act with its reason (platform filter, unsupported action,
selection).

## Jobs and the background supervisor

A run is a job with an id shaped `YYYYMMDD-HHMMSS-xxxx`, whether it runs
attached or detached.

- **Attached** (`impresario run`): the job executes in the CLI process and
  writes the same record and logs as a detached one. Ctrl-C cancels the run
  (exit 130); a second Ctrl-C exits immediately. The job is never silently
  left in `running`.
- **Detached** (`run --detach`, and every `/run` in the TUI): the job is
  submitted to a small per-user daemon over a local socket (`interprocess`;
  named pipe on Windows, Unix socket elsewhere). The CLI prints the id,
  `attach:` and `logs:` lines and exits 0 immediately.
- **Daemon**: auto-started on demand by `run --detach`; `impresario daemon`
  runs it explicitly. It idles out after 30 s with no jobs or clients
  (`--idle-seconds`, `0` = never). It owns job execution, not devices, so
  closing the terminal cannot kill a detached job.
- **Recovery**: on the next `jobs`/daemon start, a job whose owner pid is gone
  is marked `interrupted`, never left `running`. Leases and wait entries owned
  by dead pids are cleaned up the same way.

State lives under the global home (`~/.impresario` or `IMPRESARIO_HOME`):

```text
jobs/<job-id>/job.json        status, devices, plays, pid, queue position, exit code
jobs/<job-id>/events.jsonl    machine events (the same model as the reporters)
jobs/<job-id>/console.log     human console output (attach/logs print this)
jobs/<job-id>/report.json     report copy for detached jobs
jobs/leases/<device>.json     one atomic lease per device (pid-owned)
jobs/leases/wait.<job>        queued waiter (pid + device list)
jobs/daemon.token             local-socket authentication token
```

The report itself is also written to the configured `report_output`
(`./reports/<id>/`) by the usual reporters, so `report.json`, JUnit, HTML and
screenshots are identical for attached and detached runs.

### Device leases and `--on-busy`

A device is leased by at most one job at a time (native agents are
single-session). The lease is a file, not daemon memory, so it also protects an
attached run against a detached one. `--on-busy` chooses the policy:

| Value | Behaviour |
|---|---|
| `wait` (default) | Block and show the queue position; proceeds when the lease is free. |
| `fail` | Refuse before touching a device. |
| `queue` | Detached: create a `queued` job and return immediately. Attached: wait, like `wait`. |

Any number of jobs can run concurrently on different devices. Within one job,
devices run in parallel up to `parallel_devices`; fail-fast, retries and
per-device reporters are unchanged.

## Schedules, watchers and the resident runner

The resident runner is a per-user background process, so closing the CLI or the
TUI never stops scheduled work. It is a client of the installed executable and
does not shell out to platform tools.

```powershell
impresario service install        # logon autostart for this user only
impresario service start          # detach the runner for this workspace
impresario service status --json
impresario service pause
impresario service resume
impresario service logs -f
impresario service stop
impresario service uninstall --purge   # --purge also deletes state/logs
```

`service run` is the foreground loop used by the hooks. State, events and the
log live under `~/.impresario/service/` (`state.json`, `events.jsonl`,
`service.log`, rotated to `service.log.1` at 10 MiB). `service install` writes a
Windows Startup `.cmd`, a macOS LaunchAgent plist or a Linux `systemd --user`
unit; `uninstall` removes only that hook unless `--purge` is given. `service
status` reports the pid, pause state and per-schedule/per-watcher counters even
when the runner is stopped. Restarting the runner in the same workspace resumes
those counters and the missed-fire baseline; a different workspace starts clean.
A recurring schedule is anchored when the runner first observes it (or at the
last persisted observation when it was configured while the service was off),
and editing its `when` block resets the anchor.

Schedules and watchers are optional `schedules:`/`watchers:` sections in the
workspace config. Global `config.yaml` sections with the same names are merged
by name and the workspace wins. Mutating commands target the workspace layer by
default; `--global` targets `~/.impresario/config.yaml`. The CLI edits YAML
losslessly (comments and untouched bytes survive) and re-parses the complete
prospective file before writing, so a bad edit cannot leave the workspace
unparsable.

```powershell
impresario schedule list [--json]
impresario schedule add nightly --when "kind: daily, at: '02:30'" --play smoke.play --tag smoke
impresario schedule next nightly --count 5
impresario schedule edit nightly --file nightly.yaml
impresario schedule disable nightly     # enable, rm, show, run-now
impresario watch add soak --play smoke.play --gap 30s
impresario watch add hold --play smoke.play --lease no-gap
impresario watch list|show|status [--json]|dumps
impresario watch start|stop|pause|resume soak
impresario watch run-now soak
impresario notify list [--json] [--limit 20]
impresario notify open <notification-id|job-id|report-path> [--print]
impresario notify secret set ops --stdin
impresario notify secret status ops
impresario notify secret clear ops
```

`add` and `edit` also accept full YAML with `--yaml` or `--file`; `--when` is
the short one-line `when` mapping. Names are 1..=64 ASCII letters, digits, `.`,
`-` or `_` and may not start with `.`.

Wall-clock schedules use host-local time. `once`, `daily`, `weekly`, cron and
`interval` are supported; cron accepts `*`, `n`, `a-b`, comma lists and `/step`
with month/weekday names. Fall-back times fire once at the first occurrence,
spring-forward times that do not exist are skipped, and a past `once` runs when
the service observed its time or `on_missed: run_once`, otherwise it is marked
missed and disabled. A schedule fire is a normal supervised job, so reports,
logs and device leases are unchanged; it is labelled `schedule <name>` and
appears in `impresario jobs`. `on_busy` is `queue` (retry while the previous
occurrence is still running), `skip` or `fail`.

By default (`lease: gap`) watchers take a per-iteration device lease and
release it between iterations; each iteration is a normal job labelled
`watcher <name>`. `--lease no-gap` (or `lease: no_gap` in YAML) holds the lease
for the watcher lifetime instead, releasing it on stop, pause, disable or a
switch back to `gap`; it cannot be combined with `priority: low`, and
`watch list`, `watch status`, `service status`, the MCP watcher list and the TUI
Resident screen show the mode. The layered config is re-read each active
iteration, so `pacing`, `run`, `device_health.backoff` and `on_failure.notify`
edits apply without a restart (a `lease` change applies on the next iteration).
A `priority: low` watcher waits between iterations while another live job is
queued for its devices.
`pacing.mode` is `back_to_back` or `min_gap` with `gap`. `on_failure` configures
the dump, throttle and notification channels. Notification events are routed
per watcher and schedule: identical watcher failures (same
device/play/act/stage/step/code signature, with example suffixes ignored)
inside `throttle.same_failure_window` produce one `watch_notify` event and
then `watch_notify_suppressed` events carrying an increasing count; a recovered
watcher emits one `watch_notify` recovery event, and schedules emit
`schedule_notify` only for the events listed in `notify.on`, including recovery
after consecutive failures.

Delivery is configured once in the optional `notifications:` section of the
workspace config or the global `config.yaml`; the two layers merge by webhook
name and channel key:

```yaml
notifications:
  quiet_hours: { start: "22:00", end: "07:00" }
  rate_limits:
    toast: { max: 5, window: 1h }
    "webhook:ops": { max: 20, window: 1h }
  webhooks:
    ops: { url: "https://example.invalid/impresario" }
```

Quiet hours use host-local time and may wrap midnight; the window is
half-open (`start <= now < end`). Rate limits are independent moving windows
per channel and persist across restarts in `service/state.json`. A suppressed
delivery still records a `notify_delivery` event with a reason of
`quiet_hours` or `rate_limit`; duplicate-failure `watch_notify_suppressed`
events are routing decisions and are not delivered at all. Toast delivery uses
the native Windows notification, macOS notification and Linux freedesktop
services; the toast's `Open report` action opens the run report when a report
path exists and the platform supports actions.

Webhook channels POST the JSON payload (`id`, `kind`, `source`, `event`,
`status`, `signature`, `count`, `job`, `report`, `dump`, `exit_code`, `at`,
`title`, `body`) with a ten-second timeout. A webhook bearer secret is read
from the OS credential store under service `impresario.notify`, account
`webhook:NAME`; `IMPRESARIO_WEBHOOK_<NAME>_SECRET` overrides it for headless
sessions. Secrets
are never written to YAML, events or logs. `notify list` shows recent
watch/schedule notification events, `notify open` resolves an event id or job
id through the event log and job record (`--print` only prints the path), and
the `notify secret` subcommands manage the stored bearer token; `set` reads
exactly one line from stdin and never accepts the value as an argument. Any
delivery error is recorded in `notify_delivery` and does not stop the watcher
or schedule.

Dumps are filtered by `on_failure.dump` under
`reports/watch/<name>/<timestamp>/`: `report` copies `report.json`, `junit.xml`
and `report.html`; `screenshot` copies `screenshots/`; `logs`, `device_state`
and `evidence` write `logs.json`, `device_state.json` and `evidence.json`.
`dump.json` records the request, and same-millisecond dumps get a numeric
suffix. The resident MCP tools
(`service_status`, `schedules_list|add|remove|run_now`,
`watchers_list|start|stop|dumps`) expose the same state to agents, and the TUI
**Resident** screen shows the same joined view read-only (refresh every two
seconds; `j`/`k` scroll, `g`/`G` jump). No-gap lifetime leases (D-39) are shown
in every watcher view (see `docs/STATUS.md` gap 21 and `docs/SCHEDULING.md`).

## Packaging and signing

Packaging and signing are local, deterministic and pure Rust (D-19). They do
not call `ares-*`, `tizen`, `openssl`, `ar`, `tar` or `gzip`.

```powershell
# deterministic webOS IPK
impresario package webos .\build\web -o .\build\shop.ipk
impresario package inspect .\build\shop.ipk --json

# deterministic unsigned Tizen WGT
impresario package tizen .\build\web -o .\build\shop.wgt

# create a Samsung-issued TV profile without Tizen Studio; sign in through the
# browser when prompted. The DUID is printed by `impresario device check`.
impresario sign tizen profile create --name living-room --duid 1234567890

# import an existing author + distributor profile (passwords from env/keychain,
# never from the command line)
$env:IMPRESARIO_TIZEN_AUTHOR_PASSWORD = '...'
$env:IMPRESARIO_TIZEN_DISTRIBUTOR_PASSWORD = '...'
impresario sign tizen profile add living-room `
  --author .\keys\author.p12 --distributor .\keys\distributor.p12

impresario sign tizen profile list
impresario sign tizen profile show living-room --json
impresario sign tizen profile remove living-room

# re-sign for one TV and validate the result
impresario sign tizen package .\build\shop.wgt `
  --profile living-room --duid 1234567890 -o .\build\shop-signed.wgt
impresario sign tizen check .\build\shop-signed.wgt
```

`profile create` discovers Samsung's current certificate endpoints, opens the
normal Samsung account login, and generates the author/distributor profile
natively. It uses the browser callback on `localhost:4794`; close Tizen Studio's
Certificate Manager if that port is in use. `--no-browser`, `--refresh`,
`--extension-url` and `--timeout` control the flow; see `docs/SIGNING.md`.
`profile add` remains the route for an existing PKCS#12 pair.

`package inspect` accepts `.ipk` and `.wgt`; `--json` is the stable machine
shape. Errors are `IMP-SIGN-*` (see `docs/ERRORS.md`), and the full pipeline,
paths and RSA/ECDSA details are in `docs/SIGNING.md`.

During a run, a Tizen device resolves its own DUID and uses the signing profile
selected by `IMPRESARIO_TIZEN_PROFILE` (default `samsung`); Tizen 7.0+ signs
automatically, older Tizen installs the original package.
A webOS app directory or `.ipk` may be named in `platforms.webos.package`;
a directory is packaged into the cache, while an `.ipk` is validated first.

`impresario device check` and `impresario doctor` use the native TV probes:
webOS connects over SSH and queries deviceInfo/listApps; Tizen reads SDB
capabilities, DUID and app list. Both are read-only (no install, launch or
sign) and print warnings for luna/SDB/profile problems; see `docs/DEVICES.md`
§1.3/§1.4 for setup and the deferred hardware runbooks.

### webOS passphrase (global)

The six-character LG Developer Mode passphrase is saved once per device name in
the OS credential store (`impresario.webos`), not in any workspace file:

```powershell
impresario device passphrase living_room_tv          # hidden prompt
impresario device passphrase living_room_tv --status # saved / not saved
impresario device passphrase living_room_tv --clear
Get-Content .\passphrase.txt | impresario device passphrase living_room_tv --stdin
```

`IMPRESARIO_WEBOS_PASSPHRASE` overrides the saved entry for one process. The
passphrase is never printed or written to YAML; only the OS credential store
holds it. The native stores are Windows Credential Manager, macOS Keychain and
Linux Secret Service (GNOME Keyring/KWallet); a headless Linux host without a
Secret Service fails explicitly, so use the environment override there.

## Apple ID account (global, sign in once)

The Apple ID used for free provisioning is machine-global, not per workspace.
The command is dispatched before workspace/config resolution, so it works from
an empty directory:

```powershell
impresario account login                 # alias: impresario account wizard
impresario account status                # offline view of the stored session
impresario account status --online       # validate/refresh the developer token
impresario account logout
```

`login` asks for the email and password interactively (`rpassword`; the
password is wiped with `zeroize` and never written or logged). It performs
GrandSlam authentication, then supports either a trusted-device code or an SMS
code sent to a trusted number; `s`, `r`, `b` and `q` switch to SMS, resend, go
back or cancel. If the account has several Apple developer teams, choose one
with `--team <id>` or at the prompt.

The session lives in `~/.impresario/apple/session.bin` and the anisette state
in `anisette.bin`, both AES-256-GCM encrypted with a random 96-bit nonce. The
32-byte key is in the OS credential store under service `impresario.apple`,
user `session-key`. Apple's short-lived developer token is refreshed from the
stored `spd` + anisette state with `status --online`; re-login is only needed if
Apple invalidates the underlying session. `logout` removes both encrypted
files and the key.

The flow and store are covered by offline unit tests; a live Apple login and
2FA were not exercised yet (I-151), so treat the wizard as implemented and
tested, not Apple-verified.

## Impresario Agent for Apple (global setup)

`impresario apple` is also dispatched before workspace resolution. Pairing
records, signing sets, unsigned product sets and personalized DDIs are
machine-global under `~/.impresario/apple/`; a workspace only names the Apple
TV and the app under test.

```powershell
impresario apple status

# once per device (or import a pymobiledevice3 pair plist)
impresario apple pair --host <apple-tv-host>
impresario apple import-pair .\remote.plist --save <udid>

# scripted pairing: start the command, then send the code shown by the TV
impresario apple pair --host <apple-tv-host> --code-stdin

# once per signing identity / Xcode build
impresario apple signing import .\signing-source --name default
impresario apple signing verify --name default --products current --output .\signed-check
impresario apple signing status --name default

impresario apple products import .\build-tvos\Build\Products\Debug-appletvos --name current
impresario apple products status --name current
impresario apple ddi import .\ddi-source --build <build>
impresario apple ddi status

# live check and a manual agent session
impresario apple doctor --host <apple-tv-host> --signing default --products current
impresario apple session --host <apple-tv-host> --udid <udid> --signing default --products current

# recover a wedged XCUITest/testmanagerd session (reboots the TV)
impresario apple restart --host <apple-tv-host> --udid <udid>
```

`--code-stdin` opens the host connection first and reads the code only when the
TV asks for it. Put the Apple TV on Settings > Remotes and Devices > Remote App
and Devices, start the command, then enter the code the TV shows after the host
connects. The TV presents the host as `impresario`. Manual pairing performs the
RPPairing handshake and a pair-verify attempt before pair-setup.

A signing source contains `identity.p12` and one directory per bundle with
`profile.mobileprovision` + `entitlements.plist`. The PKCS#12 password is read
interactively and stored in the OS credential store. For a headless import
only, `IMPRESARIO_APPLE_P12_PASSWORD` is read and not persisted;
`signing clear-password` removes a stored password. There is no plaintext
password file and no password flag.

The product set is imported from Xcode's `Debug-appletvos`/`Debug-iphoneos`
output; exactly one host app, one stub and one `-Runner.app` are required.
The imported host, stub and nested test bundle ids are used to sign and launch
the agent, so a renamed build needs no Rust changes. The DDI is personalized
externally for the OS build reported by RSD; Impresario does not download it.

At runtime `AppleTransport` reads these machine-global stores:

| Variable | Default | Meaning |
|---|---|---|
| `IMPRESARIO_APPLE_SIGNING` | `default` | signing set name |
| `IMPRESARIO_APPLE_PRODUCTS` | `current` | product set name |
| `IMPRESARIO_APPLE_DDI` | detected OS build | explicit DDI directory |
| `IMPRESARIO_APPLE_START_TIMEOUT_SECS` | `120` | wait for the agent port |
| `IMPRESARIO_APPLE_HOST_ID` | host-derived UUID | RemotePairing host id override |

A device needs only its `ip`. Pairing is looked up by host, the UDID is learned
from the RSD tunnel, and the agent port is allocated dynamically. The same port
is passed to the UI test as `IMPRESARIO_AGENT_PORT`. The transport installs/signs
over AFC and
InstallationProxy and starts the XCUITest bundle over the direct outer-RSD
`testmanagerd`/`dtservicehub` path; it does not spawn `xcodebuild`, `iproxy`,
`simctl`, `devicectl` or `ideviceinstaller`. Simulator UDIDs are rejected and
a USB-only iPhone is not supported yet. See `docs/DEVICES.md` §3.3 and
`agents/apple/README.md`; the physical-tvOS path is proven (I-150/I-152), the
short-id host/no-stub path and the full short-id 105-step stub play are
proven (I-156).

## Profiles

Profiles are saved run selections under the top-level `profiles:` mapping of
the **global** `config.yaml` (`~/.impresario/config.yaml`), so they are
available in every workspace. They store the run flags, not secrets.

```powershell
impresario profile save smoke --tag smoke --device web1 --device pocket_phone
impresario profile list
impresario profile show smoke
impresario profile rm smoke                 # `delete` is the long spelling
impresario run --profile smoke              # explicit flags still win
```

Saving/replacing uses a targeted text splice of the `profiles:` block: the
candidate is parsed and the whole file is re-read after the edit, so unrelated
comments, ordering and CRLF endings survive. `yaml-edit` 0.3.2 mis-indents a
block mapping inserted under an existing block key, which is why this one path
does not use it (I-127); all other settings edits still use `yaml-edit`.

## Test explorer and file operations

`impresario tests` lists scenes, theatres, plays, acts, stages, tags,
platforms, parameters, files and references. It accepts the same `--play`,
`--act`, `--tag`, `--exclude-tag` and `--platform` filters; `--json` is the
stable machine shape consumed by the TUI.

```powershell
impresario tests list
impresario tests new scene LoginScreen
impresario tests new theatre Auth
impresario tests new play Smoke
impresario tests show click
impresario tests where click
impresario tests grep "Check:"
impresario tests open Smoke
impresario tests rename act Pass Passes --dry-run
```

`new` writes the canonical template and refuses to overwrite without
`--force`; `rename` updates references at identifier boundaries, supports
`--dry-run`, and refuses collisions. `open` uses `$VISUAL`, then `$EDITOR`,
then `code` (the same helper as `config edit`).

## TUI

### Landing, menu and keys

With no arguments, `impresario` opens on a landing screen, not directly in a
workspace. The colorful five-row `IMPRESARIO` wordmark sits above the
remembered-workspace list. The current workspace is first and marked
`current`; a remembered path that no longer exists is marked `missing`.
The details panel shows the selected path, whether its config exists and
whether it is the active workspace. The workspace menu shows the test, device,
app and run counts. Below the workspace rows are `Add current workspace`,
`Doctor` and `Quit`.

| Key | Action |
|---|---|
| Up/Down, `j`/`k` | Move |
| `Enter` | Open the selected workspace or menu row |
| `Esc` | Back one level (overlay/completion/filter first) |
| `Tab` | Focus the prompt on a workspace screen |
| `?` or F1 | Help overlay (keys, slash commands, examples) |
| Ctrl-Q, or Ctrl-C twice within 1.2 s | Quit (terminal restored) |
| Ctrl-Z | Suspend (Unix); terminal is restored and re-entered |

On the landing screen: `a` adds the current workspace, `d` runs doctor and `q`
quits. `Enter` on a missing workspace reports it in the status line instead of
trying to enter it. Entering a workspace opens the workspace menu:
`Overview`, `Runs`, `Tests`, `Devices`, `Apps`, `Config`, `Doctor`,
`Switch workspace` and `Quit`. `Esc` from menu content returns to the landing
screen; `Esc` from the menu prompt returns to menu content. `/menu`,
`/workspaces` and `/landing` jump directly.

The first-run welcome overlay appears after the first workspace is entered,
not on the landing screen. The Overview/Home screen shows the workspace,
config, devices and apps paths; device/app, scene/theatre/play and run counts;
and active/recent jobs. The global home appears in the first-run overlay. The TUI
deliberately does not probe device reachability or run `doctor` in the
background: use `/doctor` or `impresario device check` when live state
matters. In-app completion notices appear when a followed job reaches a
terminal state.

Prompt editing: history with Up/Down, Ctrl-R reverse search, Tab completion
(device/app/play/act/stage/tag names and slash-command flags), Shift-Tab for
the previous completion, Ctrl-A/E/U/K/W, Alt+Enter or Ctrl-J for a newline,
and bracketed paste. History is `~/.impresario/history` (permission 0600 on
Unix, capped by `ui.history_size`).

### Screens and the run view

**Run view** (from the menu, `/runs`, `/jobs`, `/run` or `/attach` which
submit/follow a job): left pane = jobs, right pane = the followed
job's device → play → act → stage → step tree, current command and rolling log.
The detail pane shows status, duration, devices, plays, message and the
failure screenshot path.

| Key (content focus) | Action |
|---|---|
| `j`/`k`, Up/Down, `g`/`G` | Move / first / last |
| `Enter` | Follow the selected job (or open the selected test/file) |
| `b` or `d` | Send the followed job to the background (clears the follow view) |
| `x` or `s` | Stop the selected job at the next command boundary |
| `f` | Pause/resume the log tail |
| `t` | Cycle the device filter (`Tab` focuses the prompt) |
| `/` | Filter the run list (Enter applies, Esc cancels) |
| `r` / `F` | Rerun the selection / rerun failed acts from its report |
| `o` / `p` | Open the report / the failure screenshot |
| `X` or Delete | Remove a finished job's state and logs |
| `?` | Help overlay |

**Tests screen** (menu → Tests, or `/tests`): `j`/`k` move, `g`/`G` first/last, Space marks,
`Enter` opens the file in the editor, `r` runs the selected/marked play or act
(detached), `d` prints the dry-run matrix for the selection. `Tab` returns to
the prompt.

**Devices/Apps/Config screens** (menu → the section, or `/devices`, `/apps`,
`/config`): `j`/`k` scroll, `Enter` opens the
relevant file in `$VISUAL`/`$EDITOR`, `r` reloads from disk. The Config screen
shows the effective setting, its value and its winning layer, plus the typed
`impresario.config` values. These screens are viewers: add/edit/remove is the
matching CLI command typed at the prompt (or `device wizard`), not a dedicated
in-TUI form (recorded as a partial in `docs/STATUS.md` gap 19).

### Slash commands

| Command | Effect |
|---|---|
| `/run [flags]` | Submit a detached job; `/run --dry-run` prints the plan instead |
| `/jobs`, `/runs` | Open the runs screen (same store as `impresario jobs`) |
| `/menu` | Open the workspace menu |
| `/workspaces`, `/landing` | Return to the landing/workspace picker |
| `/devices`, `/apps`, `/config` | Open those screens |
| `/tests [args]` | Open the explorer; with args runs `impresario tests ...` |
| `/open <path>` | Open a file/directory with the OS handler |
| `/doctor [--json]` | Run `impresario doctor` and print the result |
| `/fmt [args]`, `/validate [args]` | Run the matching CLI command |
| `/plan [flags]` | Run `impresario run --dry-run` |
| `/attach <id>` | Follow a job inside the TUI (does not spawn a terminal) |
| `/logs <id>`, `/report <id>`, `/stop <id>`, `/rm <id>` | Run the matching CLI command |
| `/profile [args]` | Run `impresario profile ...` |
| `/history`, `/clear` | Show prompt history, clear the activity panel |
| `/home` | Open the Overview screen |
| `/help`, `/?` | Help overlay |
| `/quit`, `/q` | Quit (same as Ctrl-Q) |

A plain line without `/` is run as a CLI command (`run --tag smoke` is the
same as `/run --tag smoke`). `wait`, `lsp`, `mcp`, `mock-agent` and `daemon`
are deliberately refused in the TUI with a pointer to a terminal; they block
or own stdio. An unknown slash command reports the error in the activity line
instead of doing nothing.

`/run` always adds `--detach` (unless `--dry-run`), so every TUI run survives
the TUI. The TUI spawns the CLI with `--workspace`, `--config`,
`--devices-config`, `--no-tui --plain --no-color` and captures its output into
the activity line; on Windows the child is created with `CREATE_NO_WINDOW`.

### First run

The landing screen always comes first. After a workspace without
`impresario.config` is entered (for example the current directory listed as
`current`), the TUI opens an overlay:

```text
Welcome to impresario
No workspace config was found yet. Create the workspace skeleton now?
  [y] create impresario.config, plays/, scenes/, theatres/ and an example play
  [n] or Esc skip; you can create it later with:
      impresario workspace new . --example
```

`y` runs `impresario workspace new <base> --example`; `n`/Esc only dismisses
the overlay. The generated example includes one playable act (`Check: Sample`),
so a fresh workspace immediately validates and can run against a mock. The
first-run overlay is not shown again while that workspace remains open.

### Persistence and crash safety

- `~/.impresario/state.json`: last screen, selected job id, selected test,
  last run tokens, log-follow flag, run filter and device filter.
- `~/.impresario/history`: prompt history (load/save on each accepted line).
- A panic hook and a `Drop` guard restore the alternate screen, bracketed
  paste and cursor; `Ctrl-Z` leaves and re-enters the terminal deliberately.
- The TUI never owns a job. Quitting, crashing or closing the terminal only
  ends the viewer; the daemon and the detached jobs keep running, and
  `/attach`, `impresario attach`, `jobs` and `report` still find them.

## Acceptance tests

The interactive surface is covered automatically on Windows:

- `crates/impresario-master/tests/tui_pty.rs` drives the **real binary**
  through ConPTY: landing → workspace menu → first-run scaffold (`Enter`, `y`
  → files on disk → quit), landing → menu → slash completion + help overlay,
  and run view (`MockAgent`, `b` detach → quit → `attach` → `report --json`).
  The harness answers ConPTY's startup cursor query (`ESC[6n` with
  `ESC[1;1R`), keeps the PTY master alive, resolves ANSI cursor
  movement/erases in a small screen model and asserts visible text.
- `crates/impresario-master/src/tui/snapshots.rs` renders eleven frames
  through ratatui `TestBackend` + `insta`: landing wide/narrow, workspace
  menu, home wide/narrow, tests explorer wide/narrow, help/filter overlay,
  first-run overlay, followed run view and a small terminal (the "too small"
  path).
- `tests/jobs_cli.rs` covers the daemon and leases: detach returns before the
  daemon exits, a second caller on a live lease follows `--on-busy
  wait|fail|queue`, a stopped job exits 130, recovery marks orphaned jobs
  `interrupted`, and a detached submission releases the caller's stdout
  pipes (Windows regression test).
- Live acceptance: the detached four-device run
  `20261021-080459-ec51` passed 4 devices / 4 plays / 6 acts / 189 steps
  (local Chrome, Android phone, Android TV, LG webOS TV), was attached after
  detaching and produced `report.json` with status `passed`. The LG-only
  diagnosis run `20261021-080418-843e` passed 2/2; the connector fixture
  suite passed 2/2 after `elements()` was fixed to include the collection
  root. The earlier single-web run `20261021-073144-2763` (9/9) remains the
  focused managed-Chromium check.

## Manual checklist: the three Windows shells

Run this on a Windows machine with an interactive desktop. The TUI needs a
real terminal; the non-TUI rows can also be run in CI. Record the shell and
any visual difference (colours, box drawing, cursor) rather than assuming
they match.

Legend: `[x]` = performed on this host; `[ ]` = still to do manually.

### Common preparation (all shells)

```powershell
cargo build
$env:IMPRESARIO_HOME = "$env:TEMP\impresario-shell-check"   # PowerShell
# cmd: set IMPRESARIO_HOME=%TEMP%\impresario-shell-check
impresario workspace new "$env:TEMP\impresario-shell-ws" --example
impresario --workspace "$env:TEMP\impresario-shell-ws" validate
```

Expected: `workspace new` prints every created path; `validate` exits 0 with
0 errors. `impresario --version` and `impresario --help` must fit the window
and exit 0.

### 1. Windows Terminal + PowerShell 7

- [x] Start Windows Terminal, open a PowerShell tab, `cd` to the workspace.
- [x] `impresario` starts on the landing screen with the colorful `IMPRESARIO`
      wordmark and remembered workspaces; no shell prompt is left in the
      scrollback.
- [x] Enter opens the workspace menu; Esc returns to landing; `?`/F1 opens
      help and Esc closes it.
- [x] Type `/he`, see `/help` in the completion popup, press Tab, Enter; the
      help overlay says `Help · Esc closes`.
- [x] Ctrl-R, type `run`, Enter recalls the line; Up/Down walks history.
- [x] `/run --dry-run` shows the matrix overlay; a real `/run` shows
      `Run · running`, `b` detaches with `Detached ... continues in the
      background`, and `/attach <id>` follows it again after completion.
- [x] Resize the window while following a job: content reflows, no panic, no
      duplicated frame. Ctrl-Q restores the original screen and the shell
      prompt is usable.
- [x] `impresario --no-tui` and `impresario --plain` print the plain
      dashboard; `impresario --no-color` removes colour but keeps the layout.
- [x] `impresario completions powershell | Out-String | Invoke-Expression` and
      Tab-complete `impresario ru` → `run`.

Expected: the same layout at 120×30 and at 80×24; box drawing is unbroken;
the status area shows keys, the selected job and the last activity message.

### 2. Windows PowerShell 5.1

- [x] Start `powershell.exe` (not `pwsh`) from the Start menu or `Win+R`.
- [x] Landing, workspace menu, help, `/he` + Tab, Ctrl-R, `/run`, `b`,
      `/attach`, resize and quit were checked; the current landing/menu/no
      colour frames were rerun in Windows Terminal, while the earlier
      conhost pass verified the console host's cursor restoration.
- [x] `--no-color` shows the landing and workspace menu monochrome with the
      same box drawing, spacing and navigation. The session exited 0 and the
      prompt was usable.
- [x] `impresario --plain | more` and the JSON row below do not hang and exit
      0.
- [x] JSON caveat: on PowerShell 5.1 a parenthesised assignment is required
      because `ConvertFrom-Json` emits the array as one pipeline object:
      ```powershell
      $jobs = impresario jobs --all --json | ConvertFrom-Json
      $jobs | Select-Object -First 2 id,status
      # or:
      (impresario jobs --all --json | ConvertFrom-Json) |
        Select-Object -First 2 id,status
      ```
      Verified with seven jobs; the two-row table printed ids and statuses.
      The bare `... | ConvertFrom-Json | Select-Object id,status` form prints
      a blank row on 5.1, so the checklist no longer uses it.
- [x] Run `impresario` with stdin redirected (`echo. | impresario`): it
      prints the plain dashboard, not the TUI, and does not read the pipe.

Known conhost detail: if the window is narrower than 40 columns or shorter
than 12 rows, the TUI prints `terminal too small` and asks to resize or use
`--plain`. It must not draw a corrupted frame.

### 3. cmd.exe (conhost or Windows Terminal tab)

- [x] `cmd /k` (or a Command Prompt tab), `cd` to the workspace.
- [x] `impresario` starts the landing screen; Enter opens the menu; `?` help,
      `/he`+Tab completion, Ctrl-R, `/run --dry-run`, a real `/run`, `b`,
      `/attach <id>`, resize and Ctrl-Q all behave as above.
- [x] `set CI=1` then `impresario`: plain dashboard, exit 0, no TUI.
- [x] `set CI=` then `impresario --plain`: plain dashboard, exit 0.
- [x] `impresario run --dry-run` and `impresario jobs --json` work; JSON is
      parseable (for example `impresario jobs --json > jobs.json`; also parsed
      by PowerShell 5.1 above).
- [x] `impresario completions cmd` exits 2 with the
      PowerShell/bash/zsh/fish/elvish list, not a panic.

Expected: command history, pasted multi-line text and box-drawing characters
are intact; no “not a terminal” error when the TUI is expected.

Screenshots and logs from the 2026-10-22 pass are under
`%TEMP%\impresario-shell-run` on this host (`cmd-landing.png`, `cmd-menu.png`,
`cmd-help2.png`, `cmd-run.png`, `cmd-attach.png`, `cmd-resize.png`,
`ps51-new-landing.png`, `ps51-new-menu2.png`, `ps51-final.txt`,
`ps51-quit-code.txt`); they are evidence, not repository fixtures.

### Automated equivalents already green on this host

The PTY suite exercises the real binary through ConPTY (the same console API
behind Windows Terminal and conhost) for the landing → menu → first-run flow,
completion/help, the run view and detach/close/reattach/report. Ctrl-R and
Ctrl-Q are covered by unit tests and the manual rows; they are not sent through
ConPTY because its input decoder follows the host code page.
`cargo test --workspace` also covers the non-TUI paths, daemon leases and
recovery. The three manual rows above were executed in addition, because they
check the *shell host* (font, resize, scrollback, cursor restoration), not
just the application protocol.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `impresario` prints the plain dashboard instead of the TUI | stdin/stdout is redirected, `CI` is set, or `--no-tui`/`--plain` is active. Run it in a real terminal and unset `CI`. |
| TUI shows `terminal too small` | Resize to at least 40×12 (the TUI prints the requirement). |
| `run --detach` prints a job id but no job appears | Check `impresario jobs --all` and `~/.impresario/jobs/<id>/console.log`; the daemon may have failed to start (exit 3 with the reason). |
| `attach` says `unknown job` | The id belongs to another `IMPRESARIO_HOME`; jobs are per home. |
| A job stays `queued` | Another job owns one of its device leases; `jobs` prints the position, `--on-busy fail` refuses instead. |
| `job is running; wait for it before reading the report` | `impresario wait <id>` or `attach <id>` first. |
| ConPTY test hangs with a blank screen | The harness must answer `ESC[6n` with `ESC[1;1R` **and** keep the PTY master alive; dropping it closes the output pipe (see `tests/tui_pty.rs`). |
| A fixed local port is taken | Connector and agent listeners are allocated dynamically per run, so another service holding `50051` or `50062` does not collide with Impresario. |
