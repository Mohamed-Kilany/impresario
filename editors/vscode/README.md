# Impresario for VS Code

Language support for the Impresario DSL in `.play`, `.scene` and `.theatre`
files, with two interchangeable front ends:

- **LSP** — starts `impresario lsp` and uses the embedded language server.
- **Standalone** — loads the bundled `media/impresario_language.wasm` in the
  extension host. No child process and no runtime network access.

Both modes run the same `impresario-language` service over `impresario-core`,
so diagnostics, completion, navigation and formatting cannot drift apart.
`impresario.mode: "auto"` (the default) uses the LSP when the binary resolves
and falls back to the standalone service otherwise.

## Features

Diagnostics with stable `IMP-…` codes and related locations; context
completion for scenes, pieces, theatres, stages, parameters, plays, acts,
variables, examples, platforms, devices, apps, durations, selectors and
keywords; hover; go to definition; references; prepare-rename and rename;
document and workspace symbols; folding; semantic tokens; signature help;
code actions (including did-you-mean fixes and unknown-stage parameter
fixes); whole-document, range and on-type table formatting; inlay hints; and
code lenses.

The standalone index is watched and debounced. It keeps file text in a cache,
re-reads only changed files, and re-analyses the shared service snapshot after
a 150 ms burst. The test suite includes a 1,000+ file benchmark.

## Requirements

- **LSP mode:** `impresario` on `PATH`, or set `impresario.serverPath` to the
  binary. LSP mode runs inside a workspace with `impresario.config` /
  `impresario.devices.config`.
- **Standalone mode:** no external binary. The WASM service ships in the
  VSIX; `impresario.standalone.wasmPath` can override it.
- **Development:** Node 20+ and a Rust toolchain with the
  `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`).
  VS Code is downloaded only by the extension-host tests, never at runtime.

## Build, test, package

```sh
cd editors/vscode
npm ci
npm run build:wasm       # cargo build --release --target wasm32-unknown-unknown
npm run build            # esbuild bundle into out/
npm run check            # strict tsc + ESLint
npm run test:unit        # corpus parity + 1,000-file benchmark (Node + WASM)
npm run test:host        # @vscode/test-electron extension-host suite
npm run package          # impresario-0.1.0.vsix
```

`npm test` runs the build, the unit/corpus tests and the extension-host tests.
The host suite copies `fixtures/lang-corpus` into a temporary workspace,
switches to standalone mode, and exercises diagnostics, completion, hover,
definition, formatting, symbols and code actions in a real VS Code window.

Install a built package with:

```sh
code --install-extension impresario-0.1.0.vsix
```

The VSIX contains `out/extension.js`, `media/impresario_language.wasm`, the
TextMate grammar, snippets and `language-configuration.json`; sources, tests
and tooling are excluded by `.vscodeignore`.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `impresario.mode` | `auto` | `auto`, `lsp` or `standalone`. |
| `impresario.serverPath` | `impresario` | Binary that serves `impresario lsp` (PATH name or absolute path). |
| `impresario.workspace` | *(first folder)* | Passed as `--workspace`. |
| `impresario.config` | `impresario.config` | Passed as `--config`; resolved against the workspace. |
| `impresario.devices` | `impresario.devices.config` | Passed as `--devices-config`; read by standalone mode. |
| `impresario.apps` | `impresario.apps.config` | Read by standalone mode to validate `App:` names. |
| `impresario.standalone.wasmPath` | *(bundled)* | Optional override for `media/impresario_language.wasm`. |

Use **Impresario: Restart Language Support** (status-bar item or command) after
changing modes or the workspace.

The repository's `docs/LSP.md` documents the protocol, the complete feature
table, Neovim setup and the Rust-side test suite.
