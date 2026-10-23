# Shared language corpus

The same files and expectations are exercised by three independent front ends:

1. the pure Rust service (`crates/impresario-language/tests/corpus.rs`),
2. the embedded LSP over real JSON-RPC framing
   (`crates/impresario-master/tests/lsp_corpus.rs`),
3. the standalone WASM extension (`editors/vscode/test/corpus.test.js`).

Any divergence fails CI. `files/` is copied verbatim into LSP/extension test
workspaces; `devices.config` and `apps.config` become
`impresario.devices.config`/`impresario.apps.config`.

## Layout

| Path | Meaning |
|---|---|
| `files/**/*.{scene,theatre,play}` | Source files keyed by their path under `files/`. |
| `expected-diagnostics.json` | Normalized diagnostics per file. `related` lists related locations. |
| `expected-format/<path>` | Exact canonical formatter output for every source file. |
| `completion-probes.json` | Positions (1-based line/column, Unicode scalar) and labels that must be offered. |
| `feature-probes.json` | Hover/navigation/symbol/formatting/code-action probes shared by the LSP and WASM tests. |

Diagnostic normalization compares `code`, `severity`, `line`, `col`, `message`
(with the LSP's `\nhelp: …` suffix stripped) and every related location.
