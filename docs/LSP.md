# Embedded LSP and editor service (`impresario lsp`)

`impresario lsp` runs the language server of the Impresario DSL over **stdio**
(spec section 13). It is built into the `impresario` binary - no separate
process to install - and speaks plain LSP 3.17, so any editor with a language
client can use it.

```
impresario lsp [--workspace DIR] [--config FILE] [--devices-config FILE]
```

`--workspace` is the workspace root (default: the current directory); the two
config paths resolve against it like every other command. **stdout carries the
protocol only** - logs go to stderr (`RUST_LOG=impresario_master=debug` for
detail). The server exits with code 0 when the client closes stdin.

All features are implemented once in the pure `impresario-language` crate. The
LSP converts its DTOs to LSP types; the standalone VS Code mode calls the same
crate through WASM. `.play`, `.scene` and `.theatre` are equally supported.

## Features

| Feature | Behaviour |
|---|---|
| **Diagnostics** | Every parser and semantic diagnostic of `impresario-core` plus the per-device warnings, published with their stable `IMP-...` codes, related locations and the `impresario` source. Debounced 150 ms after `didOpen`/`didChange`/`didSave` and after file-watch bursts. Open buffers carry their document `version`. |
| **Completion** | Computed from the line prefix, the nearest enclosing `Scene:`/`Act:` block and the workspace index; it keeps working while other parts of the file have errors. Trigger characters: `:`, ` `, `'`, `>`. Contexts are listed below. |
| **Hover** | Piece declarations and piece references show the resolved selector for every platform, including fallback rows. `Perform:`/`Stage:` show the signature, theatre, description and reference count. |
| **Go to definition** | `Scene:` to its declaration, pieces to their table/entry row, `Perform: Stage` and `Theatre.Stage` to the stage declaration, variables/parameters to their declarations. Works for unsaved buffers. A single result is returned as a scalar `Location`; multiple results as an array. |
| **References / rename** | Workspace-wide references and prepare-rename/rename for pieces, scenes, stages, theatres, variables and parameters. Rename returns a `WorkspaceEdit` covering every file. |
| **Symbols** | Document outline (`Play -> Act -> Perform/Stage -> Scene`, `Theatre -> Stage -> Scene`, `Scene -> Piece`) with full block ranges. Workspace symbols filter all indexed declarations. |
| **Formatting** | Whole-document, range and on-type formatting come from `impresario_core::fmt`; on typing `|` the enclosing table is re-aligned, and Enter re-aligns the current line when the client sends it. Unsaved overlays are formatted without touching disk. An empty edit array means unchanged. |
| **Folding / semantic tokens** | Folding ranges for plays, acts, stages, scenes and tables; semantic tokens for namespaces, functions, properties, variables, classes and keywords. |
| **Signature help** | Inside `Perform: Stage(`: stage signature, documentation and the active parameter (including defaults). |
| **Code actions** | Did-you-mean fixes, create missing scene/stage/piece, add a missing platform column, change an unknown stage parameter, make duplicates unique, remove unused declarations, and extract selected steps into a stage. |
| **Inlay hints / code lenses** | Resolved parameter hints on `Perform:` calls; "Run act ..." and reference-count lenses. |
| **Unsaved buffers** | Full-sync overlays; an open buffer wins over disk for parsing and for the index - an edited-but-unsaved scene is indexed immediately. |
| **File watching** | `notify`, debounced 200 ms, recursive: `scenes_dir`, `theatres_dir`, `plays_dir` and the config/devices/apps files. If a directory does not exist yet, its parent is watched until it appears. Editor temp/swap files (`.swp`, `~`, `#...#`, `.tmp`, ...) are ignored. |
| **Position encoding** | `utf-8` when the client offers it, otherwise `utf-16` (the LSP default). Spans from `impresario-core` are Unicode scalar values and are converted per encoding, surrogate pairs included. |
| **Settings** | `impresario.config` / `impresario.devices` can point the server at other config files at runtime (see below). |

### Completion contexts

| Context (line prefix) | Suggestions |
|---|---|
| file headers | keywords valid for the file kind (`Play:`/`Theatre:`/`Scene:`, `Act:`, `Stage:`, `Description:`, `Tags:`, `Platform(s):`, `App:`, `Var:`, `Examples:`, `Timeout:`, `Perform:`, `Action:`, `Check:`, `Pieces:`) |
| `Scene: ` inside a Stage | scene names with their platforms in `detail` |
| `Perform: ` (play or theatre) | stage names, with signature/theatre in `detail` |
| `Perform: Stage(` before `)` | that stage's parameters, with default or `required` in `detail`; qualified `Theatre.Stage` names resolve too |
| `Examples:` header row | parameters of every stage performed by the enclosing act, with defaults |
| `Action: <anything> => ` | pieces of the enclosing scene, sorted, with selector `detail` |
| `Action: ` (start) | action verbs and `'` string starter; `wait ` offers durations; `app ` offers app operations |
| `Check: ` | pieces; then `is visible/hidden/enabled/disabled/focused`, `text`, `textContains`, `count`, `timeout`; durations where a timeout is accepted |
| `key ` | key names, filtered to the configured devices' platforms when known |
| `Tags: ` | tags already used in the workspace |
| `Platforms: ` / `Platform: ` | platform names (plus `cross-platform` in scenes), with configured device names in `detail` |
| `App:` in a play/theatre | app names from `impresario.apps.config` |
| table scenes | `Piece` and platform header cells, known piece names in body rows, then selector prefixes for the column's platform |
| legacy `Pieces:` entries | platform keys (`web=`, `android=`, ...) and valid selector prefixes |
| inside strings | `${var.` / `${param.` / `${env.` / `${example.` namespaces and the visible names |

### Settings

The server accepts the same options through `initializationOptions` and
through `workspace/didChangeConfiguration` (`settings`), in three shapes:

```jsonc
{ "impresario": { "config": "impresario.config", "devices": "impresario.devices.config" } }
{ "impresario.config": "impresario.config", "impresario.devices": "impresario.devices.config" }
{ "config": "impresario.config", "devices": "impresario.devices.config" }
```

Relative paths resolve against the workspace root. Changing `devices`
re-analyses the workspace at once, so device warnings follow
`impresario.devices.config` without a restart. The **device set** for warnings
and `key` completion is every device in the devices file.

## VS Code

The extension in [`editors/vscode`](../editors/vscode) supports three modes:

| `impresario.mode` | What runs |
|---|---|
| `auto` (default) | `impresario lsp` when the binary resolves, otherwise standalone |
| `lsp` | the embedded server over stdio |
| `standalone` | the bundled `media/impresario_language.wasm` in the extension host, with no child process and no runtime network access |

The standalone mode uses a file-system watcher, a debounced analysis queue and
a changed-file text cache; its diagnostics and feature DTOs are the same ones
the LSP uses. The shared corpus in `fixtures/lang-corpus/` is executed by the
Rust service, the LSP test suite and the Node/WASM test suite, so all three
front ends fail CI on divergence.

Development install:

```sh
cd editors/vscode
npm ci
npm run build:wasm                              # Rust wasm32 target
npm run build                                   # esbuild -> out/
code --extensionDevelopmentPath="$PWD"
# optional, for a permanent install:
npm run package && code --install-extension impresario-0.1.0.vsix
```

Settings (`settings.json`):

```jsonc
{
  "impresario.mode": "auto",                         // auto | lsp | standalone
  "impresario.serverPath": "impresario",             // LSP binary, PATH or absolute
  "impresario.workspace": "",                        // default: first workspace folder
  "impresario.config": "impresario.config",          // resolved against the workspace
  "impresario.devices": "impresario.devices.config",
  "impresario.apps": "impresario.apps.config",
  "impresario.standalone.wasmPath": ""               // optional bundled-WASM override
}
```

LSP mode forwards `impresario.config`/`impresario.devices` as
`initializationOptions`. Standalone mode reads the devices and apps files
directly and uses the same core validation. If LSP startup fails in `auto`
mode, the extension logs the failure and starts standalone mode; an explicit
`lsp` or `standalone` mode reports the failure instead of silently switching.

Its grammar file also works with other `.play`/`.scene`/`.theatre` setups:
register `source.impresario` for the three extensions.

## Neovim

Neovim 0.11+ (`vim.lsp.config`):

```lua
vim.filetype.add({ extension = {
  play = "impresario-play",
  scene = "impresario-scene",
  theatre = "impresario-theatre",
} })

vim.lsp.config("impresario", {
  cmd = { "impresario", "lsp" },
  filetypes = { "impresario-play", "impresario-scene", "impresario-theatre" },
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
      filetypes = { "impresario-play", "impresario-scene", "impresario-theatre" },
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

## Anything else

Any LSP 3.17 client works: start `impresario lsp` on stdio and (optionally)
send the settings above as `initializationOptions` or
`workspace/didChangeConfiguration`. The server lets the client drive text
synchronisation (full sync), completion, hover, definitions, references,
rename, symbols, folding, semantic tokens, signature help, code actions,
formatting and inlay hints.

## Tests

The server has an in-process test client over a real duplex stream and real
`Content-Length` framing (`crates/impresario-master/tests/common/lsp.rs`):

- `tests/lsp_diagnostics.rs` - open/change/close, overlay vs disk, debounced
  bursts, the UTF-16/UTF-8 offset difference (surrogate pairs), config
  diagnostics, device warnings + `didChangeConfiguration` switching, unsaved
  files outside the configured directories;
- `tests/lsp_features.rs` - completion contexts, hover, definitions, document
  symbols and device-filtered keys;
- `tests/lsp_editor.rs` - the full feature surface: references, prepare-rename
  and rename, workspace symbols, folding, semantic tokens, signature help,
  code actions, range/on-type formatting, inlay hints and code lenses;
- `tests/lsp_corpus.rs` - the shared `fixtures/lang-corpus/` files and feature
  probes through real JSON-RPC framing;
- `tests/lsp_watch.rs` - external edits, a `scenes` directory created after
  start, editor noise never triggering an analysis;
- `tests/lsp_cli.rs` - the real binary on real stdio.

`cargo test -p impresario-master` runs them together with the unit tests of
the LSP modules (position conversion, watch specs, conversions).

The standalone side is tested by `editors/vscode`:

- `npm run test:unit` runs the shared corpus and a 1,000+ file index benchmark
  against the release WASM build;
- `npm run test:host` launches a real VS Code extension host and exercises
  standalone activation, diagnostics, completion, hover, definition,
  formatting (document/range/on-type), symbols and code actions;
- `npm run check` enforces strict TypeScript and ESLint.
