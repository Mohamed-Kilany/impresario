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
`--tag`, `--exclude-tag`, `--device` (repeatable or `all`), `--platform`,
`--app`, `--var K=V`, `--arg ACT.PARAM=VALUE`, `--timeout`, `--retries`,
`--reporter`, `--fail-fast`, `--only-failed`. `--exclude-tag` wins over
`--tag`. Inline platform filtering treats an act stage with no explicit
platform restriction as applicable (I-129).

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

### Screens and keys

| Key | Screen |
|---|---|
| F1 | Help overlay (keys, slash commands, examples) |
| F2 / F3 / F4 / F5 / F6 / F7 | Home / Runs / Tests / Devices / Apps / Config |
| Ctrl-Q, or Ctrl-C twice within 1.2 s | Quit (terminal restored) |
| Ctrl-Z | Suspend (Unix); terminal is restored and re-entered |
| Esc | Close overlay/completion/filter, or return focus to the prompt |

The Home screen shows the workspace and config paths, config/devices/apps
counts, test counts and active/recent jobs. The global home appears in the
first-run overlay and the help text. The TUI deliberately does not probe
device reachability or run `doctor` in the background: use `/doctor` or
`impresario device check` when live state matters. In-app completion notices
appear when a followed job reaches a terminal state.

Prompt editing: history with Up/Down, Ctrl-R reverse search, Tab completion
(device/app/play/act/stage/tag names and slash-command flags), Shift-Tab for
the previous completion, Ctrl-A/E/U/K/W, Alt+Enter or Ctrl-J for a newline,
and bracketed paste. History is `~/.impresario/history` (permission 0600 on
Unix, capped by `ui.history_size`).

**Run view** (`F3`, or `/run`/`/attach` which submit/follow a job): left pane = jobs, right pane = the followed
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
| `Tab` | Cycle the device filter |
| `/` | Filter the run list (Enter applies, Esc cancels) |
| `r` / `F` | Rerun the selection / rerun failed acts from its report |
| `o` / `p` | Open the report / the failure screenshot |
| `X` or Delete | Remove a finished job's state and logs |
| `?` | Help overlay |

**Tests screen** (`F4`): `j`/`k` move, `g`/`G` first/last, Space marks,
`Enter` opens the file in the editor, `r` runs the selected/marked play or act
(detached), `d` prints the dry-run matrix for the selection. `Tab` returns to
the prompt.

**Devices/Apps/Config screens** (`F5`–`F7`): `j`/`k` scroll, `Enter` opens the
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
| `/devices`, `/apps`, `/config` | Open those screens |
| `/tests [args]` | Open the explorer; with args runs `impresario tests ...` |
| `/open <path>` | Open a file/directory with the OS handler |
| `/doctor [--json]` | Run `impresario doctor` and print the result |
| `/fmt [args]`, `/validate [args]` | Run the matching CLI command |
| `/plan [flags]` | Run `impresario run --dry-run` |
| `/attach <id>` | Follow a job inside the TUI (does not spawn a terminal) |
| `/logs <id>`, `/report <id>`, `/stop <id>`, `/rm <id>` | Run the matching CLI command |
| `/profile [args]` | Run `impresario profile ...` |
| `/history`, `/clear`, `/home` | Show history, clear activity, go home |
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

In a directory without `impresario.config`, the TUI opens an overlay:

```text
Welcome to impresario
No workspace config was found yet. Create the workspace skeleton now?
  [y] create impresario.config, plays/, scenes/, theatres/ and an example play
  [n] or Esc skip; you can create it later with:
      impresario workspace new . --example
```

`y` runs `impresario workspace new <base> --example`; `n`/Esc only dismisses
the overlay. The generated example includes one playable act (`Check: Sample`),
so a fresh workspace immediately validates and can run against a mock.

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
  through ConPTY: first-run scaffold (`y` → files on disk → quit), slash
  completion + Ctrl-R history, and run view (`MockAgent`, `b` detach → quit →
  `attach` → `report --json`). The harness answers ConPTY's startup cursor
  query (`ESC[6n` with `ESC[1;1R`), keeps the PTY master alive, resolves ANSI
  cursor movement/erases in a small screen model and asserts visible text.
- `crates/impresario-master/src/tui/snapshots.rs` renders six frames through
  ratatui `TestBackend` + `insta`: home, followed run view, tests explorer,
  help/filter overlay, first-run overlay and a small terminal (the
  "too small" path).
- `tests/jobs_cli.rs` covers the daemon and leases: detach returns before the
  daemon exits, a second caller on a live lease follows `--on-busy
  wait|fail|queue`, a stopped job exits 130, recovery marks orphaned jobs
  `interrupted`, and a detached submission releases the caller's stdout
  pipes (Windows regression test).
- Live detach → attach → report on the web device: run
  `20261021-073144-2763` passed 9/9 steps (Chrome for Testing over managed
  Chromium); see `docs/STATUS.md` for the multi-device status.

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

- [ ] Start Windows Terminal, open a PowerShell tab, `cd` to the workspace.
- [ ] `impresario` starts the TUI, the header shows the workspace and the tabs
      `home runs tests devices apps config`, and no prompt is left in the
      scrollback.
- [ ] F1 opens help; Esc closes it. F2–F7 switch screens without flicker.
- [ ] Type `/he`, see `/help` in the completion popup, press Tab, Enter; the
      help overlay says `help · Esc closes`.
- [ ] Ctrl-R, type `run`, Enter recalls the line; Up/Down walks history.
- [ ] `/run --dry-run` prints the matrix in the activity/status area; a real
      `/run` shows `Jobs (1)` and a running job, `b` detaches with
      `continues in the background`, and `/attach <id>` follows it again.
- [ ] Resize the window while following a job: content reflows, no panic, no
      duplicated frame. Ctrl-Q restores the original screen and the shell
      prompt is usable.
- [ ] `impresario --no-tui` and `impresario --plain` print the plain
      dashboard; `impresario --no-color` removes colour but keeps the layout.
- [ ] `impresario completions powershell | Out-String | Invoke-Expression` and
      Tab-complete `impresario ru` → `run`.

Expected: the same layout at 120×30 and at 80×24; box drawing is unbroken;
the status area shows keys, the selected job and the last activity message.

### 2. Windows PowerShell 5.1 (conhost)

- [ ] Start `powershell.exe` (not `pwsh`) from the Start menu or `Win+R`.
- [ ] Repeat the TUI startup, F1/F2–F7, `/he` + Tab, Ctrl-R, `/run`, `b`,
      `/attach`, resize and Ctrl-Q checks.
- [ ] Confirm the alternate screen is restored on quit and that the console
      cursor is visible again.
- [ ] Run `impresario --plain | more` and `impresario jobs --json | ConvertFrom-Json`;
      both must not hang and must exit 0.
- [ ] Run `impresario` with stdin redirected (`echo. | impresario`): it must
      print the plain dashboard, not the TUI, and must not read the pipe.

Known conhost detail: if the window is narrower than 40 columns or shorter
than 12 rows, the TUI prints `terminal too small` and asks to resize or use
`--plain`. It must not draw a corrupted frame.

### 3. cmd.exe (conhost or Windows Terminal tab)

- [ ] `cmd /k` (or a Command Prompt tab), `cd` to the workspace.
- [ ] `impresario` starts the TUI; F1, F2–F7, `/he`+Tab, Ctrl-R, `/run`,
      `b`, `/attach`, resize, Ctrl-Q behave as above.
- [ ] `set CI=1` then `impresario`: plain dashboard, exit 0, no TUI.
- [ ] `set CI=` then `impresario --plain`: plain dashboard.
- [ ] `impresario run --dry-run` and `impresario jobs --json` work; JSON is
      parseable (for example `impresario jobs --json > jobs.json`).
- [ ] `impresario completions cmd` is not a supported shell; expect the
      PowerShell/bash/zsh/fish/elvish list, not a panic.

Expected: command history, pasted multi-line text and box-drawing characters
are intact; no “not a terminal” error when the TUI is expected.

### Automated equivalents already green on this host

The PTY suite exercises the real binary through ConPTY (the same console API
behind Windows Terminal and conhost) for the first-run flow, completion,
Ctrl-R, the run view and detach/close/reattach/report. `cargo test --workspace`
also covers the non-TUI paths, daemon leases and recovery. The three rows
above remain manual because they check the *shell host* (font, resize,
scrollback, cursor restoration), not just the application protocol.

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
| Port 50051 is taken | Common on this host (`multipassd`); pin the web device with `options.connector_address: 127.0.0.1:<free-port>`. |
