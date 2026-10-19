# Status

Current development status, per component. Updated at every phase boundary.
"Verified" means an automated check (test or command) ran successfully in this
repository on this machine (Windows 11, Rust stable 1.94.x).

## Environment constraints

- Host: Windows. No Xcode, so the Apple **runner** (XCUITest/UIKit targets) cannot
  be compiled here. The Apple **agent package** is built and unit-tested on this
  host through WSL (Ubuntu 22.04, Swift 6.4.1): `swift build` and `swift test`
  cover `AgentCore`/`AgentService`, including the full wire protocol over
  loopback gRPC. Both halves are documented in `docs/DEVICES.md` §3; the
  human-verification checklist for a macOS host is in `agents/apple/README.md`.
- Phase 0 spike "gRPC (NIO) server inside an XCUITest runner" therefore cannot
  be executed here. What *is* verified here is that the same `AgentService`
  serves the protocol (loopback tests pass); what remains open is the runner
  process itself on a device (XCTest lifecycle, local-network permission,
  `XCUIRemote`). D-14/D-18 stay **unverified on hardware** until someone runs
  the spike on a Mac; this is a recorded gap, not a silent one.
- Android SDK is present (`%LOCALAPPDATA%\Android\Sdk`), JDK 24, Gradle wrapper
  distributions cached. The Android acceptance runs on a real Android TV over
  adb (`docs/DEVICES.md` §2.5). The API 36.1 x86_64 AVD was retired after a
  host emulator graphics bug (I-57); its setup stays recorded
  (`docs/DEVICES.md` §2.3) in case another host can run it.
  - Gradle needs `-Djava.net.preferIPv6Addresses=true` here (IPv4 egress to the
    plugin repositories times out; pre-recorded in `agents/android/gradle.properties`).
  - `avdmanager` needed `SKIP_JDK_VERSION_CHECK=1` under JDK 24.

## Components

| Component | State | Verification |
|---|---|---|
| `proto/impresario/v1` + `crates/impresario-proto` | done | codegen verified via `cargo build` (vendored protoc fallback exercised) |
| `impresario-core` config (`config.rs`) | done | unit tests: spec examples, unknown keys w/ hints, ranges, endpoints per platform, options applicability, duplicates |
| `impresario-core` scanner/parser/AST | done | 111 lib tests; 94-case negative matrix (`tests/negative_cases.rs`, one per IMP-DSL code + edges); proptest never-panics suite (3 generators + nasty corpus); `insta` snapshots of the example ASTs and of a multi-error file |
| `impresario-core` semantic validation | done | unit tests: 008/009/012/013/014/017/019/020/021 with device awareness |
| `impresario-core` workspace loader | done | unit tests incl. duplicate scenes 006 |
| `impresario init` / `validate` / `parse` / `devices list` | done | CLI unit tests + integration test against the committed `examples/` workspace (`tests/parse_json.rs`: JSON stdout snapshot, exit codes) |
| `impresario devices check` | done | `tests/devices_check.rs`: DeviceInfo line + summary via the real binary, name selection, unknown name → 2, rejected handshake → 3, missing configs → 2; mock rows assert handshake/shutdown with no `Session` stream |
| `impresario run` (planner/executor) | done | planner: 20 unit tests (compilation, selectors, timeouts, secrets, bounds); executor: `tests/run_mock.rs` (16 scenarios against `MockAgent`: pass/fail exit codes, timeout+cancel by id, dropped-stream reconnect, retries/flaky with LAUNCH→RESTART, fail-fast across devices, parallel vs serial timing, `app_reset` never/per_act/per_play, tag/act selection, screenshot files, capability gaps, secret redaction, Ctrl-C → 130); CLI: `tests/run_cli.rs` (default reporters, html base64, no secret in any artifact, missing workspace → 2) |
| `impresario lsp` | done (spec 13) | in-process client over a real duplex stream with real `Content-Length` framing (`tests/common/lsp.rs`): `tests/lsp_diagnostics.rs` (open/change/close, overlay precedence over disk, the 150 ms debounce coalescing a burst, the UTF-16 vs UTF-8 offset difference after a surrogate pair, config-file diagnostics, device warnings switched by `didChangeConfiguration` in all three settings shapes, unsaved files outside the configured directories), `tests/lsp_features.rs` (every completion context of the spec table incl. device-filtered keys, hover selector tables, definitions, document symbols), `tests/lsp_watch.rs` (external edits, a `scenes` directory created after start, editor noise never triggers an analysis), `tests/lsp_cli.rs` (the real binary exits 0 when stdin closes); plus 23 unit tests (position/complete/nav/watch); `docs/LSP.md` and `editors/vscode` (client + TextMate grammar) shipped |
| `impresario mock-agent` | done (dev aid, decision I-77; not in the spec CLI list) | serves the scripted `MockAgent` on loopback (default 50071, `--port 0` prints the free port); `MockAgentBuilder::bind` unit path plus `tests/mock_cli.rs`: the real binary is spawned, a `web` device is pointed at its printed address, and the spawned `impresario run` exits 0 with `report.json` — the README quick start is under test |
| `impresario mcp` | done (dev/agent aid, decision I-80; not in the spec CLI list) | dual-era MCP server on stdio: modern stateless revision (2026-07-28 — per-request `_meta`, `server/discover`, `resultType`) plus the legacy `initialize` handshake (2025-11-25…2024-11-05, unknown → counter-offer); 8 tools (workspace info/validate, devices list/check, run, report get, artifacts list/get — screenshots as image content, artifact paths confined by `safe_join`, device-touching tools serialized by a process-wide lock) and `impresario://` resources (DSL/PARITY/DEVICES/MCP docs, workspace info); `tests/mcp_stdio.rs` spawns the real binary (legacy handshake run + artifact read, modern discover + protocol errors + diagnostics, failed run → screenshot image); 9 unit tests (dispatcher/eras/join/kinds); stdout is protocol-only while serving; **plus a scripted live smoke outside the test harness** (Python stdio client, both eras, real binary in the phone workspace, no device contact): the legacy handshake session listed the 8 tools + 5 resources, returned `workspace_info`/`devices_list`, listed the newest **real** run's artifacts (report.json, junit.xml, device screenshot) and returned the screenshot as an image block, then read the report; the modern `server/discover` session answered tools/report calls with `resultType: complete` and the unknown-tool error `-32602`; both exited 0 on stdin close; the hardware smoke followed the same day (see the MCP E2E row) |
| `impresario-clients` (transports, AgentClient) | web-family + adb + apple + mock done | `agent_client` (handshake/deadline/cancel/in-flight id/out-of-band screenshot), `convert`, `log` and `testing::MockAgent` unit-tested; `ConnectorTransport` verified via loopback mock agents; `AdbTransport` dials the on-device agent over the network first (`options.agent_address` → the serial's host → `ip route`) with `adb forward` as fallback, installs the three APKs in order and starts `am instrument` (I-62); `AppleTransport` is macOS-only (any other OS answers `DEVICE_UNAVAILABLE`): tvOS uses the configured `agent_address`, an iOS simulator the runner on loopback, an iOS device `iproxy <local>:<port> -u <udid>`, and the runner itself is started with `xcodebuild test-without-building -xctestrun $IMPRESARIO_XCTESTRUN -destination id=<udid> -only-testing <bundle>/AgentRunnerTests/testServe` (the `-iOS`/`-tvOS` bundle is pinned by both sides, I-89). The host-side clean-state seam (`reset_app_data` + `options.app_path`) runs before the session (I-84). Unit tests cover the argv builders, the reset tool choice and the non-macOS answers; the *live* path needs a Mac and stays a recorded gap |
| `impresario-connector` (raw CDP + es5 helper + page resolution + `launch:headless`) | done | `fixtures_suite.rs` (spec 10.3 acceptance): 1 end-to-end session on `fixtures/web/index.html` covering handshake/capabilities/device info, click/type/append/clear/longpress/select/focus/keypress/back, hidden/disabled/obscured/count/text/xpath checks, auto-wait, scroll/scrollto (window + `scroll.html` container), shadow contenteditable, app restart + helper re-injection, in-band + out-of-band screenshots (PNG header parsed), console log stream, cancel by id (CANCELLED + PROTOCOL for unknown), multi-session + shutdown + SessionClosed, TV D-pad session, `launch:headless` + `app launch`/`app restart`, auth (token metadata/body) and error mapping — all against real headless Chrome; 50 unit tests (keys, script ES5 lint, pages HTTP framing, png, focus, launch); `cargo test -p impresario-connector` green |
| `impresario-connector` (chromiumoxide backend) | done (feature `chromiumoxide-backend`) | same fixture suite passes with `cdp_backend=chromiumoxide`; 55 unit tests with the feature; `cargo clippy -p impresario-connector --all-targets --features chromiumoxide-backend -- -D warnings` clean |
| Android agent (Kotlin: `:agent` app, `:agent-instrumentation`, `:agent-core`, `:driver-uiautomator`) | done — verified on a real Android TV | `agents/android/tools/run-tv-suite.ps1` acceptance on the TCL TV over Wi-Fi (`docs/DEVICES.md` §2.5): driver class **26/26 in 160 s** and wire class **2/2 in 100 s**, each in its own fresh `am instrument` process (vendor freeze timer, I-68), zero `doFreeze` on impresario packages in either window; the wire class drives the published proto over real gRPC (full action/check matrix + cancel + shutdown), and the TV semantics are I-64..I-73 (D-pad, leanback IME/hint handling, `longpress` key hold, `scrollto` budget, `background`); the touch phone path is verified live too (Galaxy A51, Android 13, Wi-Fi — `docs/DEVICES.md` §2.7, `run 20261018-103702-ee29`), including the I-75 `scrollto` end-of-container fix; app-data reset hardening landed 2026-10-19 (I-82: `clear_data` honored on `LAUNCH`, `pm clear` output verified, a refused clear degrades to a WARN + force-stop cold start — the Realme RMX3997 ROM denies shell `CLEAR_APP_USER_DATA`; live MCP re-run `20261019-064750-dca4` passed) |
| Apple agent (Swift: `AgentCore`, `ImpresarioWire`, `AgentService` + the `agents/apple/Xcode` runner project) | done in code; the Swift package is built + unit-tested on this host, the Xcode targets are unverified | `swift build` + `swift test` green through WSL (Ubuntu 22.04, Swift 6.4.1; `docs/DEVICES.md` §3.1): **135 tests, 0 failures** in one bundle — 32 `AgentService` (loopback suites against a real grpc-swift server on an ephemeral port with the generated client: handshake/device info/capabilities, the whole action + check matrix over the wire, cancel, heartbeats, `SessionClosed`, token auth, `StreamLogs`, second-session rejection; plus the protocol conversion and element-type map 69/69) and 103 `AgentCore` (selector parsing incl. predicates, the 6.2 key table, the 6.3 D-pad navigator, ordered dispatcher, command executor on a fake driver incl. deadlines/cancel/hidden/`clear_data` WARN, PNG builder, state/log bus). Shutdown ordering is I-90 (found with `SWIFTNIO_STRICT=1`, which crashes on any task scheduled after a loop stopped), TV semantics I-86/I-87, clean state I-84. The Xcode deliverables (`project.yml`, host apps, stub fixture, UI-test runner + ObjC shim) are written but cannot be compiled here: `xcodegen generate`, the simulator/device runs, the tvOS focus engine and the local-network prompt stay on the macOS checklist (`agents/apple/README.md`, `docs/DEVICES.md` §3.2, gaps 1–2) |
| Live E2E (master CLI → connector → headless Chrome) | verified | `impresario run` on a scratch workspace (`launch:headless`, `file:///…/fixtures/web/index.html`): connector auto-spawned, play passed 8/8 steps in 3.1 s, exit 0; `report.json`/`junit.xml`/PNG screenshot written (image shows the driven fixture page); one environment pitfall recorded: port 50051 was already held by `multipassd`, so the device config pins `agent_address: 127.0.0.1:47654` |
| Live E2E (master CLI → Android agent → TCL Android TV) | verified | `impresario run` on the scratch TV workspace over Wi-Fi: agent auto-installed + started, network dial + handshake + session, full smoke play green — `run 20261018-094855-ddb9: passed (1m46s, exit 0)`, 1 device, 1 play, 2 acts, 89 steps, 0 failed (Act 1 scenario tour 1m02s incl. D-pad `scrollto` row 29/0, text entry, selection, key log; Act 2 app-control lifecycle 19.5s incl. `background`/`launch`, `restart`, `terminate`); agent process ≈ 100 s < the ~223 s freeze budget (I-68). First attempt hung silently before the play — fixed as I-74 (lazy `StreamLogs` headers vs a tonic client that awaits the open); re-verified after I-75: `run 20261018-104405-4f10` (1m42s, 89 steps, 0 failed) |
| Live E2E (master CLI → Android agent → Android phone) | verified | `impresario run` on the scratch phone workspace over Wi-Fi (Galaxy A51, Android 13, device `pocket_phone`, `docs/DEVICES.md` §2.7): agent + stub auto-installed, network dial + handshake + session, full touch smoke play green — `run 20261018-103702-ee29: passed (1m53s, exit 0)`, 1 device, 1 play, 2 acts, 86 steps, 0 failed (Act 1 scenario tour 1m19s incl. touch `scroll`/`swipe up`/`swipe down`/`scrollto` both ways, text entry with `key backspace` echo checks, selection, `key back`/`key home`, device info `tv=false`; Act 2 app-control lifecycle 16.3s). The first attempts exposed the I-75 `scrollto` bug (end-of-container signal) |
| Live E2E via MCP (MCP server → Android agent → Realme phone) | verified | the whole loop through `impresario mcp` alone — a small-agent smoke with no CLI and no hand-run adb (the server's own prepare installed the APKs and started the agent): `devices_check` reported RMX3997 / Android 16 / UiAutomator2 / 720×1604, then `run` of the bare play name `mobile_smoke.play` passed — `run 20261019-061207-92f9: passed (1m43s, exit 0)`, 2 acts, 86 steps, 0 failed (touch `scroll`/`swipe`/`scrollto` both ways, text entry with echo checks, selection, `back`/`home`, device info `tv=false`; lifecycle `background`/`launch`, `restart`, `terminate`); the screenshot and report came back through `impresario_artifact_get`/`impresario_report_get` and stdout stayed protocol-only all session. The first attempt exposed the bare-play-name + refusal-detail gap fixed the same day (I-80) |
| Reporters (console/junit/json/html) | done | `tests/reporters.rs` insta snapshots (console plain + color, junit.xml, report.json, report.html with base64 embed and missing-file fallback) |
| Docs (`docs/*.md`, examples, fixtures) + `README.md` | done (DSL, PROTOCOL, DECISIONS, DEVICES, STATUS, ERRORS, LSP, MCP, PARITY, README) | DSL.md covers syntax, semantics and selectors; PROTOCOL.md documents the wire contract as implemented (sequences, 7.2 rules, error-code table, endpoint resolution, I-74 lazy log stream); PARITY.md is generated from the planner's support tables and guarded by `tests/parity_doc.rs` (`IMPRESARIO_PARITY_BLESS=1` rewrites); DECISIONS.md holds I-01..I-90 (I-83..I-89: the Apple agent, its clean-state reset, the XCUITest safety shim and the WLAN/permission handling; I-90: the Swift service's shutdown ordering); DEVICES.md records per-platform setup and hardware evidence (§3 covers the Apple package and the Xcode commands); LSP.md documents the language server; MCP.md documents the stdio MCP server (tools, resources, per-client wiring, I-80); README.md's quick start is executed by `tests/mock_cli.rs` (init/validate/run against `mock-agent`) |
| CI workflows | committed, not yet executed | `.github/workflows/ci.yml`: `rust` job on `ubuntu-latest` (`fmt --check`, `clippy --workspace --all-targets -- -D warnings`, `test --workspace`; the runner ships google-chrome, so the connector fixture suite runs for real), `android` job (`setup-java` 17 + `setup-android`, `gradlew --no-daemon test`; `gradlew` is committed non-executable → `chmod +x`), and `apple` job (`setup-swift` 6.4.1 + `zlib1g-dev`, then `swift package resolve`/`swift build`/`swift test` in `agents/apple` — the Linux-buildable half of the Apple agent; the Xcode runner targets cannot build there). This host has **no GitHub remote**, so Actions has not executed the workflow yet (recorded gap); the underlying commands are green locally and the document itself is parse-checked by `tests/ci_workflow.rs` (valid YAML + the exact gate commands) |

## Known gaps and risks

1. Apple agent: the Swift **package** (`AgentCore`/`ImpresarioWire`/`AgentService`)
   is built and unit-tested on this host through WSL (see above). What has
   *never* run here is everything that needs macOS/Xcode: `xcodegen generate`,
   the host/stub/UI-test targets, the `testServe()` runner on a simulator or
   device, the tvOS focus engine and the local-network permission. Those are
   the manual checklists of spec 15 (`agents/apple/README.md`,
   `docs/DEVICES.md` §3.2).
2. Phase 0 spike 2 (NIO gRPC in XCUITest): unverified on this host (see
   above); D-18 fallback design stays available if the spike fails on a Mac.
3. `impresario-connector` is verified against headless Chrome (installed on
   this machine) via the fixture suite (I-47); Tizen/webOS-specific CDP
   quirks and the `launch` path on a real TV remain untested on hardware.
4. Tizen/webOS hardware is not available; the connector's CDP layer is tested
   against Chrome and the protocol-relevant fake (recorded in tests).
5. The `cargo-fuzz` target (`fuzz/fuzz_targets/parse.rs`) needs a nightly
   toolchain and `cargo-fuzz`, which are not installed here; it is shipped
   unverified. The stable-toolchain proptest suite
   (`crates/impresario-core/tests/parser_fuzz.rs`) covers the same
   never-panics property on every `cargo test`.
6. TV sessions run against a vendor that freezes long-lived background
   processes: the agent must stay under ~223 s per process (I-68), so the
   acceptance runner gives each test class its own process and the master
   E2E play is kept short (verified: the 89-step smoke play ran 1m46 s, ≈ 100 s of agent lifetime); a frozen agent needs a force-stop (the runner's
   stalls/retries exist exactly for this).
7. CI has not run anywhere yet: this checkout has no GitHub remote, so
   `.github/workflows/ci.yml` is committed but unexecuted on Actions
   (see the CI row above). The local gates it encodes are green: the Rust
   commands here, `gradlew test` on this host, and the Apple job's
   `swift package resolve`/`build`/`test` through WSL (its `zlib1g-dev`
   step mirrors the dependency the WSL build needed, `docs/DEVICES.md` §3.1).
8. Stub spacing on TV: the phone rhythm (one 10 dp vertical rhythm through
   `Ui.addStacked`, I-81) was verified on the A51, but the same screens still
   look too airy on a TV ("too spaced between buttons", user report). The fix
   is a TV-profile gap in `agents/android/agent-target-stub/.../Ui.kt` plus a
   `tv_smoke` re-run on the TV. Blocked: no TV or phone is reachable from the
   current Wi-Fi here (host is on `10.102.93.101`; the known devices were on
   `192.168.93.x`), so it stays open rather than shipping an unverified
   change (the phone UI must not regress).
