# Configuration

Everything a run needs is a setting: project, timeouts, directories, reporters,
agent timings, UI preferences and the global workspace registry. The schema in
`impresario-core::settings` is the single source of truth — the loader, the
`impresario config` commands, the LSP and this table all read it. The
`config_doc` test fails if this page drifts from the schema.

## Files and precedence

| Layer | File |
|---|---|
| CLI flag | `--workspace`, `--config`, per-command flags |
| Environment | `IMPRESARIO_WORKSPACE`, `IMPRESARIO_HOME`, `IMPRESARIO_*` secrets |
| Workspace | `<workspace>/impresario.config` |
| Global | `~/.impresario/config.yaml` (`%USERPROFILE%\.impresario\config.yaml` on Windows) |
| Built-in default | the schema defaults below |

High beats low. A missing file is an empty layer, and every key has a default,
so an empty workspace is valid. `impresario config get <key> --explain` prints
the winning layer and every shadowed value. `--global` and `--workspace` select
one file; without either flag reads use the merged value and writes go to the
workspace file.

## Editing

```console
$ impresario config list
$ impresario config get timeout_default_ms --explain
$ impresario config set timeout_default_ms 45000
$ impresario config set timeout_default_ms 45000 --global
$ impresario config unset timeout_default_ms
$ impresario config path --global
$ impresario config edit --global
```

`set`, `unset` and `edit` parse the whole file again and refuse an edit that
would make it invalid; `edit` restores the previous contents and offers to
reopen the editor when the result does not parse. Edits preserve comments, key
order, line endings and every untouched byte (TASK-2 section 4.2). The format
stays YAML (I-91).

Unknown keys are errors (`IMP-CFG-001`) and invalid values use the same
`IMP-CFG-*` diagnostics as the loader.

## Keys

<!-- BEGIN GENERATED CONFIG -->
| Key | Scope | Type | Default | Description |
|---|---|---|---|---|
| `project` | global + workspace | string | (unset) | Project name used in reports (default: the workspace directory name). |
| `timeout_default_ms` | global + workspace | integer 100..=600000 | `30000` | Default timeout for one command, in milliseconds. |
| `screenshot_on_failure` | global + workspace | bool | `true` | Capture a screenshot when a step fails. |
| `report_output` | global + workspace | path | `./reports` | Directory for report files. |
| `plays_dir` | global + workspace | path | `./plays` | Directory scanned for .play files. |
| `scenes_dir` | global + workspace | path | `./scenes` | Directory scanned for .scene files. |
| `theatres_dir` | global + workspace | path | `./theatres` | Directory scanned for .theatre files. |
| `retries` | global + workspace | integer 0..=5 | `0` | Retry a failed act this many times. |
| `parallel_devices` | global + workspace | integer 1..=64 | `4` | Maximum devices driven in parallel within one job. |
| `app_reset` | global + workspace | one of: per_act, per_play, never | `per_act` | When app state is reset between acts or plays. |
| `poll_interval_ms` | global + workspace | integer 20..=2000 | `100` | Polling interval for waits and checks, in milliseconds. |
| `scroll_max_attempts` | global + workspace | integer 1..=100 | `10` | Maximum scroll attempts while looking for a piece. |
| `focus_max_steps` | global + workspace | integer 1..=500 | `50` | Maximum focus traversal steps on TV platforms. |
| `agent_start_timeout_ms` | global + workspace | integer 1000..=600000 | `60000` | How long to wait for an executor to start, in milliseconds. |
| `command_grace_ms` | global + workspace | integer 0..=30000 | `2000` | Extra grace period after a command completes, in milliseconds. |
| `reporters` | global + workspace | list of: console, junit, json, html | `console,junit,json` | Reporters attached to a run. |
| `workspace` | global | path | (unset) | Workspace the CLI uses when no flag, environment variable or marker applies. |
| `workspaces` | global | list of paths | (unset) | Workspaces remembered by `impresario workspace add`. |
| `ui.theme` | global + workspace | string | `auto` | TUI color theme (`auto`, `dark`, `light` or a theme name). |
| `ui.history_size` | global + workspace | integer 100..=100000 | `1000` | Number of interactive prompt entries kept. |
<!-- END GENERATED CONFIG -->
