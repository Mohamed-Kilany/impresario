# impresario

Cross-platform UI test automation for **web, webOS, Tizen, Android, Android
TV, iOS and tvOS** — one DSL, one CLI, and an executor that runs *on the
target* (or next to it) instead of shelling out screen taps from the host.

- **One workspace, every platform.** Table-form `*.scene` files hold
  per-platform selectors, reusable `*.theatre` stages compose scenes, and
  `*.play` files bind Acts with `Perform:` and optional `Examples:` tables.
  The planner expands a play per device and rejects anything the target cannot
  do before hardware is touched (`IMP-DSL-013`). `impresario fmt` canonicalises
  all three file kinds. The pre-release schema accepts only the canonical
  table/theatre forms; older workspaces are rejected with a diagnostic.
- **Everything is gRPC to an executor.** The Android/TV agent, the Apple
  agent (see `docs/STATUS.md`) and the web connector all speak the
  same `Agent` service (`proto/impresario/v1/impresario.proto`). Android uses
  Impresario's native ADB transport (no `adb` binary); Apple's `xcodebuild` is
  used only to install and start the executor for now. Every action then
  travels over the network (see `docs/PROTOCOL.md`).
- **Honest support matrix.** TVs get D-pad semantics, phones get touch,
  `swipe` is refused on TV platforms, `focus` on touch platforms — all
  documented in `docs/PARITY.md`, generated from the planner's own tables.
- **Playback truth, not just pixels.** `Check: Showing => Piece` and `Check:
  Player => Piece Playing DRM-enabled Widevine with position>30s` collect
  DOM/HTMLMedia/EME, Android MediaSession/MediaDrm/accessibility or XCUITest
  evidence, and return `pass`, `fail` or `unverifiable` from one core
  evaluator. Missing evidence is never guessed; the optional debug-only in-app
  SDKs add authoritative player/DRM state for your own apps. See
  `docs/PROBES.md`.
- **Reports you can file:** console tree, JUnit XML, `report.json`, and a
  self-contained HTML report (screenshots inlined).
- **A language server and standalone editor** (`impresario lsp` plus the
  shipped VS Code client): diagnostics, completion, hover, navigation,
  formatting and refactoring in either LSP or in-process WASM mode -
  `docs/LSP.md`.
- **A Model Context Protocol server** (`impresario mcp`): any MCP-capable AI
  agent can validate workspaces, check or list devices, run plays on real
  hardware and read reports plus screenshots over stdio — `docs/MCP.md`.
- **A TUI and background jobs.** Run `impresario` in a terminal for a
  landing-first TUI: pick a remembered workspace, then use the overview,
  runs, tests, devices, apps and config screens with slash commands and
  history. The first start also puts `impresario` on your user `PATH` (opt
  out with `IMPRESARIO_NO_PATH_INSTALL`); `run --detach` hands a job to a
  per-user daemon so closing the terminal does not kill it. `jobs`, `attach`,
  `logs`, `stop`, `rm`, `report` and `wait` work from any shell, device leases
  prevent two jobs from driving one device, and profiles save run selections —
  `docs/CLI.md`.

## Repository layout

| Path | What |
|---|---|
| `crates/impresario-core` | DSL scanner/parser/AST, config, validation, support matrix |
| `crates/impresario-proto` | generated gRPC bindings (vendored protoc) |
| `crates/impresario-clients` | master-side `AgentClient`, device transports, `MockAgent` |
| `crates/impresario-apple-device` | pure-Rust Apple pairing/RSD/signing/install/DDI/XCUITest library |
| `crates/impresario-connector` | web/webOS/Tizen executor over CDP (raw + chromiumoxide backends) |
| `crates/impresario-master` | the `impresario` CLI: planner, executor, reporters, LSP, MCP server |
| `agents/android` | Kotlin agent: `:agent` app, `:agent-instrumentation`, `:agent-core`, `:driver-uiautomator`, `:agent-target-stub`, `:impresario-hook` |
| `agents/apple` | Impresario Agent for iOS/tvOS: Swift package builds + unit-tests on Linux, `ImpresarioHook` SDK, XCUITest bundle (macOS/Xcode only to build the artifact; the Rust host path installs/starts it) |
| `sdk/` | web JavaScript hook SDK; the Android and Swift SDKs live with their agents (`agents/android/impresario-hook`, `agents/apple/Sources/ImpresarioHook`) |
| `editors/vscode` | VS Code client (LSP or standalone WASM) + TextMate grammar |
| `docs/` | DSL, probes, protocol, devices, parity, LSP, MCP, CLI/TUI, errors, decisions, status |
| `examples/`, `fixtures/` | the example workspace written by `impresario workspace new --example` (table scenes, `theatres/login.theatre`, theatrical play), CDP fixture pages |

## Requirements

- Rust stable **1.89+** (developed against 1.94; `libadb` requires 1.87,
  `ratatui` 0.30.2 requires 1.88, and `russh` 0.63.3 requires 1.89). Nothing
  else is needed for the quick start below.
- Optional, only for the matching targets:
  - Chrome/Chromium for `web` runs, or the pinned managed browser:
    `impresario tools install chromium` (consent + SHA-256 verified; I-123),
  - JDK 17+ and the Android SDK (with `adb`) only to build or manually debug
    the Android agent; using a device needs no `adb` (I-121) - see
    `docs/DEVICES.md`,
  - macOS + Xcode only to build Impresario Agent's XCUITest bundle (the Swift
    package itself builds on Linux; pairing, signing, install and session start
    are pure Rust on Windows/macOS/Linux; `docs/DEVICES.md` §3).

## Quick start (works from a clean clone)

### 1. Install the CLI

```sh
cargo install --path crates/impresario-master --locked
# optional, for web runs: the master can start it on demand
cargo install --path crates/impresario-connector --locked
```

### 2. Create and validate a workspace

```sh
mkdir demo && cd demo
impresario workspace new --example  # writes config, devices, apps, scenes/, theatres/ and plays/
impresario validate    # parse + resolve the whole workspace, no device touched
impresario doctor      # what each platform needs, what was found, and the fix
impresario fmt --check # canonical table/stage formatting
```

### 3. Plan a run without hardware

The planner expands every Act for the selected device and reports what would
run, without opening a transport. The example play interpolates
`${env.TEST_PASSWORD}`, so set it first:

```sh
export TEST_PASSWORD=impresario
impresario run --play plays/login.play --device living_room_tv --dry-run
```

PowerShell instead of bash: `$env:TEST_PASSWORD = "impresario"`.

A configured device is only a name, an exact `type` and an `ip`; transport,
ports, pairing and signing details are derived during planning. The shipped
example file is:

```yaml
devices:
  - name: living_room_tv
    type: webOS
    ip: 192.168.1.50
```

The generated `impresario.apps.config` defines `shop`, and
`plays/login.play` binds it with `App: shop`. `--app NAME` overrides the
header; a run never mixes two apps.

`impresario mock-agent` (decision I-77) remains available for the automated
suite, but it is a test harness: production devices always use the normal
per-platform transport, and connector/agent ports are allocated dynamically.

### 4. Run on a real target (optional)

- **web** — uncomment the `local_chrome` device in
  `impresario.devices.config` (or point the app's `platforms.web.url` at your
  page) and run with `--device local_chrome`; the connector starts headless
  Chrome. A remote browser is the same device type with its host IP and must
  expose CDP on port `9222` (`docs/DEVICES.md` §1).
- **android / androidtv** — build/install the agent and run over Wi-Fi:
  `docs/DEVICES.md` §2.2–2.7 record the exact commands (verified on a
  Galaxy A51 and a TCL Android TV).
- **iOS / tvOS** — Impresario Agent builds and unit-tests as a Swift package on
  Linux (`docs/DEVICES.md` §3.1). The XCUITest bundle needs macOS + Xcode to
  build (CI does this unsigned), but the host path is pure Rust: pair once,
  import signing/product/DDI sets, then `impresario device check` /
  `impresario run`. Physical tvOS has hardware evidence (agent start,
  handshake, shutdown); the renamed bundle ids still need profiles and a full
  play. See `agents/apple/README.md` and `docs/STATUS.md` gaps 1–2.

## Editor support

`editors/vscode` is a ready-to-package client with a TextMate grammar,
snippets and two interchangeable modes: the embedded LSP (when `impresario` is
on `PATH`) or the bundled WASM service (no child process). Both use the same
`impresario-language` service and the same corpus. Any LSP-capable editor also
works - `docs/LSP.md` has VS Code and Neovim recipes, the settings shapes, and
the completion/diagnostic feature table.

## Connect an AI agent (MCP)

`impresario mcp` serves a Model Context Protocol server over stdio
(newline-delimited JSON-RPC 2.0), so any MCP-capable agent — Claude Code,
Claude Desktop, VS Code, Cursor, … — can work the workspace directly:
validate plays, list and check devices, run plays against real hardware
(Android phones/TVs through the agent), then read the report, logs and
screenshots of a run. The server speaks both the current stateless protocol
revision (per-request `_meta`, `server/discover`) and the legacy
`initialize` handshake, so any client era connects.

```json
{
  "mcpServers": {
    "impresario": { "command": "impresario", "args": ["mcp"] }
  }
}
```

`docs/MCP.md` has per-client wiring plus the full tool and resource tables;
the design is recorded as decision I-80 in `docs/DECISIONS.md`.

## Tests

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace          # mock-driven; no device required
```

- The Rust suites cover core/parser/validation, the connector against a real
  headless Chrome, master planner/executor/reporters, the LSP, and the
  README flow above.
- Android JVM unit tests: `cd agents/android && ./gradlew test`. On-device
  acceptance (driver + wire classes) is scripted in
  `agents/android/tools/run-tv-suite.ps1` and documented in
  `docs/DEVICES.md`.
- CI (`.github/workflows/ci.yml`) runs the Rust gates on Ubuntu and the
  Android unit tests with JDK 17 + the Android SDK.

## Documentation

| Doc | Contents |
|---|---|
| `docs/DSL.md` | syntax, semantics, selectors, text entry, timeouts, `Showing`/`Player` probes |
| `docs/PROBES.md` | evidence/verdict model, per-platform reachability, fallback rungs, SDK contract |
| `docs/PROTOCOL.md` | wire protocol, sequences, rules, error codes |
| `docs/DEVICES.md` | per-platform setup and recorded hardware evidence |
| `docs/PARITY.md` | action/check/key support per platform, generated + reconciled |
| `docs/LSP.md` | language server features, settings, editor setup |
| `docs/MCP.md` | MCP server: tools, resources, per-client wiring |
| `docs/CLI.md` | TUI screens/keys, slash commands, jobs/daemon, leases, profiles, test explorer, three-shell checklist |
| `docs/ERRORS.md` | every `IMP-*` code with cause and fix |
| `docs/DECISIONS.md` | the binding decision log (spec + implementation) |
| `docs/STATUS.md` | what is done, verified where, and every known gap |
| `TASK.md` | the specification this repository implements |

## Security notes

- Agent tokens ride in gRPC metadata (`x-impresario-token`) and are read
  from the environment never from files; unauthenticated calls are rejected.
- `RunAction.text` is never logged at INFO or above, and configured secrets
  are redacted from every report artifact.
- Reports may contain screenshots of the application under test — treat the
  `reports/` directory as sensitive.

## License

MIT OR Apache-2.0 (see the workspace `Cargo.toml`).
