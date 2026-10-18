# Embedded LSP (`impresario lsp`)

`impresario lsp` runs the language server of the Impresario DSL over **stdio**
(spec section 13). It is built into the `impresario` binary — no separate
process to install — and speaks plain LSP 3.17, so any editor with a language
client can use it.

```
impresario lsp [--workspace DIR] [--config FILE] [--devices-config FILE]
```

`--workspace` is the workspace root (default: the current directory); the two
config paths resolve against it like every other command. **stdout carries the
protocol only** — logs go to stderr (`RUST_LOG=impresario_master=debug` for
detail). The server exits with code 0 when the client closes stdin.

## Features

| Feature | Behaviour |
|---|---|
| **Diagnostics** | Every parser and semantic diagnostic of `impresario-core` plus the per-device warnings, published with their stable `IMP-…` codes and `impresario` source. Debounced 150 ms after `didOpen`/`didChange`/`didSave` and after file-watch bursts. Open buffers carry their document `version`. |
| **Completion** | Computed from the current line prefix and the nearest enclosing `Scene:` line of lower indentation (scanning upward), so it keeps working while the rest of the file has errors. Trigger characters: `:`, ` `, `'`, `>`. See the context table below. |
| **Hover** | On a piece (in a step or in its declaration): the per-platform selector table as Markdown. |
| **Go to definition** | `Scene: X` → the scene header in its `.scene` file; a piece in a step → its declaration line. Works for unsaved buffers. |
| **Document symbols** | `Play → Act → Stage` for `.play`, `Scene → Pieces` for `.scene`. |
| **Unsaved buffers** | Full-sync overlays; an open buffer wins over disk for parsing **and** for the index — an edited-but-unsaved scene is indexed immediately. |
| **File watching** | `notify`, debounced 200 ms, recursive: `scenes_dir`, `plays_dir` and both config files. If a directory does not exist yet, its parent is watched until it appears. Editor temp/swap files (`.swp`, `~`, `#…#`, `.tmp`, …) are ignored. |
| **Position encoding** | `utf-8` when the client offers it, otherwise `utf-16` (the LSP default). Spans from `impresario-core` are Unicode scalar values and are converted per encoding, surrogate pairs included. |
| **Settings** | `impresario.config` / `impresario.devices` can point the server at other config files at runtime (see below). |

### Completion contexts

| Context (line prefix) | Suggestions |
|---|---|
| `Scene: ` inside a Stage | all scene names (with their platforms as `detail`) |
| `Action: <anything> => ` | pieces of the enclosing scene, sorted, with selector `detail` |
| `Check: ` | pieces; after a piece + space: `is visible/hidden/enabled/disabled/focused`, `text`, `textContains`, `count`, `timeout` |
| `Action: ` (start) | action verbs + the `'` string starter (inserts `''`) |
| `key ` | key names, filtered to the configured devices' platforms when known |
| `Tags: ` | tags already used in the workspace |
| `Platforms: ` / `Platform: ` | platform names (plus `cross-platform` in scenes) |
| inside `Pieces:` entries | platform keys (`web=`, `android=`, …) and the selector prefixes valid for that key |

### Settings

The server accepts the same options through `initializationOptions` and
through `workspace/didChangeConfiguration` (`settings`), in three shapes:

```jsonc
{ "impresario": { "config": "impresario.config", "devices": "impresario.devices.config" } }
{ "impresario.config": "impresario.config", "impresario.devices": "impresario.devices.config" }
{ "config": "impresario.config", "devices": "impresario.devices.config" }
```

Relative paths resolve against the workspace root. Changing `devices`
re-analyses the workspace at once, so the device warnings follow
`impresario.devices.config` without a restart. The **device set** for the
warnings and the `key` completion is every device in the devices file.

## VS Code

A minimal client ships in [`editors/vscode`](../editors/vscode) — it launches
`impresario lsp`, wires `.play`/`.scene` to it and adds a TextMate grammar.
Development install:

```sh
cd editors/vscode
npm install
code --extensionDevelopmentPath="$PWD"          # a window over your workspace
# optional, for a permanent install:
npx @vscode/vsce package && code --install-extension impresario-0.1.0.vsix
```

Settings (`settings.json`):

```jsonc
{
  "impresario.serverPath": "impresario",             // the binary (PATH or absolute)
  "impresario.workspace": "",                        // default: first workspace folder
  "impresario.config": "impresario.config",          // resolved against the workspace
  "impresario.devices": "impresario.devices.config"
}
```

The extension forwards `impresario.config`/`impresario.devices` as
`initializationOptions`, and `impresario.serverPath` has to be on `PATH` (or
absolute). Its grammar file also works with other `.play`/`.scene` setups:
register `source.impresario` for both extensions.

## Neovim

Neovim 0.11+ (`vim.lsp.config`):

```lua
vim.filetype.add({ extension = { play = "impresario-play", scene = "impresario-scene" } })

vim.lsp.config("impresario", {
  cmd = { "impresario", "lsp" },
  filetypes = { "impresario-play", "impresario-scene" },
  root_markers = { "impresario.config" },
  settings = {
    impresario = {
      config = "impresario.config",
      devices = "impresario.devices.config",
    },
  },
})
vim.lsp.enable({ "impresario" })
```

`nvim-lspconfig` (older releases) with a custom server entry:

```lua
local lspconfig = require("lspconfig")
local configs = require("lspconfig.configs")

if not configs.impresario then
  configs.impresario = {
    default_config = {
      cmd = { "impresario", "lsp" },
      filetypes = { "impresario-play", "impresario-scene" },
      root_dir = lspconfig.util.root_pattern("impresario.config"),
      settings = {
        impresario = {
          config = "impresario.config",
          devices = "impresario.devices.config",
        },
      },
    },
  }
end

lspconfig.impresario.setup({})
```

If you do not register the filetypes, start the client for `yaml`-like
buffers with `:LspStart impresario` or set `filetypes` to the ones your setup
already uses.

## Anything else

Any LSP 3.17 client works: start `impresario lsp` on stdio and (optionally)
send the settings above as `initializationOptions` or
`workspace/didChangeConfiguration`. The server lets the client drive text
synchronisation (full sync), completions (with the trigger characters),
hover, definitions and document symbols.

## Tests

The server has an in-process test client over a real duplex stream and real
`Content-Length` framing (`crates/impresario-master/tests/common/lsp.rs`):

- `tests/lsp_diagnostics.rs` — open/change/close, overlay vs disk, debounced
  bursts, the UTF-16/UTF-8 offset difference (surrogate pairs), config
  diagnostics, device warnings + `didChangeConfiguration` switching, unsaved
  files outside the configured directories;
- `tests/lsp_features.rs` — every completion context above, hover,
  definitions, document symbols, device-filtered keys;
- `tests/lsp_watch.rs` — external edits, a `scenes` directory created after
  start, editor noise never triggering an analysis;
- `tests/lsp_cli.rs` — the real binary on real stdio.

`cargo test -p impresario-master` runs them together with the unit tests of
the LSP modules (position conversion, completion, navigation, watch specs).
