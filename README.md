# impresario

Cross-platform UI test automation for **web, webOS, Tizen, Android, Android
TV, iOS and tvOS** — one DSL, one CLI, and an executor that runs *on the
target* (or next to it) instead of shelling out screen taps from the host.

- **One workspace, every platform.** `*.scene` files hold per-platform
  selectors; `*.play` files hold platform-independent steps. The planner
  expands a play per device and rejects anything the target cannot do before
  hardware is touched (`IMP-DSL-013`).
- **Everything is gRPC to an executor.** The Android/TV agent, the Apple
  agent (see `docs/STATUS.md`) and the web connector all speak the
  same `Agent` service (`proto/impresario/v1/impresario.proto`). `adb` /
  `xcodebuild` are used only to install and start the executor; every action
  then travels over the network (see `docs/PROTOCOL.md`).
- **Honest support matrix.** TVs get D-pad semantics, phones get touch,
  `swipe` is refused on TV platforms, `focus` on touch platforms — all
  documented in `docs/PARITY.md`, generated from the planner's own tables.
- **Reports you can file:** console tree, JUnit XML, `report.json`, and a
  self-contained HTML report (screenshots inlined).
- **A language server** (`impresario lsp`) with diagnostics, completion,
  hover, definitions and a shipped VS Code client — `docs/LSP.md`.
- **A Model Context Protocol server** (`impresario mcp`): any MCP-capable AI
  agent can validate workspaces, check or list devices, run plays on real
  hardware and read reports plus screenshots over stdio — `docs/MCP.md`.

## Repository layout

| Path | What |
|---|---|
| `crates/impresario-core` | DSL scanner/parser/AST, config, validation, support matrix |
| `crates/impresario-proto` | generated gRPC bindings (vendored protoc) |
| `crates/impresario-clients` | master-side `AgentClient`, device transports, `MockAgent` |
| `crates/impresario-connector` | web/webOS/Tizen executor over CDP (raw + chromiumoxide backends) |
| `crates/impresario-master` | the `impresario` CLI: planner, executor, reporters, LSP, MCP server |
| `agents/android` | Kotlin agent: `:agent` app, `:agent-instrumentation`, `:agent-core`, `:driver-uiautomator`, `:agent-target-stub` |
| `agents/apple` | Swift agent for iOS/tvOS: package builds + unit-tests on Linux, XCUITest runner needs macOS (`docs/STATUS.md`) |
| `editors/vscode` | VS Code client + TextMate grammar |
| `docs/` | DSL, protocol, devices, parity, LSP, errors, decisions, status |
| `examples/`, `fixtures/` | the example workspace written by `impresario workspace new --example`, CDP fixture pages |

## Requirements

- Rust stable **1.85+** (developed against 1.94). Nothing else is needed for
  the quick start below.
- Optional, only for the matching targets:
  - Chrome/Chromium for `web` runs (the connector launches or attaches to it),
  - JDK 17+ and the Android SDK (with `adb`) to build/install the Android
    agent — see `docs/DEVICES.md`,
  - macOS + Xcode to build the Apple agent's XCUITest runner (the Swift package
  itself builds on Linux; `docs/DEVICES.md` §3).

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
impresario workspace new --example  # writes config, devices, apps, scenes/ and plays/
impresario validate    # parse + resolve the whole workspace, no device touched
```

### 3. Smoke-run without hardware

`impresario mock-agent` (decision I-77) serves a scripted executor on
loopback so the full pipeline can be exercised on any machine. It always
reports success — it is a plumbing check, not a simulator.

Terminal A:

```sh
impresario mock-agent --port 50071
# impresario: mock device listening on 127.0.0.1:50071
```

Append this device to `impresario.devices.config` (the example play's act
targets `webos`, and `webos` falls back to the `web` selectors, so the
generated example workspace runs against it unchanged):

```yaml
  - name: mock_device
    platform: webos
    connection:
      transport: ssh
      host: 127.0.0.1
      endpoint: ws://127.0.0.1:9999
    options:
      connector_address: 127.0.0.1:50071
```

The generated `impresario.apps.config` defines `shop`, and
`plays/login.play` binds it with `App: shop` (the example play uses the webOS
platform entry). `--app NAME` overrides the header; a run never mixes two
apps.

Terminal B — the example play interpolates `${env.TEST_PASSWORD}`, so set it
first:

```sh
export TEST_PASSWORD=impresario
impresario run --play plays/login.play --device mock_device
echo $?                                    # 0 = passed
```

PowerShell instead of bash: `$env:TEST_PASSWORD = "impresario"`, then check
`$LASTEXITCODE` after the run.

You get the console tree on stdout and `reports/<run-id>/report.json` +
`junit.xml` next to it. This is exactly what
`cargo test -p impresario-master --test mock_cli` automates.

### 4. Run on a real target (optional)

- **web** — uncomment the `local_chrome` device in
  `impresario.devices.config` (or point the app's `platforms.web.url` at your
  page) and run with `--device local_chrome`; the connector starts headless
  Chrome or attaches to a running one (`docs/DEVICES.md` §1).
- **android / androidtv** — build/install the agent and run over Wi-Fi:
  `docs/DEVICES.md` §2.2–2.7 record the exact commands (verified on a
  Galaxy A51 and a TCL Android TV).
- **iOS / tvOS** — the Swift agent package builds and unit-tests on Linux
  (`docs/DEVICES.md` §3.1); the XCUITest runner targets need macOS + Xcode and
  are written but unbuilt on this host (`docs/STATUS.md`, gaps 1–2).

## Editor support

`editors/vscode` is a ready-to-side-load client (TextMate grammar included).
Any LSP-capable editor works — `docs/LSP.md` has VS Code and Neovim
recipes, the settings shapes, and the completion/diagnostic feature table.

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
| `docs/DSL.md` | syntax, semantics, selectors, text entry, timeouts |
| `docs/PROTOCOL.md` | wire protocol, sequences, rules, error codes |
| `docs/DEVICES.md` | per-platform setup and recorded hardware evidence |
| `docs/PARITY.md` | action/check/key support per platform, generated + reconciled |
| `docs/LSP.md` | language server features, settings, editor setup |
| `docs/MCP.md` | MCP server: tools, resources, per-client wiring |
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
