# Status

Current development status, per component. Updated at every phase boundary.
"Verified" means an automated check (test or command) ran successfully in this
repository on this machine (Windows 11, Rust stable 1.94.x).

## Environment constraints

- Host: Windows. No Swift/Xcode toolchain: the Apple agent cannot be built or
  run here. Apple sources will be complete and reviewed, but building/running
  them is out of reach; manual verification checklists are provided instead
  (spec sections 12 and 15).
- Phase 0 spike "gRPC (NIO) server inside an XCUITest runner" therefore cannot
  be executed here. D-14/D-18 remain **unverified** until someone runs the
  spike on a Mac; this is a recorded gap, not a silent one.
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
| `impresario lsp` | not implemented yet | fails loudly (exit 2) |
| `impresario-clients` (transports, AgentClient) | web-family + adb + mock done | `agent_client` (handshake/deadline/cancel/in-flight id/out-of-band screenshot), `convert`, `log` and `testing::MockAgent` unit-tested; `ConnectorTransport` verified via loopback mock agents; `AdbTransport` dials the on-device agent over the network first (`options.agent_address` → the serial's host → `ip route`) with `adb forward` as fallback, installs the three APKs in order and starts `am instrument` (I-62); Apple transport lands with the Apple agent (deferred, no Swift here) |
| `impresario-connector` (raw CDP + es5 helper + page resolution + `launch:headless`) | done | `fixtures_suite.rs` (spec 10.3 acceptance): 1 end-to-end session on `fixtures/web/index.html` covering handshake/capabilities/device info, click/type/append/clear/longpress/select/focus/keypress/back, hidden/disabled/obscured/count/text/xpath checks, auto-wait, scroll/scrollto (window + `scroll.html` container), shadow contenteditable, app restart + helper re-injection, in-band + out-of-band screenshots (PNG header parsed), console log stream, cancel by id (CANCELLED + PROTOCOL for unknown), multi-session + shutdown + SessionClosed, TV D-pad session, `launch:headless` + `app launch`/`app restart`, auth (token metadata/body) and error mapping — all against real headless Chrome; 50 unit tests (keys, script ES5 lint, pages HTTP framing, png, focus, launch); `cargo test -p impresario-connector` green |
| `impresario-connector` (chromiumoxide backend) | done (feature `chromiumoxide-backend`) | same fixture suite passes with `cdp_backend=chromiumoxide`; 55 unit tests with the feature; `cargo clippy -p impresario-connector --all-targets --features chromiumoxide-backend -- -D warnings` clean |
| Android agent (Kotlin: `:agent` app, `:agent-instrumentation`, `:agent-core`, `:driver-uiautomator`) | done — verified on a real Android TV | `agents/android/tools/run-tv-suite.ps1` acceptance on the TCL TV over Wi-Fi (`docs/DEVICES.md` §2.5): driver class **26/26 in 160 s** and wire class **2/2 in 100 s**, each in its own fresh `am instrument` process (vendor freeze timer, I-68), zero `doFreeze` on impresario packages in either window; the wire class drives the published proto over real gRPC (full action/check matrix + cancel + shutdown), and the TV semantics are I-64..I-73 (D-pad, leanback IME/hint handling, `longpress` key hold, `scrollto` budget, `background`); the touch phone path is verified live too (Galaxy A51, Android 13, Wi-Fi — `docs/DEVICES.md` §2.7, `run 20261018-103702-ee29`), including the I-75 `scrollto` end-of-container fix |
| Apple agent (Swift) | not started | — |
| Live E2E (master CLI → connector → headless Chrome) | verified | `impresario run` on a scratch workspace (`launch:headless`, `file:///…/fixtures/web/index.html`): connector auto-spawned, play passed 8/8 steps in 3.1 s, exit 0; `report.json`/`junit.xml`/PNG screenshot written (image shows the driven fixture page); one environment pitfall recorded: port 50051 was already held by `multipassd`, so the device config pins `agent_address: 127.0.0.1:47654` |
| Live E2E (master CLI → Android agent → TCL Android TV) | verified | `impresario run` on the scratch TV workspace over Wi-Fi: agent auto-installed + started, network dial + handshake + session, full smoke play green — `run 20261018-094855-ddb9: passed (1m46s, exit 0)`, 1 device, 1 play, 2 acts, 89 steps, 0 failed (Act 1 scenario tour 1m02s incl. D-pad `scrollto` row 29/0, text entry, selection, key log; Act 2 app-control lifecycle 19.5s incl. `background`/`launch`, `restart`, `terminate`); agent process ≈ 100 s < the ~223 s freeze budget (I-68). First attempt hung silently before the play — fixed as I-74 (lazy `StreamLogs` headers vs a tonic client that awaits the open); re-verified after I-75: `run 20261018-104405-4f10` (1m42s, 89 steps, 0 failed) |
| Live E2E (master CLI → Android agent → Android phone) | verified | `impresario run` on the scratch phone workspace over Wi-Fi (Galaxy A51, Android 13, device `pocket_phone`, `docs/DEVICES.md` §2.7): agent + stub auto-installed, network dial + handshake + session, full touch smoke play green — `run 20261018-103702-ee29: passed (1m53s, exit 0)`, 1 device, 1 play, 2 acts, 86 steps, 0 failed (Act 1 scenario tour 1m19s incl. touch `scroll`/`swipe up`/`swipe down`/`scrollto` both ways, text entry with `key backspace` echo checks, selection, `key back`/`key home`, device info `tv=false`; Act 2 app-control lifecycle 16.3s). The first attempts exposed the I-75 `scrollto` bug (end-of-container signal) |
| Reporters (console/junit/json/html) | done | `tests/reporters.rs` insta snapshots (console plain + color, junit.xml, report.json, report.html with base64 embed and missing-file fallback) |
| Docs (`docs/*.md`, examples, fixtures) | partially (DSL, DECISIONS, DEVICES, STATUS, ERRORS) | DSL.md covers syntax, semantics and selectors; DECISIONS.md holds I-01..I-75; DEVICES.md records per-platform setup and hardware evidence; `fixtures/web/*.html` is the connector fixture; PROTOCOL/PARITY/LSP pending later phases |
| CI workflows | not started | — |

## Known gaps and risks

1. Apple agent: cannot be compiled here (no Swift). Documented; manual
   checklists required (spec 15).
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
