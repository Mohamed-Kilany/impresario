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
| `impresario-core` config (`config.rs`) and apps (`apps.rs`) | done | unit tests: split device/app schema, spec examples, unknown keys w/ hints, ranges, endpoints/auth/options per platform, duplicate devices and apps, app platform entries, global + workspace merging; the real workspace tests consume the merged layers |
| `impresario device` / `app` / `migrate` | done (TASK-2 phases 1B + 3; wizard/discovery/pairing I-124) | global-by-default definitions with `--workspace` scope, append-preserving edits (I-103), list/show/check/edit/rename/remove/import/export; `migrate` dry-run/idempotency/.bak/mixed-file refusal and the legacy endpoint/app split (I-104); `tests/definitions_cli.rs` (7 tests) covers both layers, duplicate/bad fields, platform-scoped options, app check failures and the migration cases. Phase 3 adds `device discover`, `device pair`, the interactive `device wizard` (scope/platform/name/route/port/labels/check/preview, back/cancel with no writes), Android phone/TV role detection, and six scripted wizard tests; live sessions wrote phone, TV and web definitions, and a fresh-key wizard pairing passed `device check`. `app check` still verifies ids/package existence only; installed state, install/uninstall and signatures remain later work |
| `impresario-core` scanner/parser/AST | done | core library suite (165 tests); negative matrix (`tests/negative_cases.rs`) plus dedicated theatrical fixtures for 030..050; table-form scenes, `.theatre` stages, `Examples:` tables and the legacy compatibility forms (I-110); proptest never-panics suite (3 generators + nasty corpus); `insta` snapshots of the example ASTs, theatre AST and a multi-error file |
| `impresario-core` semantic validation | done | unit tests: 008/009/012/013/014/017/019/020/021 with device awareness, plus theatrical resolution (031/032/034/036/037/040/041) and workspace lints 042..050 (unused scene/piece/stage/theatre/var/example column, duplicate `Var:`, Act platform coverage, required parameter never supplied) |
| `impresario-core` workspace loader | done | unit tests incl. duplicate scenes 006 |
| `impresario workspace new --example` / `validate` / `parse` / `device list` | done | CLI unit tests + integration test against the committed `examples/` workspace (`tests/parse_json.rs`: JSON stdout snapshot, exit codes); `workspace new` writes the split device/app example and refuses to overwrite without `--force` (I-102/I-103; `init` is gone); `validate` now resolves `App:` headers against the merged apps layer and shows the app count |
| `impresario device check` (alias `devices check`) | done | `tests/devices_check.rs`: DeviceInfo line + summary via the real binary, name selection, unknown name → 2, rejected handshake → 3, missing configs → 2; mock rows assert handshake/shutdown with no `Session` stream. Non-interactive; it does not take an app (it probes the device, not an app) |
| `impresario doctor` | done (TASK-2 Phase 3, I-124) | per-platform report (`ready`/`degraded`/`unavailable`/`planned`) with what is needed, what was found, the chosen rung and the exact fix; configured devices are probed and reported individually; managed browser and host-key status are included. `--json` emits the stable machine shape; it never downloads or installs; exit is nonzero only on `unavailable`/configured-device failure (unsupported platforms without a device stay `planned`). Live with `adb` absent from `PATH`: Android, Android TV and web `ready`, exit 0 (`target/phase3-doctor.json`). Unit tests cover status formatting, JSON shape and exit semantics |
| `impresario tools` (managed Chromium) | done (TASK-2 Phase 3, I-123) | `tools list`, `tools install chromium`, `tools remove chromium`; install requires interactive consent or `--yes`, downloads only the pinned Chrome for Testing 155.0.8059.84 archive for the current platform, verifies its SHA-256 before extraction (the pinned size is shown for consent), stages under `~/.impresario/tools/chromium/<version>/<platform>`, and grants the two Windows AppContainer RX ACEs needed by the sandbox. `doctor` only reports. Verified with a managed headless run without `--no-sandbox` and a full 9/9 web E2E; Windows-only ACL path live-tested, other platforms pinned but not downloaded here (gap 16) |
| `impresario run` (planner/executor) | done | planner: 20 unit tests (compilation, selectors, timeouts, secrets, bounds); executor: `tests/run_mock.rs` (16 scenarios against `MockAgent`: pass/fail exit codes, timeout+cancel by id, dropped-stream reconnect, retries/flaky with LAUNCH→RESTART, fail-fast across devices, parallel vs serial timing, `app_reset` never/per_act/per_play, tag/act selection, screenshot files, capability gaps, secret redaction, Ctrl-C → 130); CLI: `tests/run_cli.rs` (default reporters, html base64, no secret in any artifact, missing workspace → 2) |
| `impresario lsp` | done (spec 13; Phase 2 theatre additions I-116) | in-process client over a real duplex stream with real `Content-Length` framing (`tests/common/lsp.rs`): `tests/lsp_diagnostics.rs` (open/change/close, overlay precedence over disk, the 150 ms debounce coalescing a burst, the UTF-16 vs UTF-8 offset difference after a surrogate pair, config-file diagnostics, device warnings switched by `didChangeConfiguration` in all three settings shapes, unsaved files outside the configured directories), `tests/lsp_features.rs` (every completion context of the spec table incl. device-filtered keys, hover selector tables, definitions, document symbols), `tests/lsp_watch.rs` (external edits, a `scenes` directory created after start, editor noise never triggers an analysis), `tests/lsp_cli.rs` (the real binary exits 0 when stdin closes); plus 24 unit tests (position/complete/nav/watch); `docs/LSP.md` and `editors/vscode` (client + TextMate grammar) shipped |
| `impresario mock-agent` | done (dev aid, decision I-77; not in the spec CLI list) | serves the scripted `MockAgent` on loopback (default 50071, `--port 0` prints the free port); `MockAgentBuilder::bind` unit path plus `tests/mock_cli.rs`: the real binary is spawned, a `web` device is pointed at its printed address, and the spawned `impresario run` exits 0 with `report.json` — the README quick start is under test |
| `impresario config` / `workspace` + `impresario-core::settings` | done (TASK-2 phase 1A) | `settings.rs` is the single schema (scope, type, default, docs) and resolves the workspace layer over the global layer; `home.rs` resolves `~/.impresario` (or `IMPRESARIO_HOME`) and `workspace.rs` resolves the root from the flag, environment, marker, global `workspace` key or cwd. `config list|get|set|unset|edit|path|explain` and `workspace current|list|add|remove|use|new` are wired with `--global`/`--workspace`, `--explain`, JSON output and atomic writes; `set`/`unset`/`edit` re-parse the whole file and refuse to make it invalid, and `edit` restores the old bytes and offers to reopen. `tests/config_cli.rs` (6 tests) covers lossless set/unset, refusals with `IMP-CFG-*`, precedence/explain/JSON, registry operations and the editor restore path; `docs/CONFIG.md` is generated from the schema and guarded by `tests/config_doc.rs`; I-100 (flag placement) and I-101 (yaml-edit list workaround) record the two deviations. The LSP and MCP now load the same layered settings |
| `impresario mcp` | done (dev/agent aid, decision I-80; not in the spec CLI list) | dual-era MCP server on stdio: modern stateless revision (2026-07-28 — per-request `_meta`, `server/discover`, `resultType`) plus the legacy `initialize` handshake (2025-11-25…2024-11-05, unknown → counter-offer); 8 tools (workspace info/validate, devices list/check, run, report get, artifacts list/get — screenshots as image content, artifact paths confined by `safe_join`, device-touching tools serialized by a process-wide lock) and `impresario://` resources (DSL/PARITY/DEVICES/MCP docs, workspace info); `tests/mcp_stdio.rs` spawns the real binary (legacy handshake run + artifact read, modern discover + protocol errors + diagnostics, failed run → screenshot image); 9 unit tests (dispatcher/eras/join/kinds); stdout is protocol-only while serving; **plus a scripted live smoke outside the test harness** (Python stdio client, both eras, real binary in the phone workspace, no device contact): the legacy handshake session listed the 8 tools + 5 resources, returned `workspace_info`/`devices_list`, listed the newest **real** run's artifacts (report.json, junit.xml, device screenshot) and returned the screenshot as an image block, then read the report; the modern `server/discover` session answered tools/report calls with `resultType: complete` and the unknown-tool error `-32602`; both exited 0 on stdin close; the hardware smoke followed the same day (see the MCP E2E row) |
| `impresario-clients` (transports, AgentClient) | web-family + native adb + apple + mock done (Android tool-free; Apple D-19 gap) | `agent_client` (handshake/deadline/cancel/in-flight id/out-of-band screenshot), `convert`, `log` and `testing::MockAgent` unit-tested; `ConnectorTransport` verified via loopback mock agents; `AdbTransport` is native (I-121) and starts no `adb` binary: it dials the on-device agent over the network first (`connection.endpoint` → the serial's host → `ip route`) with a direct native `tcp:` forward fallback, sync-installs the three APKs in order, starts `am instrument` and streams host logcat/screencap (I-62/I-121); `AppleTransport` is macOS-only (any other OS answers `DEVICE_UNAVAILABLE`): tvOS uses the configured `connection.endpoint`, an iOS simulator the runner on loopback, an iOS device `iproxy <local>:<port> -u <udid>`, and the runner itself is started with `xcodebuild test-without-building -xctestrun $IMPRESARIO_XCTESTRUN -destination id=<udid> -only-testing <bundle>/AgentRunnerTests/testServe` (the `-iOS`/`-tvOS` bundle is pinned by both sides, I-89). The host-side clean-state seam (`reset_app_data` + the app platform's `package`) runs before the session (I-84). Android and web are now tool-free; `AppleTransport` still shells out, which is the remaining scope of D-19 (gap 10). Unit tests cover the argv builders, the reset tool choice and the non-macOS answers; the *live* path needs a Mac and stays a recorded gap |
| `impresario-connector` (raw CDP + es5 helper + page resolution + `launch:headless`) | done | `fixtures_suite.rs` (spec 10.3 acceptance): 1 end-to-end session on `fixtures/web/index.html` covering handshake/capabilities/device info, click/type/append/clear/longpress/select/focus/keypress/back, hidden/disabled/obscured/count/text/xpath checks, auto-wait, scroll/scrollto (window + `scroll.html` container), shadow contenteditable, app restart + helper re-injection, in-band + out-of-band screenshots (PNG header parsed), console log stream, cancel by id (CANCELLED + PROTOCOL for unknown), multi-session + shutdown + SessionClosed, TV D-pad session, `launch:headless` + `app launch`/`app restart`, auth (token metadata/body) and error mapping — all against real headless Chrome; 50 unit tests (keys, script ES5 lint, pages HTTP framing, png, focus, launch); `cargo test -p impresario-connector` green |
| `impresario-connector` (chromiumoxide backend) | done (feature `chromiumoxide-backend`) | same fixture suite passes with `cdp_backend=chromiumoxide`; 55 unit tests with the feature; `cargo clippy -p impresario-connector --all-targets --features chromiumoxide-backend -- -D warnings` clean |
| Android agent (Kotlin: `:agent` app, `:agent-instrumentation`, `:agent-core`, `:driver-uiautomator`) | done — verified on a real Android TV | `agents/android/tools/run-tv-suite.ps1` acceptance on the TCL TV over Wi-Fi (`docs/DEVICES.md` §2.5): driver class **26/26 in 160 s** and wire class **2/2 in 100 s**, each in its own fresh `am instrument` process (vendor freeze timer, I-68), zero `doFreeze` on impresario packages in either window; the wire class drives the published proto over real gRPC (full action/check matrix + cancel + shutdown), and the TV semantics are I-64..I-73 (D-pad, leanback IME/hint handling, `longpress` key hold, `scrollto` budget, `background`); the touch phone path is verified live too (Galaxy A51, Android 13, Wi-Fi — `docs/DEVICES.md` §2.7, `run 20261018-103702-ee29`), including the I-75 `scrollto` end-of-container fix; app-data reset hardening landed 2026-10-19 (I-82: `clear_data` honored on `LAUNCH`, `pm clear` output verified, a refused clear degrades to a WARN + force-stop cold start — the Realme RMX3997 ROM denies shell `CLEAR_APP_USER_DATA`; live MCP re-run `20261019-064750-dca4` passed) |
| Apple agent (Swift: `AgentCore`, `ImpresarioWire`, `AgentService` + the `agents/apple/Xcode` runner project) | done in code; the Swift package is built + unit-tested on this host, the Xcode targets are unverified | `swift build` + `swift test` green through WSL (Ubuntu 22.04, Swift 6.4.1; `docs/DEVICES.md` §3.1): **135 tests, 0 failures** in one bundle — 32 `AgentService` (loopback suites against a real grpc-swift server on an ephemeral port with the generated client: handshake/device info/capabilities, the whole action + check matrix over the wire, cancel, heartbeats, `SessionClosed`, token auth, `StreamLogs`, second-session rejection; plus the protocol conversion and element-type map 69/69) and 103 `AgentCore` (selector parsing incl. predicates, the 6.2 key table, the 6.3 D-pad navigator, ordered dispatcher, command executor on a fake driver incl. deadlines/cancel/hidden/`clear_data` WARN, PNG builder, state/log bus). Shutdown ordering is I-90 (found with `SWIFTNIO_STRICT=1`, which crashes on any task scheduled after a loop stopped), TV semantics I-86/I-87, clean state I-84. The Xcode deliverables (`project.yml`, host apps, stub fixture, UI-test runner + ObjC shim) are written but cannot be compiled here: `xcodegen generate`, the simulator/device runs, the tvOS focus engine and the local-network prompt stay on the macOS checklist (`agents/apple/README.md`, `docs/DEVICES.md` §3.2, gaps 1–2) |
| Live E2E (master CLI → connector → headless Chrome) | verified | `impresario run` on a scratch workspace (`launch:headless`, `file:///…/fixtures/web/index.html`): connector auto-spawned, play passed 8/8 steps in 3.1 s, exit 0; `report.json`/`junit.xml`/PNG screenshot written (image shows the driven fixture page); one environment pitfall recorded: port 50051 was already held by `multipassd`, so the device config pinned the connector at `127.0.0.1:47654` (now `options.connector_address`) |
| Live E2E (master CLI → Android agent → TCL Android TV) | verified | `impresario run` on the scratch TV workspace over Wi-Fi: agent auto-installed + started, network dial + handshake + session, full smoke play green — `run 20261018-094855-ddb9: passed (1m46s, exit 0)`, 1 device, 1 play, 2 acts, 89 steps, 0 failed (Act 1 scenario tour 1m02s incl. D-pad `scrollto` row 29/0, text entry, selection, key log; Act 2 app-control lifecycle 19.5s incl. `background`/`launch`, `restart`, `terminate`); agent process ≈ 100 s < the ~223 s freeze budget (I-68). First attempt hung silently before the play — fixed as I-74 (lazy `StreamLogs` headers vs a tonic client that awaits the open); re-verified after I-75: `run 20261018-104405-4f10` (1m42s, 89 steps, 0 failed) |
| Live E2E (master CLI → Android agent → Android phone) | verified | `impresario run` on the scratch phone workspace over Wi-Fi (Galaxy A51, Android 13, device `pocket_phone`, `docs/DEVICES.md` §2.7): agent + stub auto-installed, network dial + handshake + session, full touch smoke play green — `run 20261018-103702-ee29: passed (1m53s, exit 0)`, 1 device, 1 play, 2 acts, 86 steps, 0 failed (Act 1 scenario tour 1m19s incl. touch `scroll`/`swipe up`/`swipe down`/`scrollto` both ways, text entry with `key backspace` echo checks, selection, `key back`/`key home`, device info `tv=false`; Act 2 app-control lifecycle 16.3s). The first attempts exposed the I-75 `scrollto` bug (end-of-container signal) |
| Live E2E via MCP (MCP server → Android agent → Realme phone) | verified | the whole loop through `impresario mcp` alone — a small-agent smoke with no CLI and no hand-run adb (the server's own prepare installed the APKs and started the agent): `devices_check` reported RMX3997 / Android 16 / UiAutomator2 / 720×1604, then `run` of the bare play name `mobile_smoke.play` passed — `run 20261019-061207-92f9: passed (1m43s, exit 0)`, 2 acts, 86 steps, 0 failed (touch `scroll`/`swipe`/`scrollto` both ways, text entry with echo checks, selection, `back`/`home`, device info `tv=false`; lifecycle `background`/`launch`, `restart`, `terminate`); the screenshot and report came back through `impresario_artifact_get`/`impresario_report_get` and stdout stayed protocol-only all session. The first attempt exposed the bare-play-name + refusal-detail gap fixed the same day (I-80) |
| Live E2E (Phase 3 no-tool runs: web + phone + TV) | verified | After the native ADB and managed-browser changes, the same migrated workspaces were rerun with `PATH` reduced to `C:\Windows\System32;C:\Windows` (`Get-Command adb` empty) and `IMPRESARIO_HOME=target\phase3-home`. Headless Chrome via managed Chromium `run 20261020-154341-d767` passed 9/9 steps in 2.42 s; Realme RMX3997 `run 20261020-153947-986f` passed 2 acts/86 steps (1m43s); Smart TV Pro `run 20261020-154152-216d` passed 2 acts/89 steps (1m34s). The wizard wrote live phone/TV/web definitions, and a fresh Impresario key paired through the Android pairing-code screen in the wizard (`wizard_pair_phone`, new home) and then passed `device check`. `impresario doctor --json` exited 0 with `android`, `androidtv` and `web` all `ready`, managed Chromium found without PATH, and `adb on PATH` reported as not present/not needed. Logs and doctor output are under `target\phase3-*.json/txt` on this host (not committed). |
| Live E2E (Phase 2 theatrical DSL, migrated workspaces: web + phone + TV) | verified | The three legacy Phase-1 scratch workspaces were copied and converted with `impresario migrate` on 2026-10-20: split devices/apps, table scenes, generated `.theatre` files, and plays rewritten to `Perform:`. All three then validated with **0 warnings** and were `fmt --check`-clean; a second `migrate` was a no-op. The phone scene needed one cleanup after migration: four TV-only pieces (`KeysRow`, `Alpha`, `Bravo`, `KeyLog`) that the shared legacy `Pieces:` block declared but the touch play never used were removed to clear `IMP-DSL-043`; this did not touch the generated theatre and the final run used the cleaned workspace. Real runs: headless Chrome `run 20261020-133846-23c0` passed 9/9 steps in 1.87 s; Realme RMX3997 `run 20261020-134217-2dfe` passed 2 acts/86 steps in 1m35s; Smart TV Pro `run 20261020-133548-61d9` passed 2 acts/89 steps in 1m38s. All reports are schema v2 with stage spans and JUnit `stage_sources` pointing into the generated theatres. Workspaces/logs are under `%TEMP%\impresario-phase2-e2e` on this host (not committed). Two environment quirks: `multipassd` owns port 50051, so the web run pinned `options.connector_address: 127.0.0.1:47654`; the TV agent APK had been installed only for secondary user 10, fixed by reinstalling all three APKs with `install --user 0` (`docs/DEVICES.md` §2.6) |
| Reporters (console/junit/json/html) | done | `tests/reporters.rs` insta snapshots (console plain + color, junit.xml, report.json schema v2, report.html with base64 embed and missing-file fallback); Act example display names, stage source spans and secret-key masking are covered |
| `impresario fmt` / DSL migration | done | `impresario_core::fmt` (idempotence property test, comments/blanks, Unicode-width table alignment, absent cells) and `impresario_core::migrate` (table scenes, generated theatres, duplicate suffixes, idempotence, fail-closed on invalid generated text) have unit tests; `tests/golden_dsl.rs` covers the formatter golden files and a complete legacy migration snapshot; `tests/definitions_cli.rs` drives `migrate --dry-run`, `.bak`, idempotence and generated theatre output; committed `examples/` are migrated and `fmt --check`-clean |
| `impresario run --arg` / theatrical runtime | done | planner tests cover `--arg` precedence (CLI > Examples > default), unknown/malformed arguments (IMP-DSL-032), nested stage expansion, cycles/depth and report display data; MCP `args` maps to the same code path |
| Docs (`docs/*.md`, examples, fixtures) + `README.md` | done (DSL, PROTOCOL, DECISIONS, DEVICES, STATUS, ERRORS, LSP, MCP, PARITY, CONFIG, SPIKES, THIRD_PARTY, README) | DSL.md covers syntax, semantics and selectors; PROTOCOL.md documents the wire contract as implemented (sequences, 7.2 rules, error-code table, endpoint resolution, I-74 lazy log stream); PARITY.md is generated from the planner's support tables and guarded by `tests/parity_doc.rs` (`IMPRESARIO_PARITY_BLESS=1` rewrites); CONFIG.md is generated from `settings::docs_table()` and guarded by `tests/config_doc.rs`; DECISIONS.md holds I-01..I-124 (I-91..I-98: the phase-0 spikes; I-99: webOS; I-100: `--workspace` placement; I-101: the yaml-edit list workaround; I-102: `init` replaced by `workspace new`, superseding I-11; I-103..I-108: the device/app split, migration, app binding, diagnostics and deferred wizard; I-109: the TV stub gap; I-110..I-119: theatrical DSL compatibility, formatter, Examples/`--arg`, report v2, migration, workspace scans, LSP, qualified references/interpolation scope, lints 049/050 and reporter/migration fail-closed behavior; I-120: Phase 2 live acceptance and spacing verification; I-121..I-124: native Android/libadb and live pairing, the Windows key-store workaround, managed Chromium, wizard/discovery/pair and doctor); SPIKES.md carries the S-1..S-8 evidence and verdicts, updated with the Phase 3 S-1 outcome; THIRD_PARTY.md records every dependency/reference/managed-payload license; DEVICES.md records per-platform setup and hardware evidence, including the Phase 3 native ADB, pairing, wizard and managed-browser sections (§3 covers the Apple package and the Xcode commands); LSP.md documents the language server; MCP.md documents the stdio MCP server (tools, resources, per-client wiring, I-80); README.md's quick start is executed by `tests/mock_cli.rs` (workspace create/validate/run against `mock-agent`) |
| CI workflows | committed, not yet executed | `.github/workflows/ci.yml`: `rust` job on `ubuntu-latest` (`fmt --check`, `clippy --workspace --all-targets -- -D warnings`, `test --workspace`; the runner ships google-chrome, so the connector fixture suite runs for real), `android` job (`setup-java` 17 + `setup-android`, `gradlew --no-daemon test`; `gradlew` is committed non-executable → `chmod +x`), and `apple` job (`setup-swift` 6.4.1 + `zlib1g-dev`, then `swift package resolve`/`swift build`/`swift test` in `agents/apple` — the Linux-buildable half of the Apple agent; the Xcode runner targets cannot build there). This host has **no GitHub remote**, so Actions has not executed the workflow yet (recorded gap); the underlying commands are green locally and the document itself is parse-checked by `tests/ci_workflow.rs` (valid YAML + the exact gate commands) |

## TASK-2 phase status

| Phase | State | Evidence |
|---|---|---|
| 0 | settled (S-1..S-8) | Verdicts and ladder rungs in `docs/SPIKES.md`; licenses in `docs/THIRD_PARTY.md`. Go: Android (S-1), Apple (S-2), webOS (S-3), Tizen (S-4), CLI/TUI/config (S-5), signing (S-6/S-7), WASM editor (S-8). Hardware validation is scoped per spike. |
| 1A | done | `home.rs`, layered `settings.rs`, workspace resolution, `config`/`workspace` commands, generated `docs/CONFIG.md`; full Rust gate green (commit `61751f8`). |
| 1B | done (wizard closed by Phase 3) | Split device/app models and merged layers, endpoint/auth/options derivation, `device`/`app` commands, `migrate` with `.bak`/dry-run/idempotency, plan/exec/transport/MCP/LSP app awareness, examples updated. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` are green. The interactive wizard and discovery/pairing were deferred here and are now implemented in Phase 3 (I-124, gap 9 closed); installed/signature `app check` remains later work. |
| 2 | done (live E2E verified 2026-10-20) | Table-form scenes, `.theatre` stages, `Perform:` / nested expansion, Act parameters, `Examples:` one-row-per-execution, `${param.*}`/`${example.*}`, `--arg` precedence, formatter, migration, lints 042..050, report schema v2, stage source locations in console/HTML/JUnit, qualified `Theatre.Stage` references and LSP theatre/formatting support. Legacy forms stay accepted (I-110), all choices are recorded as I-110..I-120. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` are green on this host; the migrated `examples/` validate and are `fmt --check`-clean. The exit criterion that required a **live web + phone + TV E2E with migrated workspaces** is met: the three Phase-1 legacy workspaces were migrated, validated with 0 warnings, and run green on headless Chrome, the Realme phone and the Smart TV Pro (run IDs in the Live E2E row and gap 11 closure). Code/docs are committed as `b551ec1`/`f5a11d0`; the hardware close-out is recorded here. |
| 3 | done (live Android/web verified 2026-10-20) | Native Android transport on `libadb` 0.6.0 with USB/TCP/mDNS discovery, wireless pairing, shell v2/exec streams, sync APK install, instrumentation output, forwarding fallback, logcat, screencap, `pm clear`/force-stop, `getprop` and `ip route` - no `adb` process (I-121). Managed Chromium download/verification with consent and Windows sandbox ACLs (I-123), `doctor` human/JSON output, and the full device wizard/discover/pair surface (I-124). Live acceptance: web/phone/TV E2E and a fresh-key wizard pairing with `adb` absent from `PATH`; `doctor --json` exit 0 with Android, Android TV and web `ready` (`target/phase3-doctor.json`). `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` are green. The native `tcp:` forwarding fallback was exercised live on the phone by setting an unreachable `connection.endpoint`; after the 30 s network wait, `device check` used the forward and passed. USB data and non-Windows Chromium archives remain unverified (gaps 10, 15, 16). |

## Known gaps and risks

Items 8, 9 and 11 were closed on 2026-10-20; they stay in place, clearly
marked, so the numbers used elsewhere remain stable.

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
   this machine) via the fixture suite (I-47). The attach path now waits for the
   page to leave `loading` and for the actual scroll event (fixing the
   under-load `fixture_suite_raw_backend` race); Tizen/webOS-specific CDP
   quirks and the `launch` path on a real TV remain untested on hardware.
4. Tizen and webOS hardware is not available. Tizen has the Rust sdb client +
   in-process mock (S-4/I-93); webOS has the `russh` 0.63.3 + `russh-sftp`
   stack building on Windows and passing a loopback probe (S-3/I-99), and its
   protocol facts are cited, but no TV has accepted the key, an SFTP upload or
   a luna install. The connector's CDP layer is tested against Chrome.
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
8. **Stub spacing on TV — closed 2026-10-20 (I-120).** The phone keeps the
   I-81-verified 10 dp rhythm and Android TV selects 6 dp (I-109), as shown by
   the phone `stub_home.png` from run `20261020-134217-2dfe` and a fresh TV home
   capture at `%TEMP%\impresario-phase2-e2e\tv-home.png` from the same build.
   The TV is visibly tighter without crowding, and the phone layout is
   unchanged. The previous hardware blocker is gone.
9. **Phase 1B wizard/discovery deferral - closed 2026-10-20 (I-124).**
   `device discover`, `device pair` and the interactive `device wizard` now
   exist; the wizard has defaults, immediate validation,
   back/cancel without writes, YAML/scope preview and non-interactive flag
   forms. Live sessions wrote phone, TV and web definitions, and a fresh-key
   Android pairing driven through the wizard passed `device check`. The
   ratatui device screen is part of the Phase 4 TUI, not this CLI wizard.
   `app check`
   still verifies ids and package existence only: `app install`/`uninstall`,
   installed state, code signature and re-signing remain later work.
   `migrate` still handles the workspace devices file only, not app data in a
   legacy global `~/.impresario/config.yaml`.
10. D-19 is **applied to Android and web as of Phase 3**, and remains a
    known, explicit deviation for **Apple**. Android no longer starts `adb`
    (I-121) and the web path uses the managed Chromium download/lookup (I-123);
    `doctor` reports `adb on PATH` as not present/not needed. `AppleTransport`
    still uses `xcodebuild`, `iproxy`, `simctl`/`devicectl`/`ideviceinstaller`
    (I-95), and the webOS/Tizen SSH/SDB clients exist at the spike level
    (I-99/I-93) but are not wired into the installer flows. Those gaps must
    close before the final D-19 claim.
11. **Phase 2 live acceptance (web + phone + TV) — closed 2026-10-20.** The
    three Phase-1 legacy workspaces were migrated with `impresario migrate`
    (split devices/apps, table scenes, generated theatres, `Perform:` plays),
    validated with 0 warnings, formatted, and run against headless Chrome
    (`20261020-133846-23c0`, 9/9 steps), the Realme RMX3997 phone
    (`20261020-134217-2dfe`, 86/86 steps) and the Smart TV Pro
    (`20261020-133548-61d9`, 89/89 steps). Report schema v2 stage spans and
    JUnit `stage_sources` point into the generated theatres. See the Live E2E
    row for the environment quirks (connector port 50051, TV user 10).
12. The old `Pieces:` scene syntax and inline-`Stage:` play syntax remain
    accepted for compatibility (I-110). They are converted by `fmt`/`migrate`
    but not yet deprecated with warnings, so both forms remain supported in
    this phase.
13. Interpolation is implemented for quoted strings only (Actions, Perform
    arguments and parameter defaults); durations, timeouts, counts, selectors
    and platform lists are literal (I-117). TASK-2 asks for an exact set and
    this one is deliberate; an attempt to use `${...}` elsewhere fails that
    position's normal grammar rather than silently doing nothing.
14. The Phase 2 LSP work covers diagnostics, theatre/stage indexing,
    completion, navigation, symbols and whole-document formatting. The wider
    editor surface — code actions/quick-fixes, find references, rename, range
    and on-type formatting, inlay hints, code lenses and the standalone WASM
    extension — is TASK-2 Phase 5 (section 9) and is not implemented yet.

15. Android **USB data** is implemented over `libadb`/`nusb` but was not
    hardware-verified on this host: USB discovery saw no device exposing the
    ADB interface (the phone was reachable over wireless debugging only), so
    the live runs used TCP. Windows USB also needs a WinUSB-compatible driver,
    as `doctor` says. Two USB limits are deliberate: host-side logcat is
    skipped because USB ADB carries one session, and the forward fallback
    refuses to start while instrumentation is running; both return explicit
    guidance instead of silently degrading.
16. Managed Chromium is live-verified on **Windows x64 only**. All five
    platform archives are pinned with version, size and SHA-256, but the
    Linux/macOS downloads, extraction and browser launch have not run here;
    the Windows AppContainer ACL grant is likewise Windows-only. No managed
    download is attempted by `doctor` or a run without an installed copy.
