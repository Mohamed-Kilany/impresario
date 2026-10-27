# Status

Current development status, per component. Updated at every phase boundary.
"Verified" means an automated check (test or command) ran successfully in this
repository on this machine (Windows 11, Rust stable 1.94.x). Phase 5 closed
2026-10-23; Phase 6 (native webOS/Tizen transports, packaging and Tizen
signing) closed the same day on fixture/replay evidence, with TV hardware
acceptance explicitly deferred. TASK-3 playback probes were implemented on
2026-10-25 on fixture/replay evidence (web connector, Android and Apple agent
packages, SDKs); no probe claim is hardware-verified.

Hardware access changed on 2026-10-25: a Samsung Tizen TV, an LG webOS TV and
an Apple TV are reachable on the LAN. Tizen completed `device check` (SDB
handshake without an AUTH challenge, capabilities, DUID and app list); the LG
TV serves its Developer Mode key but not the passphrase. The Apple TV moved to
`10.102.93.222` on 2026-10-26 and now has a proven physical tvOS path: signed
Impresario Agent/stub/UI-test bundles installed with a pure-Rust
RemotePairing/RSD/AFC pipeline, the UI test passed under XCUITest, and a real
gRPC `Handshake` + `Shutdown` completed on port 50062 (I-150). The same path is
now in the product `AppleTransport` and passed `impresario device check`; the
new product name/bundle ids still need regenerated profiles (I-152). The LG
passphrase and a Tizen signing profile are still missing, and no playback or
probe acceptance has run on any TV. Reachability alone closes no HD item.

## Current priority and parked work (2026-10-26)

Physical tvOS was the P0 blocker and its execution path is now proven. On
2026-10-26 the Apple TV `RAMA (4)` (`AppleTV5,3`, tvOS 26.1 build `23J582`,
UDID `a4d6ff71e98403878ba98fe85cb77e8d98d622e5`) was reached at
`10.102.93.222`. The unsigned `Apple Xcode` artifact from run `40286509471` at
`89d35e3` was signed locally with the user's free-team profiles and installed
over a pure-Rust RemotePairing -> RSD -> AFC -> InstallationProxy pipeline.
The personalized tvOS Developer Disk Image mounted, the UI test was started
over a direct outer-RSD `testmanagerd`/`dtservicehub` path, and
`AgentRunnerTests.testServe()` (pre-rename name) passed: Impresario Agent bound
`0.0.0.0:50062`, a real gRPC `Handshake` was accepted (session
`47123c1c-df6e-44b8-8efd-0043cdb3d001`, `AppleTV5,3`, tvOS 26.1, 1920x1080,
capabilities `2,3,5,7,8,9,10,11`) and `Shutdown` completed the test. The tvOS
`SystemAlerts` crash is fixed. This is startup/serve evidence, not a full tvOS
play: the action/check matrix, `XCUIRemote` focus and the local-network prompt
were not exercised.

That pipeline now lives in `impresario-apple-device`; `AppleTransport` spawns
no `xcodebuild`, `iproxy`, `simctl`, `devicectl` or `ideviceinstaller`, and
`impresario device check` passed through it on the same Apple TV with the
pre-rename bundle ids (I-152). The product rename is implemented but the new
bundle ids have not been signed/installed on hardware; their profiles must be
regenerated. The pairing record was created earlier by pymobiledevice3, so
pure-Rust pairing creation (as opposed to verification/tunnelling) remains
open. The global Apple ID wizard is implemented and unit-tested offline, but no
live Apple login/2FA has been exercised yet.

The TASK-3 Phase 2 actors/groups/audience/stage-resolution work is now on
`master` (merged from `wip/phase2-tvos-p0` on 2026-10-27), with the Rust gates
green. It remains parked, not abandoned; resume the parked work now that the
Apple product path is ported and the full tvOS play has run. The same history
carries the physical-tvOS work.

## Environment constraints

- Host: Windows. No local Xcode. `Apple Xcode` compiles the unsigned Xcode
  targets on GitHub's hosted `macos-15` runner; it does not execute them.
  Latest pre-rename green run is `40294832377` at `b96c7fd`; the
  `impresario-apple-unsigned` artifact contains the unsigned tvOS/iOS `.app`
  bundles. The evidence run used `40286509471` at `89d35e3`; its tvOS bundles
  were downloaded and signed here. A new run is required for the renamed
  targets.
  The signed copies were produced locally. Apple simulator testing was rejected by the
  user on 2026-10-25 and the simulator workflow/scripts were removed (I-148
  withdrawn), so no simulator execution is claimed. The Apple **agent
  package** is built and unit-tested
  here through WSL (Ubuntu 22.04, Swift 6.4.1): `swift build` and `swift test`
  cover `AgentCore`/`AgentService`, including the full wire protocol over
  loopback gRPC. Both halves are documented in `docs/DEVICES.md` §3; the
  physical-device checklist is in `agents/apple/README.md`.
- Physical Apple TV execution no longer needs a local Mac for the UI test and
  install path used in the evidence run: a Windows Rust host performed the
  RemotePairing tunnel, AFC upload, installation and direct-RSD XCUITest
  startup against `10.102.93.222`. Signing still uses the user's existing
  free-team `.p12`/profiles in a local, user-controlled step, and those profiles
  expire on 2026-11-02 (the rename requires regenerating them). The tvOS
  system-alert/local-network prompt and `XCUIRemote` focus path remain
  untested (D-14/D-18 are hardware-unverified; HD-1 stays open for the full
  play).
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
| `impresario device` / `app` / `migrate` | done (TASK-2 phases 1B + 3; wizard/discovery/pairing I-124) | global-by-default definitions with `--workspace` scope, append-preserving edits (I-103), list/show/check/edit/rename/remove/import/export; `migrate` dry-run/idempotency/.bak/mixed-file refusal and the legacy endpoint/app split (I-104); `tests/definitions_cli.rs` (7 tests) covers both layers, duplicate/bad fields, platform-scoped options, app check failures and the migration cases. Phase 3 adds `device discover`, `device pair`, the interactive `device wizard` (scope/platform/name/route/port/labels/check/preview, back/cancel with no writes), Android phone/TV role detection, and scripted wizard tests (including physical tvOS); live sessions wrote phone, TV and web definitions, and a fresh-key wizard pairing passed `device check`. `app check` still verifies ids/package existence only; installed state, install/uninstall and signatures remain later work. The wizard also covers physical `tvos`: platform `tvos`, the Apple TV UDID and the agent's LAN endpoint are written as `transport: network`; the wizard does not probe tvOS while writing the definition, and points at `impresario device check` after the machine-global Apple setup. `device check` itself passed on the physical Apple TV through the pure-Rust transport (I-152); the wizard's own live check was not exercised on tvOS |
| `impresario-core` scanner/parser/AST | done | core library suite (181 tests); negative matrix (`tests/negative_cases.rs`) plus dedicated theatrical fixtures for 030..050; table-form scenes, `.theatre` stages, `Examples:` tables and the legacy compatibility forms (I-110); proptest never-panics suite (3 generators + nasty corpus); `insta` snapshots of the example ASTs, theatre AST and a multi-error file |
| `impresario-core` semantic validation | done | unit tests: 008/009/012/013/014/017/019/020/021 with device awareness, plus theatrical resolution (031/032/034/036/037/040/041) and workspace lints 042..050 (unused scene/piece/stage/theatre/var/example column, duplicate `Var:`, Act platform coverage, required parameter never supplied) |
| `impresario-core` workspace loader | done | unit tests incl. duplicate scenes 006 |
| `impresario workspace new --example` / `validate` / `parse` / `device list` | done | CLI unit tests + integration test against the committed `examples/` workspace (`tests/parse_json.rs`: JSON stdout snapshot, exit codes); `workspace new` writes the split device/app example and refuses to overwrite without `--force` (I-102/I-103; `init` is gone); `validate` now resolves `App:` headers against the merged apps layer and shows the app count |
| `impresario device check` (alias `devices check`) | done | `tests/devices_check.rs`: DeviceInfo line + summary via the real binary, name selection, unknown name → 2, rejected handshake → 3, missing configs → 2; mock rows assert handshake/shutdown with no `Session` stream. Non-interactive; it does not take an app (it probes the device, not an app) |
| `impresario doctor` | done (TASK-2 Phase 3, resilience I-132) | per-platform report (`ready`/`degraded`/`unavailable`/`planned`) with what is needed, what was found, the chosen rung and the exact fix; configured devices are probed and reported individually; managed browser and host-key status are included. `--json` emits the stable machine shape; it never downloads or installs; exit is nonzero only on `unavailable`/configured-device failure (unsupported platforms without a device stay `planned`). A broken workspace config or devices file no longer aborts the command: `config_errors` and `device_errors` are reported, every remaining platform/tool probe still runs, and the command then exits 2. Live with `adb` absent from `PATH`: Android, Android TV and web `ready`, exit 0 (`target/phase3-doctor.json`). Unit tests cover status formatting, JSON shape and exit semantics; `tests/doctor_cli.rs` regression-tests a malformed config plus devices file and asserts the probes/JSON survive |
| `impresario tools` (managed Chromium) | done (TASK-2 Phase 3, I-123) | `tools list`, `tools install chromium`, `tools remove chromium`; install requires interactive consent or `--yes`, downloads only the pinned Chrome for Testing 155.0.8059.84 archive for the current platform, verifies its SHA-256 before extraction (the pinned size is shown for consent), stages under `~/.impresario/tools/chromium/<version>/<platform>`, and grants the two Windows AppContainer RX ACEs needed by the sandbox. `doctor` only reports. Verified with a managed headless run without `--no-sandbox` and a full 9/9 web E2E; Windows-only ACL path live-tested, other platforms pinned but not downloaded here (gap 16) |
| `impresario run` (planner/executor) | done | planner: 20 unit tests (compilation, selectors, timeouts, secrets, bounds); executor: `tests/run_mock.rs` (16 scenarios against `MockAgent`: pass/fail exit codes, timeout+cancel by id, dropped-stream reconnect, retries/flaky with LAUNCH→RESTART, fail-fast across devices, parallel vs serial timing, `app_reset` never/per_act/per_play, tag/act selection, screenshot files, capability gaps, secret redaction, Ctrl-C → 130); CLI: `tests/run_cli.rs` (default reporters, html base64, no secret in any artifact, missing workspace → 2) |
| `impresario lsp` | done (spec 13; full editor surface Phase 5) | in-process client over a real duplex stream with real `Content-Length` framing (`tests/common/lsp.rs`): `tests/lsp_diagnostics.rs` (open/change/close, overlay precedence over disk, the 150 ms debounce coalescing a burst, the UTF-16 vs UTF-8 offset difference after a surrogate pair, config-file diagnostics, device warnings switched by `didChangeConfiguration` in all three settings shapes, unsaved files outside the configured directories), `tests/lsp_features.rs` (completion contexts incl. device-filtered keys, hover selector tables, definitions, document symbols), `tests/lsp_editor.rs` (references, prepare-rename/rename, workspace symbols, folding, semantic tokens, signature help, code actions, document/range/on-type formatting, inlay hints, code lenses), `tests/lsp_corpus.rs` (the shared corpus and feature probes over JSON-RPC), `tests/lsp_watch.rs` (external edits, a `scenes` directory created after start, editor noise never triggers an analysis), `tests/lsp_cli.rs` (the real binary exits 0 when stdin closes); plus LSP unit tests. Every feature routes through the shared `impresario-language` service; `docs/LSP.md` is updated. |
| `impresario-language` + standalone VS Code extension | done (TASK-2 Phase 5, I-134...I-137) | The pure `impresario-language` crate is the single editor service (index, diagnostics, text, completion, navigation, formatting, actions). The LSP and the standalone WASM front end both call it; the extension host suite passed on VS Code 1.143.1/Windows. Standalone uses `impresario-language-wasm` (raw JSON ABI, no wasm-bindgen) with `impresario.mode: auto\|lsp\|standalone`; its watcher is debounced (150 ms) and its text cache re-reads only changed files. The shared `fixtures/lang-corpus/` runs in all three front ends. `npm run test:unit` covers corpus parity plus a 1,170-file benchmark (observed under 100 ms initial / under 60 ms re-analysis on this host); `npm run test:host` covers activation, diagnostics, completion, hover, definition, formatting (document/range/on-type), symbols and code actions. `npm run package` produced a 10-file VSIX containing the extension bundles, WASM, grammar, snippets, language configuration, manifest and README. No runtime network access. |
| `impresario mock-agent` | done (dev aid, decision I-77; not in the spec CLI list) | serves the scripted `MockAgent` on loopback (default 50071, `--port 0` prints the free port); `MockAgentBuilder::bind` unit path plus `tests/mock_cli.rs`: the real binary is spawned, a `web` device is pointed at its printed address, and the spawned `impresario run` exits 0 with `report.json` — the README quick start is under test |
| `impresario config` / `workspace` + `impresario-core::settings` | done (TASK-2 phase 1A) | `settings.rs` is the single schema (scope, type, default, docs) and resolves the workspace layer over the global layer; `home.rs` resolves `~/.impresario` (or `IMPRESARIO_HOME`) and `workspace.rs` resolves the root from the flag, environment, marker, global `workspace` key or cwd. `config list|get|set|unset|edit|path|explain` and `workspace current|list|add|remove|use|new` are wired with `--global`/`--workspace`, `--explain`, JSON output and atomic writes; `set`/`unset`/`edit` re-parse the whole file and refuse to make it invalid, and `edit` restores the old bytes and offers to reopen. `tests/config_cli.rs` (6 tests) covers lossless set/unset, refusals with `IMP-CFG-*`, precedence/explain/JSON, registry operations and the editor restore path; `docs/CONFIG.md` is generated from the schema and guarded by `tests/config_doc.rs`; I-100 (flag placement) and I-101 (yaml-edit list workaround) record the two deviations. The LSP and MCP now load the same layered settings |
| `impresario mcp` | done (dev/agent aid, decision I-80; not in the spec CLI list) | dual-era MCP server on stdio: modern stateless revision (2026-07-28 — per-request `_meta`, `server/discover`, `resultType`) plus the legacy `initialize` handshake (2025-11-25…2024-11-05, unknown → counter-offer); 8 tools (workspace info/validate, devices list/check, run, report get, artifacts list/get — screenshots as image content, artifact paths confined by `safe_join`, device-touching tools serialized by a process-wide lock) and `impresario://` resources (DSL/PARITY/DEVICES/MCP docs, workspace info); `tests/mcp_stdio.rs` spawns the real binary (legacy handshake run + artifact read, modern discover + protocol errors + diagnostics, failed run → screenshot image); 9 unit tests (dispatcher/eras/join/kinds); stdout is protocol-only while serving; **plus a scripted live smoke outside the test harness** (Python stdio client, both eras, real binary in the phone workspace, no device contact): the legacy handshake session listed the 8 tools + 5 resources, returned `workspace_info`/`devices_list`, listed the newest **real** run's artifacts (report.json, junit.xml, device screenshot) and returned the screenshot as an image block, then read the report; the modern `server/discover` session answered tools/report calls with `resultType: complete` and the unknown-tool error `-32602`; both exited 0 on stdin close; the hardware smoke followed the same day (see the MCP E2E row) |
| `impresario` TUI, jobs/daemon, profiles and test explorer | done (TASK-2 Phase 4, I-125…I-133) | With no arguments the binary starts the ratatui TUI only when stdin **and** stdout are terminals, `CI` is unset/empty/`0`/`false`, and neither `--no-tui` nor `--plain` is set; otherwise it prints the plain dashboard. Screens: landing (colorful `IMPRESARIO` wordmark, remembered workspaces, Add current/Doctor/Quit), workspace menu, overview/home, runs, tests, devices, apps, config (Landing/Menu replace the old F2–F7 tabs; Esc walks back); slash commands `/run /jobs /devices /apps /config /tests /open /doctor /fmt /validate /plan /help /quit`; Tab completion, Ctrl-R history (`~/.impresario/history`), run view keys `b` background, `x` stop, `f` log tail, `t` device filter, `/` filter, `?` help; `~/.impresario/state.json`; the first-run scaffold overlay appears after entering a workspace; a no-subcommand start self-installs to `~/.impresario/bin` and the user PATH (Windows HKCU + broadcast, Unix profile files), opt out with `IMPRESARIO_NO_PATH_INSTALL` or any `IMPRESARIO_HOME`; panic hook + `Drop` restore the terminal. The TUI is a client of the CLI (children run with `--no-tui --plain --no-color`; `/run` always detaches). Jobs: attached runs and a per-user daemon over `interprocess` local sockets (named pipe/UDS, token-authenticated, idle exit), `jobs [--all]`, `attach`, `logs`, `stop`, `rm`, `report`, `wait`; device lease files with `--on-busy wait\|fail\|queue`; orphaned jobs become `interrupted`; detached submissions release the caller's stdout pipes. Profiles live under global `profiles:`; the test explorer provides `tests list/new/show/where/open/grep/rename`, `--json`, templates, and `run --dry-run` act × device matrices with skipped reasons. Verification: `tests/tui_pty.rs` (3 real-binary ConPTY tests: first-run scaffold, slash completion + help, run view with `MockAgent` → `b` detach → close → `attach` → `report --json`), `src/tui/snapshots.rs` (11 `TestBackend`/`insta` frames: landing wide/narrow, workspace menu, home wide/narrow, tests wide/narrow, help/filter, first-run, followed run, small terminal), `tests/jobs_cli.rs` (6: detach/wait/logs/attach/stop/rm, leases/queue, Windows pipe release, recovery, daemon idle), `tests/tests_cli.rs` (3) and `tests/profile_cli.rs` (3). Live web detach → attach → report in `run 20261021-073144-2763` (9/9 steps, Chrome for Testing through the managed Chromium). Final live acceptance: detached four-device run `20261021-080459-ec51` passed 4 devices / 4 plays / 6 acts / 189 steps (local Chrome, Android phone, Android TV, LG webOS), was attached to completion and reported; the separate LG-only diagnosis run `20261021-080418-843e` passed 2/2, and the connector fixture suite passes after the collection-root fix. The Windows Terminal/PowerShell 7, PowerShell 5.1/conhost and cmd.exe checklist was executed (commands and results in `docs/CLI.md`). Recorded partial (gap 19): the device/app/config screens browse/open the owning file rather than offering dedicated edit forms, and the home screen does not probe reachability or run `doctor` automatically. |
| `impresario-clients` (transports, AgentClient) | web-family + native adb + native webOS/Tizen + pure-Rust Apple + mock done (no host tool for any device path) | `agent_client` (handshake/deadline/cancel/in-flight id/out-of-band screenshot), `convert`, `log` and `testing::MockAgent` unit-tested; `ConnectorTransport` verified via loopback mock agents; `AdbTransport` is native (I-121) and starts no `adb` binary: it dials the on-device agent over the network first (`connection.endpoint` → the serial's host → `ip route`) with a direct native `tcp:` forward fallback, sync-installs the three APKs in order, starts `am instrument` and streams host logcat/screencap (I-62/I-121); `AppleTransport` drives physical iOS/tvOS devices through `impresario-apple-device`: it loads the machine-global pair/signing/product/DDI stores, signs and installs Impresario Agent + stub + UI-test bundle with AFC/InstallationProxy, mounts the personalized DDI, and starts the UI test over the direct outer-RSD `testmanagerd`/`dtservicehub` path. No `xcodebuild`, `iproxy`, `simctl`, `devicectl` or `ideviceinstaller` is spawned. The host and port resolve from `connection.host`/`endpoint`; the agent port (`options.agent_port` → endpoint port → 50062) is passed to the test bundle as `IMPRESARIO_AGENT_PORT`. Simulator UDIDs are rejected with `DEVICE_UNAVAILABLE`, and a USB-only iPhone is not supported yet. The clean-state seam (`reset_app_data` + the app platform's signed `package`) reinstalls over the live control tunnel (I-84). Unit tests cover host/port parsing, simulator rejection, the reset no-op/unavailable cases, signing/product stores, IPA inspection and agent-port launch environment. Integration: `impresario device check` passed on the physical Apple TV with the pre-rename bundle ids (I-150/I-152). Phase 6 adds native `WebOsTransport` (SSH/SFTP/luna, pinned/TOFU host keys, luna install/launch/list, inspector forwarding) and `TizenTransport` (SDB AUTH/framing, capability/DUID, sync push, verified install, debug launch, `tcp:` forwarding), both with in-process replay servers; the live TV paths are hardware-gated. |
| `impresario package` / `impresario sign tizen` + `tizen_sign` / `webos_package` | done (fixture-verified; hardware pending) | Deterministic webOS `.ipk` (`ar` + fixed-metadata gzip/ustar; appinfo/control/packageinfo; symlink refusal) and Tizen `.wgt` (sorted ZIP, fixed timestamps). Tizen profile import validates PKCS#12, key/certificate match, expiry and DUID SANs; passwords use keychain service `impresario.tizen` (env overrides for headless); profile replacement now restores the old keychain entries if staging or the directory swap fails. RSA and P-256/P-384 ECDSA XML-DSig use SHA-512, exclusive C14N, fixed `#prop` digests and percent-encoded references. `sign tizen package` caches by package SHA-256/profile import/DUID, validates before reuse/install; Tizen 7.0+ re-signs, older firmware installs the original. Tests: 64 `impresario-clients` unit tests including deterministic packages, RSA/ECDSA round trips, tamper rejection, fake-SSH and fake-SDB replays; no TV hardware has run (HD-3/HD-4/HD-7). |
| Playback probes (`Showing`, `Player`, DRM) | done on fixture/replay evidence (2026-10-25); hardware acceptance deferred | One authoritative model and verdict implementation in `impresario-core::probe`; tri-state `pass`/`fail`/`unverifiable`; `--unverifiable fail\|warn` and `unverifiable_policy`; DSL parsing/planning for showing options, statuses, providers and typed assertions; `Probe` wire command plus `PROBE_*` capabilities and handshake `probe_sources`; master canonicalization so executor verdicts are advisory. Web connector collects DOM/HTMLMedia/EME/AVPlay/screenshot coverage and optional `window.__impresarioHook`; Android collects accessibility/screenshot/MediaSession/`media.drm` plus the debug hook logcat marker; Apple collects XCUITest evidence plus the Swift loopback hook and never invents AVPlayer state. Reports/JUnit/console/HTML carry `unverifiable`. Web fixture tests `media_probes_collect_showing_player_and_drm_evidence`, `web_app_hook_supplies_authoritative_player_evidence` and `tizen_avplay_parser_and_source_health`; Android `ProbeTest` (7 tests) covers the collector and the stub APK builds, Swift `ProbeCollectorTests` pass, and no hardware claim is made (HD-1/HD-5/HD-6/HD-8). |
| `impresario-connector` (raw CDP + es5 helper + page resolution + `launch:headless`) | done | `fixtures_suite.rs` (spec 10.3 acceptance): 1 end-to-end session on `fixtures/web/index.html` covering handshake/capabilities/device info, click/type/append/clear/longpress/select/focus/keypress/back, hidden/disabled/obscured/count/text/xpath checks, auto-wait, scroll/scrollto (window + `scroll.html` container), shadow contenteditable, app restart + helper re-injection, in-band + out-of-band screenshots (PNG header parsed), console log stream, cancel by id (CANCELLED + PROTOCOL for unknown), multi-session + shutdown + SessionClosed, TV D-pad session, `launch:headless` + `app launch`/`app restart`, auth (token metadata/body) and error mapping — all against real headless Chrome; plus the media/probe, optional-hook and Tizen-AVPlay fixtures (five integration tests without the feature, six with it); 52 unit tests (keys, script ES5 lint, pages HTTP framing, png, focus, launch, probe, source health); `cargo test -p impresario-connector` green |
| `impresario-connector` (chromiumoxide backend) | done (feature `chromiumoxide-backend`) | same fixture suite passes with `cdp_backend=chromiumoxide`; 57 unit tests with the feature; `cargo clippy -p impresario-connector --all-targets --features chromiumoxide-backend -- -D warnings` clean |
| Android agent (Kotlin: `:agent` app, `:agent-instrumentation`, `:agent-core`, `:driver-uiautomator`) | done — verified on a real Android TV | `agents/android/tools/run-tv-suite.ps1` acceptance on the TCL TV over Wi-Fi (`docs/DEVICES.md` §2.5): driver class **26/26 in 160 s** and wire class **2/2 in 100 s**, each in its own fresh `am instrument` process (vendor freeze timer, I-68), zero `doFreeze` on impresario packages in either window; the wire class drives the published proto over real gRPC (full action/check matrix + cancel + shutdown), and the TV semantics are I-64..I-73 (D-pad, leanback IME/hint handling, `longpress` key hold, `scrollto` budget, `background`); the touch phone path is verified live too (Galaxy A51, Android 13, Wi-Fi — `docs/DEVICES.md` §2.7, `run 20261018-103702-ee29`), including the I-75 `scrollto` end-of-container fix; app-data reset hardening landed 2026-10-19 (I-82: `clear_data` honored on `LAUNCH`, `pm clear` output verified, a refused clear degrades to a WARN + force-stop cold start — the Realme RMX3997 ROM denies shell `CLEAR_APP_USER_DATA`; live MCP re-run `20261019-064750-dca4` passed) |
| `impresario-apple-auth` + `impresario account` | implemented and offline-tested; live Apple login/2FA unverified | Vendored MIT `isideload` auth/anisette/dev/util subset; `impresario account login`, `status` and `logout` are dispatched before workspace resolution. The wizard prompts for the Apple ID and password with `rpassword` (password zeroized, never written/logged), performs GrandSlam auth, supports trusted-device and SMS 2FA (select number, resend, back, cancel) and team selection. The email/team/token/`spd` session and anisette state are AES-256-GCM files under `~/.impresario/apple/`; the 32-byte key lives in the OS keyring under `impresario.apple`/`session-key`, and `status --check` rebuilds the developer token from `spd` + anisette. Five offline tests cover the store, redacted diagnostics, 2FA paths and token refresh; no live Apple account was used (I-151/HD-2). |
| Impresario Agent for Apple (Swift: `AgentCore`, `ImpresarioWire`, `AgentService` + the `agents/apple/Xcode` targets) | done in code; Swift package tested on Linux; physical tvOS agent start/handshake verified; renamed bundles need new profiles; full tvOS play unverified | `swift build` + `swift test` green through WSL (Ubuntu 22.04, Swift 6.4.1; `docs/DEVICES.md` §3.1): **173 tests, 0 failures** in one bundle — 32 `AgentService` (loopback suites against a real grpc-swift server on an ephemeral port with the generated client: handshake/device info/capabilities including probe sources, the whole action + check matrix over the wire, cancel, heartbeats, `SessionClosed`, token auth, `StreamLogs`, second-session rejection; plus the protocol conversion and element-type map) and 141 `AgentCore` (selector parsing incl. predicates, the 6.2 key table, the 6.3 D-pad navigator, ordered dispatcher, command executor on a fake driver incl. deadlines/cancel/hidden/`clear_data` WARN, PNG builder, state/log bus, and the probe collector). Shutdown ordering is I-90 (found with `SWIFTNIO_STRICT=1`, which crashes on any task scheduled after a loop stopped), TV semantics I-86/I-87, clean state I-84. The Xcode deliverables (`project.yml`, host apps, stub fixture, UI-test bundle + ObjC shim) are compiled unsigned by `Apple Xcode`. On 2026-10-26 the pre-rename bundles were signed and installed on the physical Apple TV `RAMA (4)` (`AppleTV5,3`, tvOS 26.1 `23J582`) over a pure-Rust RemotePairing/RSD/AFC pipeline; the personalized DDI was mounted, the UI test started over the direct-RSD path, and `AgentRunnerTests/testServe()` passed with a real gRPC `Handshake` and `Shutdown` (I-150). The product rename is implemented: targets `ImpresarioAgent-iOS/-tvOS`, `ImpresarioAgentTests-iOS/-tvOS`, `ImpresarioStub-iOS/-tvOS`; bundle ids `com.impresario.agent`, `com.impresario.agent.tests`, `com.impresario.stub`. The renamed bundles have not been signed/installed on hardware because new profiles are required (I-152). Not yet verified on hardware: the full action/check matrix, `XCUIRemote` focus, the local-network prompt, the hook receiver and playback/probes (HD-1; `agents/apple/README.md`, `docs/DEVICES.md` §3.3) |
| Live E2E (master CLI → connector → headless Chrome) | verified | `impresario run` on a scratch workspace (`launch:headless`, `file:///…/fixtures/web/index.html`): connector auto-spawned, play passed 8/8 steps in 3.1 s, exit 0; `report.json`/`junit.xml`/PNG screenshot written (image shows the driven fixture page); one environment pitfall recorded: port 50051 was already held by `multipassd`, so the device config pinned the connector at `127.0.0.1:47654` (now `options.connector_address`) |
| Live E2E (master CLI → Android agent → TCL Android TV) | verified | `impresario run` on the scratch TV workspace over Wi-Fi: agent auto-installed + started, network dial + handshake + session, full smoke play green — `run 20261018-094855-ddb9: passed (1m46s, exit 0)`, 1 device, 1 play, 2 acts, 89 steps, 0 failed (Act 1 scenario tour 1m02s incl. D-pad `scrollto` row 29/0, text entry, selection, key log; Act 2 app-control lifecycle 19.5s incl. `background`/`launch`, `restart`, `terminate`); agent process ≈ 100 s < the ~223 s freeze budget (I-68). First attempt hung silently before the play — fixed as I-74 (lazy `StreamLogs` headers vs a tonic client that awaits the open); re-verified after I-75: `run 20261018-104405-4f10` (1m42s, 89 steps, 0 failed) |
| Live E2E (master CLI → Android agent → Android phone) | verified | `impresario run` on the scratch phone workspace over Wi-Fi (Galaxy A51, Android 13, device `pocket_phone`, `docs/DEVICES.md` §2.7): agent + stub auto-installed, network dial + handshake + session, full touch smoke play green — `run 20261018-103702-ee29: passed (1m53s, exit 0)`, 1 device, 1 play, 2 acts, 86 steps, 0 failed (Act 1 scenario tour 1m19s incl. touch `scroll`/`swipe up`/`swipe down`/`scrollto` both ways, text entry with `key backspace` echo checks, selection, `key back`/`key home`, device info `tv=false`; Act 2 app-control lifecycle 16.3s). The first attempts exposed the I-75 `scrollto` bug (end-of-container signal) |
| Live E2E via MCP (MCP server → Android agent → Realme phone) | verified | the whole loop through `impresario mcp` alone — a small-agent smoke with no CLI and no hand-run adb (the server's own prepare installed the APKs and started the agent): `devices_check` reported RMX3997 / Android 16 / UiAutomator2 / 720×1604, then `run` of the bare play name `mobile_smoke.play` passed — `run 20261019-061207-92f9: passed (1m43s, exit 0)`, 2 acts, 86 steps, 0 failed (touch `scroll`/`swipe`/`scrollto` both ways, text entry with echo checks, selection, `back`/`home`, device info `tv=false`; lifecycle `background`/`launch`, `restart`, `terminate`); the screenshot and report came back through `impresario_artifact_get`/`impresario_report_get` and stdout stayed protocol-only all session. The first attempt exposed the bare-play-name + refusal-detail gap fixed the same day (I-80) |
| Live E2E (Phase 3 no-tool runs: web + phone + TV) | verified | After the native ADB and managed-browser changes, the same migrated workspaces were rerun with `PATH` reduced to `C:\Windows\System32;C:\Windows` (`Get-Command adb` empty) and `IMPRESARIO_HOME=target\phase3-home`. Headless Chrome via managed Chromium `run 20261020-154341-d767` passed 9/9 steps in 2.42 s; Realme RMX3997 `run 20261020-153947-986f` passed 2 acts/86 steps (1m43s); Smart TV Pro `run 20261020-154152-216d` passed 2 acts/89 steps (1m34s). The wizard wrote live phone/TV/web definitions, and a fresh Impresario key paired through the Android pairing-code screen in the wizard (`wizard_pair_phone`, new home) and then passed `device check`. `impresario doctor --json` exited 0 with `android`, `androidtv` and `web` all `ready`, managed Chromium found without PATH, and `adb on PATH` reported as not present/not needed. Logs and doctor output are under `target\phase3-*.json/txt` on this host (not committed). |
| Live E2E (Phase 2 theatrical DSL, migrated workspaces: web + phone + TV) | verified | The three legacy Phase-1 scratch workspaces were copied and converted with `impresario migrate` on 2026-10-20: split devices/apps, table scenes, generated `.theatre` files, and plays rewritten to `Perform:`. All three then validated with **0 warnings** and were `fmt --check`-clean; a second `migrate` was a no-op. The phone scene needed one cleanup after migration: four TV-only pieces (`KeysRow`, `Alpha`, `Bravo`, `KeyLog`) that the shared legacy `Pieces:` block declared but the touch play never used were removed to clear `IMP-DSL-043`; this did not touch the generated theatre and the final run used the cleaned workspace. Real runs: headless Chrome `run 20261020-133846-23c0` passed 9/9 steps in 1.87 s; Realme RMX3997 `run 20261020-134217-2dfe` passed 2 acts/86 steps in 1m35s; Smart TV Pro `run 20261020-133548-61d9` passed 2 acts/89 steps in 1m38s. All reports are schema v2 with stage spans and JUnit `stage_sources` pointing into the generated theatres. Workspaces/logs are under `%TEMP%\impresario-phase2-e2e` on this host (not committed). Two environment quirks: `multipassd` owns port 50051, so the web run pinned `options.connector_address: 127.0.0.1:47654`; the TV agent APK had been installed only for secondary user 10, fixed by reinstalling all three APKs with `install --user 0` (`docs/DEVICES.md` §2.6) |
| Reporters (console/junit/json/html) | done | `tests/reporters.rs` insta snapshots (console plain + color, junit.xml, report.json schema v2, report.html with base64 embed and missing-file fallback); Act example display names, stage source spans and secret-key masking are covered |
| `impresario fmt` / DSL migration | done | `impresario_core::fmt` (idempotence property test, comments/blanks, Unicode-width table alignment, absent cells) and `impresario_core::migrate` (table scenes, generated theatres, duplicate suffixes, idempotence, fail-closed on invalid generated text) have unit tests; `tests/golden_dsl.rs` covers the formatter golden files and a complete legacy migration snapshot; `tests/definitions_cli.rs` drives `migrate --dry-run`, `.bak`, idempotence and generated theatre output; committed `examples/` are migrated and `fmt --check`-clean |
| `impresario run --arg` / theatrical runtime | done | planner tests cover `--arg` precedence (CLI > Examples > default), unknown/malformed arguments (IMP-DSL-032), nested stage expansion, cycles/depth and report display data; MCP `args` maps to the same code path |
| Docs (`docs/*.md`, examples, fixtures) + `README.md` | done (DSL, PROBES, PROTOCOL, DECISIONS, DEVICES, STATUS, ERRORS, LSP, MCP, CLI, PARITY, CONFIG, SIGNING, SPIKES, THIRD_PARTY, README) | DSL.md covers syntax, semantics, selectors and the `Showing`/`Player` probe grammar; PROBES.md covers the evidence model, verdict policy, per-platform reachability, fallback rungs, fixtures and the in-app SDK contract; PROTOCOL.md documents the wire contract as implemented (sequences, 7.2 rules, error-code table, endpoint resolution, I-74 lazy log stream); PARITY.md is generated from the planner's support tables and guarded by `tests/parity_doc.rs` (`IMPRESARIO_PARITY_BLESS=1` rewrites); CONFIG.md is generated from `settings::docs_table()` and guarded by `tests/config_doc.rs`; DECISIONS.md holds I-01..I-148 (I-91..I-98: the phase-0 spikes; I-99: webOS; I-100: `--workspace` placement; I-101: the yaml-edit list workaround; I-102: `init` replaced by `workspace new`, superseding I-11; I-103..I-108: the device/app split, migration, app binding, diagnostics and deferred wizard; I-109: the TV stub gap; I-110..I-119: theatrical DSL compatibility, formatter, Examples/`--arg`, report v2, migration, workspace scans, LSP, qualified references/interpolation scope, lints 049/050 and reporter/migration fail-closed behavior; I-120: Phase 2 live acceptance and spacing verification; I-121..I-124: native Android/libadb and live pairing, the Windows key-store workaround, managed Chromium, wizard/discovery/pair and doctor; I-125..I-130: the TUI-as-CLI-client architecture, jobs/daemon/leases, profiles, the ConPTY harness, test-explorer semantics and Phase 4 acceptance; I-131: the landing-first wizard navigation and PATH self-install; I-132: doctor's broken-config/device resilience; I-133: Phase 4 close-out and the deferred device-verification debt; I-146: remote CI and the 2026-10-25 hardware window; I-147: Tizen hardware protocol corrections; I-148: the withdrawn Apple simulator test gate); SPIKES.md carries the S-1..S-8 evidence and verdicts, updated with the Phase 3 S-1 and Phase 4 S-5 outcomes; THIRD_PARTY.md records every dependency/reference/managed-payload license, including the Phase 4 TUI/job crates; DEVICES.md records per-platform setup and hardware evidence, including the Phase 3 native ADB, pairing, wizard and managed-browser sections (§3 covers the Apple package and the Xcode commands); LSP.md documents the language server; MCP.md documents the stdio MCP server (tools, resources, per-client wiring, I-80); CLI.md documents the TUI, jobs/daemon, profiles, test explorer and the manual three-Windows-shell checklist; README.md's quick start is executed by `tests/mock_cli.rs` (workspace create/validate/run against `mock-agent`) |
| CI workflows | green on GitHub Actions | The public repository `https://github.com/Mohamed-Kilany/impresario` runs two workflows. `CI` run 40267725650 at `4e44203` passed all four jobs: Rust (`fmt --check`, `clippy --workspace --all-targets -- -D warnings`, `test --workspace`; the runner ships Chrome, so the connector fixture suite runs), Android (`gradlew --no-daemon test` with IPv4 preference and a retry for runner network flakes), editor (TypeScript, WASM corpus and VS Code host) and the Apple Swift package (`setup-swift` 6.4.1 + `zlib1g-dev`, `swift package resolve`/`build`/`test`). `Apple Xcode` run `40294832377` at `b96c7fd` generated the Xcode project and built the unsigned pre-rename tvOS and iOS stub/UI-test targets with `xcodebuild`; its `impresario-apple-unsigned` artifact contains the `.app` bundles and `.xctestrun` files. Earlier downloads had **zero** cross-platform `Info-*.plist` files and no top-level `_CodeSignature`/`embedded.mobileprovision` on the tvOS bundles. The macOS runner cannot reach the LAN Apple TV, so CI proves compilation only; the physical install/agent/handshake evidence is separate (I-150/I-152). The renamed targets need a new `Apple Xcode` run (`docs/DEVICES.md` §3.3). Workflow YAMLs are also parse-checked by `tests/ci_workflow.rs` |

## TASK-2 phase status

| Phase | State | Evidence |
|---|---|---|
| 0 | settled (S-1..S-8) | Verdicts and ladder rungs in `docs/SPIKES.md`; licenses in `docs/THIRD_PARTY.md`. Go: Android (S-1), Apple (S-2), webOS (S-3), Tizen (S-4), CLI/TUI/config (S-5), signing (S-6/S-7), WASM editor (S-8). Hardware validation is scoped per spike. |
| 1A | done | `home.rs`, layered `settings.rs`, workspace resolution, `config`/`workspace` commands, generated `docs/CONFIG.md`; full Rust gate green (commit `61751f8`). |
| 1B | done (wizard closed by Phase 3) | Split device/app models and merged layers, endpoint/auth/options derivation, `device`/`app` commands, `migrate` with `.bak`/dry-run/idempotency, plan/exec/transport/MCP/LSP app awareness, examples updated. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` are green. The interactive wizard and discovery/pairing were deferred here and are now implemented in Phase 3 (I-124, gap 9 closed); installed/signature `app check` remains later work. |
| 2 | done (live E2E verified 2026-10-20) | Table-form scenes, `.theatre` stages, `Perform:` / nested expansion, Act parameters, `Examples:` one-row-per-execution, `${param.*}`/`${example.*}`, `--arg` precedence, formatter, migration, lints 042..050, report schema v2, stage source locations in console/HTML/JUnit, qualified `Theatre.Stage` references and LSP theatre/formatting support. Legacy forms stay accepted (I-110), all choices are recorded as I-110..I-120. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` are green on this host; the migrated `examples/` validate and are `fmt --check`-clean. The exit criterion that required a **live web + phone + TV E2E with migrated workspaces** is met: the three Phase-1 legacy workspaces were migrated, validated with 0 warnings, and run green on headless Chrome, the Realme phone and the Smart TV Pro (run IDs in the Live E2E row and gap 11 closure). Code/docs are committed as `b551ec1`/`f5a11d0`; the hardware close-out is recorded here. |
| 3 | done (live Android/web verified 2026-10-20) | Native Android transport on `libadb` 0.6.0 with USB/TCP/mDNS discovery, wireless pairing, shell v2/exec streams, sync APK install, instrumentation output, forwarding fallback, logcat, screencap, `pm clear`/force-stop, `getprop` and `ip route` - no `adb` process (I-121). Managed Chromium download/verification with consent and Windows sandbox ACLs (I-123), `doctor` human/JSON output, and the full device wizard/discover/pair surface (I-124). Live acceptance: web/phone/TV E2E and a fresh-key wizard pairing with `adb` absent from `PATH`; `doctor --json` exit 0 with Android, Android TV and web `ready` (`target/phase3-doctor.json`). `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` are green. The native `tcp:` forwarding fallback was exercised live on the phone by setting an unreachable `connection.endpoint`; after the 30 s network wait, `device check` used the forward and passed. USB data and non-Windows Chromium archives remain unverified (gaps 10, 15, 16). |
| 4 | done (live acceptance 2026-10-21; landing/PATH/doctor pass 2026-10-22) | TUI, job store/supervisor, device leases, `--on-busy`, profiles, test explorer and file operations shipped as described in the component row and `docs/CLI.md`; decisions I-125…I-133. The Rust gates are green (`fmt --check`, `clippy --workspace --all-targets -- -D warnings`, `test --workspace`). W-CLI §3.6 evidence: 3 real-binary ConPTY tests (first-run, slash completion + help, run-view detach → close → reattach → report), 11 `TestBackend` frame snapshots, plus the live detached web run `20261021-073144-2763` and the final four-device detached run `20261021-080459-ec51` (4 devices / 4 plays / 6 acts / 189 steps). Live acceptance is complete: the four-device detached run was followed to completion and reported, the LG-only diagnosis run plus connector suite passed, and the Windows Terminal/PowerShell 7, PowerShell 5.1/conhost and cmd.exe rows were executed (see `docs/CLI.md`). The TUI editor/home scope remains a recorded partial (gap 19). Hardware re-verification is deferred as technical debt HD-1…HD-8 (I-133) and is not a Phase 4 blocker. |
| 5 | done (2026-10-23) | W-EDITOR parity gate: the pure `impresario-language` service is shared by `impresario lsp` and the standalone WASM/VS Code front end. `.play`, `.scene` and `.theatre` are covered by diagnostics, completion, hover, definition/references/rename, symbols, folding, semantic tokens, signature help, code actions, document/range/on-type formatting, inlay hints and code lenses. The three-front-end corpus (`impresario-language`, `lsp_corpus`, Node/WASM) is green; the extension host suite passes; strict TypeScript, ESLint, esbuild, `vsce` packaging and the 1,000+ file benchmark pass. `docs/LSP.md` and the extension README are updated. Decisions I-134…I-137 record the architecture and packaging. |
| 6 | done on fixture/replay evidence (2026-10-23); TV install/launch deferred (Tizen transport check passed 2026-10-25) | S-3/S-4 rung 1 shipped: pure-Rust `WebOsTransport` and `TizenTransport`, deterministic `.ipk`/`.wgt` packaging, Tizen profile import and RSA/ECDSA re-signing, native `device check`/`doctor` probes, planner package resolution, and `impresario package`/`sign tizen`. Replay coverage uses an in-process russh SSH server and SDB fake device; signing has deterministic, round-trip, tamper and ECDSA tests. On 2026-10-25 the Tizen TV completed native SDB `device check` (handshake without an AUTH challenge, capabilities, DUID, app list); the sync upload and `0 vd_appinstall` were accepted, but a self-signed package was rejected at certificate validation; a Samsung-issued profile, webOS SSH authentication and launch remain blocked, so no install/launch acceptance is claimed. HD-3/HD-4/HD-7 remain deferred with runbooks in `docs/DEVICES.md` §1.3/§1.4. Decisions I-138…I-147. |

## TASK-3 playback probe status

Recorded 2026-10-25. This is not a hardware acceptance record: it says what
compiled and what the fixture/replay tests exercised. `UNVERIFIABLE` remains a
real outcome on every platform.

| Area | State | Evidence / limits |
|---|---|---|
| Core model and policy | done | `impresario-core::probe` owns evidence classes/confidence, claims/assertions, the support matrix and `unverifiable_policy` (default `fail`). Unit tests cover conflicting evidence, missing evidence, negation and typed comparisons. |
| DSL and planner | done | `Check: [not] Showing => Piece [with coverage>=N%, decoded, source~"…"]` and `Check: Player => Piece … [with …]`; plan-time `IMP-DSL-013` for unsupported combinations; `source~` is web-only. Parser/planner tests cover canonicalization and rejection. |
| Wire and handshake | done | One `Probe` command with `kind`; `ProbeResult` evidence/verdicts; `PROBE_SHOWING/PLAYER/DRM`; `ProbeSourceInfo` advertised by mock, connector, Android and Apple. `UNVERIFIABLE` error code maps to the policy. |
| Master evaluation | done | `canonicalize_probe` validates evidence and recomputes verdict/confidence/reason with the core functions; executor verdicts cannot drift the result. Reports/JUnit/console/HTML render `unverifiable`. |
| Web / webOS / Tizen collector | done on fixtures | DOM, `HTMLMediaElement`, EME hook, Tizen AVPlay, screenshot coverage and optional app hook. Local web fixture covers showing/player/DRM/image/missing-piece; a synthetic AVPlay page covers the Tizen parser and Tizen-only source health. No TV hardware or TV CDP (HD-7). |
| Android collector | done on fixtures | A11y tree, screenshot coverage, `dumpsys media_session`, conservative `dumpsys media.drm`, `logcat` app hook. Stub APK builds with local H.264/AAC video and decoded PNG. USB and real playback acceptance deferred (HD-5/HD-8). |
| Apple collector | package tested; agent UI test starts on physical tvOS; collector/hook not executed | `ProbeCollectorTests` pass under WSL and the pre-rename agent UI test now runs on the physical Apple TV with a real handshake/shutdown (I-150). That acceptance does not drive the stub or collect probe evidence: `XCUIDriver`, the hook receiver and the full action matrix have not run on hardware, and XCUITest cannot read another app's AVPlayer/FairPlay, so player/DRM claims still need the optional hook. Local-network consent, playback and the tvOS remote remain HD-1. |
| In-app SDKs | web, Android and Swift implemented | Same optional JSON contract; debug-only, no playback effect. Web reads `window.__impresarioHook`, Android logs an `IMPRESARIO_HOOK` line read by the driver, Swift POSTs to `127.0.0.1:47831`. Missing hook only lowers achievable confidence. |
| Fixtures and docs | done | `fixtures/web/media.html` (video + audio-only + image), `hook.html`, `tizen-avplay.html`, `sample.webm`, `sample.png`; Android `MediaActivity` + assets; Apple `MediaViewController` + local video/audio/image and public BipBop HLS. `docs/PROBES.md` is the reference; fixtures are local/offline except the explicitly public HLS sample. |

The probe feature adds no new external runtime tool. D-19 still forbids
`ares-*`, `ssh`, `sdb`, `tizen`, `openssl`, `ar`, `tar`, `gzip` and host `adb`
at runtime; packaging uses in-process crates and Android uses `libadb`.

## Deferred hardware verification (technical debt)

Recorded 2026-10-22 with I-133 and extended for Phase 6 (I-145), the TASK-3
probes and the 2026-10-25 hardware window (I-146/I-147). The items below are the remaining device/platform verification
work and are deliberately deferred, not code-gate blockers. "Evidence today"
describes what actually ran on this host; this table does not promote any item
to verified. The webOS, Tizen and device-matrix items are scheduled with the
Phase 6/7 hardware/release window and link to the runbooks in
`docs/DEVICES.md`; the probe rows in those runbooks exercise real playback. The
Apple hook receiver and every playback path are covered by fixtures/replays,
not hardware.

| ID | Technical debt | Evidence today | Planned |
|---|---|---|---|
| HD-1 | Apple iOS/tvOS Xcode deliverable and real-device path | Startup/serve is hardware-verified on the Apple TV: `RAMA (4)` (`AppleTV5,3`, tvOS 26.1 `23J582`, UDID `a4d6ff71e98403878ba98fe85cb77e8d98d622e5`) at `10.102.93.222` mounted the personalized DDI, launched the UI test through the direct outer-RSD `testmanagerd`/`dtservicehub` path, and passed `AgentRunnerTests/testServe()` (pre-rename); Impresario Agent accepted a real gRPC `Handshake` and completed `Shutdown` (session `47123c1c-df6e-44b8-8efd-0043cdb3d001`, capabilities `2,3,5,7,8,9,10,11`). `impresario device check` later repeated the install/start/handshake through the product `AppleTransport`. The tvOS `SystemAlerts` crash is fixed. Not yet run on hardware: the renamed bundles, the full action/check matrix, `XCUIRemote` focus, the local-network prompt, `HookReceiver` and the Swift loopback hook (I-150/I-152; gaps 1-2). Player/DRM probes on Apple remain unverified without the hook. | Regenerate profiles for the renamed ids, reinstall, then run a full tvOS play; user grant for the local-network prompt if it appears |
| HD-2 | Apple D-19, signing and account | `AppleTransport` is now pure Rust: `impresario-apple-device` owns pairing verification, RSD tunnels, DDI, signing, AFC/InstallationProxy and direct-RSD XCUITest, and `device check` passed on the Apple TV with the pre-rename bundle ids. Remaining: the new bundle ids need profiles (existing free-team profiles expire 2026-11-02); pure-Rust pairing **creation** is implemented but not hardware-verified (the evidence pair record came from pymobiledevice3); DDI personalization/download is still user-supplied; App Store Connect and live global Apple ID login/2FA have not been exercised against Apple. No plaintext Apple ID password is stored. | Regenerate profiles for the rename, reinstall, exercise live login/2FA, verify pure-Rust pairing creation |
| HD-3 | webOS TV transport and install | The full native SSH/SFTP/luna transport, deterministic IPK packaging and install/launch/forward path are implemented and replay-tested. The LG TV at `192.168.93.116` exposes SSH 9922 and the key server 9991; its encrypted key was fetched to `keys/webos/192.168.93.116.pem`, but SSH authentication is blocked on the Developer Mode passphrase, so no IPK was uploaded and no install/launch ran (gap 4). The earlier 2026-10-21 four-device run used the connector/web path, not this native transport. | Phase 6/7 hardware window; runbook `docs/DEVICES.md` §1.3 |
| HD-4 | Tizen TV transport, install and signing | The native SDB transport, version-gated re-signing, verified app-list install and forwarding are implemented and replay-tested. On 2026-10-25 the Samsung TV completed `device check` over SDB: the handshake finished without an AUTH challenge, then capabilities, DUID `P7CIFS3MU3C26`, 864 apps, Tizen 9.0, staging `/home/owner/share/tmp/sdk_tools`, and the signing-required warning. A temporary self-signed profile then exercised upload and install: the TV accepted the per-WRTE-acknowledged sync push and `0 vd_appinstall`, reached certificate validation, and rejected the chain with `[118, -12] Invalid certificate chain with certificate in signature`. No app was installed. A Samsung-issued, DUID-bound profile is still required, and Samsung-account certificate creation remains external (gap 4). | Phase 6/7 hardware window; runbook `docs/DEVICES.md` §1.4 |
| HD-5 | Android USB data and playback probes | The native `libadb`/nusb path is implemented, but this host exposed no ADB USB interface; all live phone/TV runs used TCP. Windows WinUSB setup and the USB session limits need a hardware pass (gap 15). The Android media stub and `logcat` hook path compile but have not run on hardware; exact decoder/DRM facts remain vendor-dependent. | Phase 6/7 hardware window; runbook `docs/DEVICES.md` §4.1 |
| HD-6 | Managed Chromium outside Windows x64 | Five archives are pinned with size and SHA-256. Only Windows x64 download, extraction, AppContainer ACLs and headless launch actually ran here (gap 16). The web media/probe fixtures have only run on this host's Chrome; repeat them on each managed archive. | Phase 6/7 release-matrix window; runbook `docs/DEVICES.md` §4.2 |
| HD-7 | Real-TV CDP and web launch | The connector is verified against headless Chrome. Tizen/webOS CDP quirks and the `launch` path on a real TV are untested (gap 3); the native transports produce the forwarded inspector endpoint, but no TV has served CDP through it. Tizen reachability and the app list do not exercise `0 debug` or a forwarded WebSocket; LG SSH never authenticated. | Phase 6/7 hardware window; runbooks §1.3/§1.4 |
| HD-8 | Fresh re-run of the Phase 4 device matrix | The four-device run `20261021-080459-ec51` and LG-only run `20261021-080418-843e` passed on 2026-10-21 and remain the acceptance evidence. LG and Tizen TVs are attached again as of 2026-10-25, but neither can install/launch the fixture (passphrase and a Samsung-issued signing profile missing); the Apple TV now has a working agent path (I-150/I-152) but its play has not been run, so no new matrix run was attempted. Include the media stub/probe checks in the rerun; no probe result has been hardware-verified. | Phase 6/7 regression window after Phase 7; runbook `docs/DEVICES.md` §4.3 |

These are scheduling items, not silent stubs: the corresponding docs and code
still state the limits, and no command reports success for an unverified path.

### Hardware access log (2026-10-25)

- **Samsung Tizen TV** at `192.168.93.37:26101`. `impresario device check
  --device samsung_tizen` completed with the native SDB transport:
  `Tizen TV · Tizen 9.0 · 0x0 @1x · SDB (native)`; the SDB banner was
  `device::UA55CU8500UXSA::0` (55" CU8500). The handshake completed **without
  an AUTH challenge**, so the RSA AUTH path is still unexercised. The
  read-only probe read staging `/home/owner/share/tmp/sdk_tools`, DUID
  `P7CIFS3MU3C26` and 864 apps, and reported the expected Tizen 7.0+
  `signing required` warning. A temporary self-signed profile was imported to
  exercise the install path: the SDB sync push now succeeds only with a
  per-WRTE acknowledgement and the TV accepted `0 vd_appinstall` for package
  `Impresario`, then rejected the package at certificate validation with
  `[118, -12] Invalid certificate chain with certificate in signature`. No app
  was installed, and a Samsung-issued DUID-bound profile is still required.
- **LG webOS TV** at `192.168.93.116`. TCP 9922 (SSH) and 9991 (key server) are
  reachable; `http://192.168.93.116:9991/webos_rsa` was fetched and stored as
  `~/.impresario/keys/webos/192.168.93.116.pem`. `device check` stops at
  `webOS key passphrase: no passphrase in the OS keychain`; no SSH session,
  upload or launch has run.
- **Apple TV** now at `10.102.93.222`; the 2026-10-25 address was
  `192.168.93.19`. AirPlay `/info` identifies `AppleTV5,3` (Apple TV HD), name
  `RAMA (4)`, device ID `B6:FD:1D:0C:99:3C`, build `23J582`; the developer
  record reports tvOS 26.1 and UDID
  `a4d6ff71e98403878ba98fe85cb77e8d98d622e5`. TCP 7000, 62078 and 49152 are
  reachable. Impresario Agent, the stub and the UI-test bundles are installed,
  the personalized DDI is mounted and the direct-RSD XCUITest path passed
  `testServe()` with a real handshake and shutdown (I-150). The full UI action
  matrix and the local-network prompt are still unverified; the renamed ids
  need new profiles.
- The Apple TV agent UI-test start/handshake above is a hardware acceptance of
  that path (I-150). No result above is a hardware playback or probe acceptance;
  HD-1 (full play) and HD-3...HD-8 remain deferred.

## Known gaps and risks

Items 8, 9 and 11 were closed on 2026-10-20; they stay in place, clearly
marked, so the numbers used elsewhere remain stable.

1. Apple agent: the Swift **package** (`AgentCore`/`ImpresarioWire`/`AgentService`)
   is built and unit-tested on this host through WSL (see above). The
   `Apple Xcode` job compiles the host/stub/UI-test targets unsigned, but
   executes nothing. Simulator testing was rejected by the user and no
   physical device is available (I-148 withdrawn), so `testServe()`, the real
   `XCUIDriver`, the local-network permission and the tvOS remote/focus
   experience have never run here. They remain the manual checklists of spec
   15 (`agents/apple/README.md`, `docs/DEVICES.md` §3.3) and HD-1.
2. Phase 0 spike 2 (NIO gRPC in XCUITest): unverified on this host (see
   above); D-18 fallback design stays available if the spike fails on a Mac.
3. `impresario-connector` is verified against headless Chrome (installed on
   this machine) via the fixture suite (I-47). The attach path now waits for the
   page to leave `loading` and for the actual scroll event (fixing the
   under-load `fixture_suite_raw_backend` race); Tizen/webOS-specific CDP
   quirks and the `launch` path on a real TV remain untested on hardware.
4. Tizen and webOS hardware became reachable again on 2026-10-25. Phase 6
   ships the native `TizenTransport`/`WebOsTransport`, deterministic packaging
   and Tizen signing, all covered by in-process fake SDB/SSH servers and
   generated certificates (I-138...I-142). On the Tizen TV, `device check`
   completed the SDB handshake without an AUTH challenge, then capability
   parsing, DUID and app-list reads (Tizen 9.0, DUID `P7CIFS3MU3C26`, 864
   apps). A temporary self-signed profile proved the sync push and
   `0 vd_appinstall` path end to end up to certificate validation, which the TV
   rejected (`[118, -12]` invalid chain); a Samsung-issued profile is still
   missing. On the LG TV the key server and
   encrypted key were fetched, but SSH authentication stops at the missing
   Developer Mode passphrase. The connector CDP layer itself is still tested
   against Chrome only; HD-3/HD-4/HD-7 remain open. The LG webOS TV in the
   Phase 4 four-device run used the connector/web path, not this native
   transport, and is not evidence for it.
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
7. **CI remote compilation - closed 2026-10-25 (physical execution P0).**
   The public GitHub repository runs `CI` and `Apple Xcode`. The Apple job was
   green at `b96c7fd` (run `40294832377`); the proposed simulator workflow was
   withdrawn at user direction (I-148), and a GitHub-hosted runner cannot reach
   the LAN Apple TV. Physical tvOS is the active P0 path (I-149), and its
   pure-Rust execution path is now proven from this Windows host (I-150/I-152);
   the renamed bundles still need profiles and a hardware re-run (HD-1). The
   local gates are unchanged: Rust commands here, `gradlew test` on this host,
   and the Apple package `swift package resolve`/`build`/`test` through WSL
   (`docs/DEVICES.md` §3.1).
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
10. D-19 is **applied to Android, web, webOS and Tizen as of Phase 6**, and
    remains a known, explicit deviation for **Apple**. Android no longer starts
    `adb` (I-121); web uses the managed Chromium lookup (I-123); webOS uses
    native SSH/SFTP/luna, deterministic IPK packaging and native install
    (I-138/I-139); Tizen uses native SDB, WGT packaging and RSA/ECDSA signing
    (I-140...I-142). `doctor` reports `adb on PATH` as not present/not needed.
    `AppleTransport` still uses `xcodebuild`, `iproxy`,
    `simctl`/`devicectl`/`ideviceinstaller` (I-95); replacing that remains the
    final D-19 gap (Phase 7).
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
14. **Phase 2 LSP scope - closed 2026-10-23 (Phase 5).** The LSP and the
    standalone extension now share `impresario-language` and cover code
    actions/quick-fixes, find references, prepare-rename/rename, range and
    on-type formatting, inlay hints, code lenses, semantic tokens, signature
    help and workspace symbols for all three file kinds. The parity gate runs
    the same corpus in the Rust service, the LSP and the Node/WASM extension;
    the extension-host suite passes in standalone mode. `vsce` packaged the
    extension (10 files: bundles, WASM, grammar, snippets, language config,\n    manifest and README) and
    the archive contents were inspected; the host suite runs the development
    extension rather than installing that VSIX, so "install from VSIX" is not
    separately claimed. Hardware is not involved in Phase 5; HD-5/HD-6/HD-8
    remain scheduled as before.

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
17. **Phase 4 live multi-device acceptance — closed 2026-10-21.** The
    four-device detached run `20261021-080459-ec51` passed with local Chrome,
    the Android phone, the Android TV and the LG webOS TV: 4 devices, 4 plays,
    6 acts, 189 steps, exit 0, 1m51s. The viewer attached to it after
    detaching and `report.json` was read after completion. The LG-only
    diagnosis run `20261021-080418-843e` passed 2/2; after the connector
    collection-root fix (`elements()` now includes the collection root) the
    connector fixture suite passed 2/2. The prior four-device run remains the
    Phase 4 acceptance evidence; the separate LG run documents the diagnosis.
18. **The manual three-Windows-shell checklist — closed 2026-10-22.**
    `docs/CLI.md` records the executed Windows Terminal/PowerShell 7,
    PowerShell 5.1/conhost and cmd.exe rows: landing/menu navigation, help,
    completion, Ctrl-R, dry-run, real detach/attach, resize, Ctrl-Q and
    terminal restoration, plus the plain/no-TUI rows and JSON parsing. The
    PS 5.1 JSON row needs parentheses (or an assignment) because PowerShell
    5.1's `ConvertFrom-Json` emits the array as one pipeline object; the exact
    working command is recorded there. `impresario completions cmd` exits 2
    with the supported-shell list. The real-binary ConPTY suite remains the
    automated equivalent.
19. **TUI editor/home scope is a recorded partial against TASK-2 §3.2.**
    The devices/apps/config screens list the effective definitions/settings
    and `Enter` opens the owning file in `$VISUAL`/`$EDITOR`; add/edit/remove
    are run as CLI commands from the prompt (or the CLI wizard), not as
    dedicated in-TUI forms. The landing screen lists remembered workspaces
    (current first) with Add current/Doctor/Quit; Home shows the
    workspace/config paths, counts, active/recent jobs and in-app completion
    notifications, and the global home path appears in the first-run
    overlay; it deliberately does not probe device reachability or run
    `doctor` on every tick. Use `/doctor`,
    `/run --dry-run` or the CLI `device check` for live state. No silent
    stub: these surfaces either act or say what they do not do.
20. **TV setup is non-interactive (recorded partial, Phase 6).** The wizard
    still accepts Android/web only; webOS and Tizen are created with
    `impresario device add` or YAML. Tizen signing certificates are import-only
    (creation needs Samsung account services), and the webOS Developer Mode
    passphrase is read from the OS keychain or
    `IMPRESARIO_WEBOS_PASSPHRASE`; there is no interactive passphrase prompt or
    wizard step yet. The native transports and probes work without these
    conveniences, and missing profile/passphrase states fail with explicit
    instructions rather than an unsigned install.
