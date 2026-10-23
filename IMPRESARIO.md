# IMPRESARIO.md

Orientation for an AI reader: **what this repository is and what has been
built**. Usage lives in `README.md`; the normative spec is `TASK.md`.

## What it is

Cross-platform UI test automation for **web, webOS, Tizen, Android, Android TV,
iOS and tvOS**: one DSL, one CLI, and an executor that runs *on the target* (or
next to it) instead of driving taps from the host.

## Vocabulary

- **workspace** — `impresario.config` (defaults/endpoints), `*.devices.config`
  (devices), `scenes/*.scene` (per-platform selectors), `plays/*.play`
  (platform-independent steps).
- **play** = acts of steps; a **scene** resolves selector names per platform;
  the **planner** expands a play per device and rejects anything a platform
  cannot do *before* hardware is touched.

## What has been built

| Piece | Path | What it is |
|---|---|---|
| spec + DSL | `TASK.md`, `crates/impresario-core` | scanner/parser/AST, config, validation, and the single source of truth for the platform support matrix |
| wire contract | `proto/impresario/v1/impresario.proto`, `crates/impresario-proto` | one gRPC `Agent` service for every platform + generated Rust bindings |
| master | `crates/impresario-master` | the `impresario` CLI: planner, parallel executor, reporters (console/JUnit/JSON/HTML), LSP server, MCP server |
| transports | `crates/impresario-clients` | master-side `AgentClient`, per-family transports (adb, connector, apple, mock), `MockAgent` test double |
| web executor | `crates/impresario-connector` | web/webOS/Tizen over CDP (raw + chromiumoxide backends, `launch:headless`) |
| Android agent | `agents/android` | Kotlin gRPC agent + UiAutomator driver, plus a stub app under test |
| Apple agent | `agents/apple` | Swift `AgentCore`/`ImpresarioWire`/`AgentService` + an XCUITest runner that hosts the same agent in a UI-test bundle (iOS/tvOS) |
| editor | `editors/vscode`, `crates/impresario-language` | VS Code client (LSP or standalone WASM), grammar, and the shared editor service |
| docs | `docs/` | DSL, PROTOCOL, DEVICES, PARITY, ERRORS, LSP, MCP, DECISIONS, STATUS |

## How it fits together

1. The master plans and, per device, installs/starts the executor (`adb`,
   `xcodebuild`, or spawning the connector) — that is the *only* platform-tool use.
2. Everything after that travels over the network as gRPC to the executor:
   commands in order, results/events/logs back, screenshots out of band.
3. The executor owns the platform semantics (touch vs. D-pad, keys, text entry,
   focus) and answers `UNSUPPORTED_ACTION` rather than faking success.
4. The same protocol is driven by three front ends: the CLI (`impresario run`),
   the language server (`impresario lsp`) and the MCP server (`impresario mcp`),
   so an AI agent can validate, run and read reports/screenshots on its own.

## Rules that shaped it

- Unsupported combinations fail loudly (`UNSUPPORTED_ACTION`) and are documented
  in `docs/PARITY.md`; unverifiable pieces are listed in `docs/STATUS.md`;
  spec deviations are recorded as decisions `I-nn` in `docs/DECISIONS.md`.
- TV platforms get D-pad semantics, phones get touch (`swipe` is refused on TV,
  `focus` on touch); tests are expected to be honest about what they could not do.
- Rust gates: `cargo fmt --check`, clippy `-D warnings`, `cargo test --workspace`.

## Verified versus not

- Verified live end to end over the network: web via headless Chrome; Android on
  a real phone and a real Android TV; the same loop driven purely through the
  MCP server.
- Apple: the Swift package builds and passes its unit tests (135) on Linux
  through WSL, including the full wire protocol over loopback gRPC; the Xcode
  runner targets need macOS and are written but unbuilt.
- Not available here at all: macOS/Xcode, webOS/Tizen hardware, CI execution
  (no GitHub remote), `cargo-fuzz` (needs nightly). All recorded in
  `docs/STATUS.md`.

## Pointers

- `README.md` — quick start, repository layout, requirements.
- `TASK.md` — the spec (sections 1–18) everything is implemented against.
- `docs/STATUS.md` — per-component state, evidence, known gaps.
- `docs/DECISIONS.md` — every deliberate deviation, `I-01` … `I-90`.
