# Impresario for VS Code

A minimal client for the embedded language server of the Impresario DSL
(`impresario lsp`, spec section 13). It binds `.play` and `.scene` files to
the `impresario-play`/`impresario-scene` languages, adds a TextMate grammar
and forwards diagnostics, completion, hover, definitions and the outline.

## Requirements

`impresario` must be on `PATH` (or set `impresario.serverPath` to the binary);
run it inside a workspace that has `impresario.config` /
`impresario.devices.config` so the server can index scenes and plays.

## Run it in development

```sh
npm install
code --extensionDevelopmentPath="$PWD"
```

`npm install` pulls `vscode-languageclient`; the extension itself is
CommonJS (`extension.js`), no build step.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `impresario.serverPath` | `impresario` | The binary that serves `impresario lsp`. |
| `impresario.workspace` | *(first folder)* | Passed as `--workspace`. |
| `impresario.config` | `impresario.config` | Passed as `--config` and as `initializationOptions.impresario.config`. |
| `impresario.devices` | `impresario.devices.config` | Passed as `--devices-config` and as `initializationOptions.impresario.devices`. |

See `docs/LSP.md` in the repository for the full feature and settings
reference, the Neovim setup and the test suite.
