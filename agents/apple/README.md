# Impresario Apple agent (iOS / tvOS)

The Apple side of Impresario: an in-runner gRPC agent that drives iOS and tvOS
devices through XCUITest. Spec: `TASK.md` §12; parity gaps: `docs/PARITY.md`;
verification status: `docs/STATUS.md`.

```
agents/apple/
  Package.swift              AgentCore (pure Swift) + ImpresarioWire + AgentService
  Sources/AgentCore/         wire model, selectors, key tables, focus navigator,
                             ordered dispatcher, command executor  (no XCTest!)
  Sources/ImpresarioWire/    committed protobuf/gRPC code (scripts/gen-swift.sh)
  Sources/AgentService/      proto <-> core bridge, grpc-swift server, Bonjour
  Tests/AgentCoreTests/      executor/dispatcher/focus tests against fakes
  Tests/AgentServiceTests/   loopback gRPC tests (no device needed)
  Xcode/AgentUITests/        the runner: XCUIDriver over XCUIApplication,
                             AgentRunnerTests/testServe, ObjC safety shim
  Xcode/AgentHost/           empty host apps (the test bundles' target app)
  Xcode/AgentTargetStub/     the fixture app under test
  project.yml                XcodeGen spec: the Xcode project is generated, not
                             committed (docs/DECISIONS.md I-89)
```

Two layers exist because devices are only reachable from XCUITest:

| Layer | Runs where | Tested by |
| --- | --- | --- |
| `AgentCore` / `AgentService` | any platform (Linux, macOS, iOS, tvOS) | `swift test` |
| `XCUIDriver` + `AgentRunnerTests` | inside the XCUITest runner on a device/simulator | Xcode (device), manual |

## 1. Build and unit-test (works without a device)

```bash
cd agents/apple
swift build          # AgentCore + ImpresarioWire + AgentService
swift test           # AgentCoreTests + AgentServiceTests (loopback gRPC over 127.0.0.1)
```

Both run on Linux and macOS. `swift test` starts a real grpc-swift server on a
loopback port and speaks the full protocol (handshake, commands, logs,
screenshots, shutdown), so the wire layer is verified without any Apple device.

Dependencies are pinned (`Package.resolved` is committed):

| Package | Pin |
| --- | --- |
| swift-protobuf | 1.39.0 |
| grpc-swift | 1.27.6 |
| swift-nio | 2.104.0 (resolved) |
| swift-nio-http2 | 1.46.0 (resolved) |

Wire code is generated, not hand-written:

```bash
scripts/gen-swift.sh          # protoc + protoc-gen-swift/grpc-swift -> Sources/ImpresarioWire
```

## 2. Generate the Xcode project

```bash
brew install xcodegen
cd agents/apple
xcodegen generate             # writes ImpresarioApple.xcodeproj
```

Targets and bundle ids:

| Target | Type | Bundle id |
| --- | --- | --- |
| `AgentHost-iOS` / `AgentHost-tvOS` | empty app | `com.impresario.agent.AgentHost-iOS` / `-tvOS` |
| `AgentUITests-iOS` / `AgentUITests-tvOS` | UI-test bundle (the runner) | `com.impresario.agent.AgentUITests-iOS` / `-tvOS` |
| `AgentTargetStub-iOS` / `AgentTargetStub-tvOS` | fixture app under test | `com.impresario.agent.AgentTargetStub-iOS` / `-tvOS` |

`AppleTransport` (`crates/impresario-clients/src/transport.rs`) hard-codes the
runner selection:

```
-only-testing com.impresario.agent.AgentUITests-iOS/AgentRunnerTests/testServe
-only-testing com.impresario.agent.AgentUITests-tvOS/AgentRunnerTests/testServe
```

Do not rename the test bundle or `testServe`.

Signing: device runs need a development team. Set it in Xcode
(Signing & Capabilities) or add a `Local.xcconfig` — nothing in the repo
hard-codes a team id.

## 3. Build the app under test (the fixture)

```bash
xcodebuild build -scheme AgentTargetStub-iOS \
  -destination 'platform=iOS,id=<udid>' -derivedDataPath build
# -> build/Build/Products/Debug-iphoneos/AgentTargetStub-iOS.app
```

Point the config at it so `clear_app_data` can reinstall it per test
(`docs/DECISIONS.md` I-84):

```toml
[[devices]]
id = "<udid>"
platform = "ios"
app_id = "com.impresario.agent.AgentTargetStub-iOS"

[devices.options]
clear_app_data = true
app_path = "/abs/path/build/Build/Products/Debug-iphoneos/AgentTargetStub-iOS.app"
```

## 4. Start the runner and connect a master

```bash
# one-time per device/OS combination (produces the .xctestrun)
xcodebuild build-for-testing -scheme AgentUITests-iOS \
  -destination 'platform=iOS,id=<udid>' -derivedDataPath build
```

The master starts the runner itself when `IMPRESARIO_XCTESTRUN` points at the
produced file:

```bash
IMPRESARIO_XCTESTRUN=build/Build/Products/AgentUITests-iOS_iphoneos16.0-arm64.xctestrun \
  impresario run --config impresario.toml
```

Runner environment (read from the `.xctestrun`'s `TestingEnvironmentVariables`,
so inject them there with `plutil` or by editing the plist):

| Variable | Meaning |
| --- | --- |
| `IMPRESARIO_AGENT_PORT` | port the agent binds (default `50062`, the value `AppleTransport` dials) |
| `IMPRESARIO_AGENT_TOKEN` | shared token; when set, the master must send it (spec 7.2.1) |
| `IMPRESARIO_NO_BONJOUR` | `1` disables the `_impresario._tcp` advertisement |

```bash
plutil -insert TestingEnvironmentVariables.IMPRESARIO_AGENT_PORT -string 50062 \
  build/Build/Products/AgentUITests-iOS_iphoneos16.0-arm64.xctestrun
```

The runner logs everything it knows at start (visible in the `xcodebuild`
output the master drains):

```
impresario agent: platform=ios port=50062 bonjour=on token=off
impresario agent: reachable at http://<device-ip>:50062 (grpc) — pass agent_address=<device-ip>:50062 to the master for WLAN runs
impresario agent: addresses en0=192.168.1.42
impresario agent: serving; waiting for a master (Shutdown ends the run)
```

For tvOS (no USB path) put that address into the device config:

```toml
[[devices]]
id = "<udid>"
platform = "tvos"
agent_address = "192.168.1.42:50062"
```

For iOS over USB the master runs `iproxy` itself. iOS over WLAN works the same
way as tvOS: `agent_address = "<device-ip>:50062"`.

## 5. Clean state per test (I-84)

Android wipes the app's data dir in the agent; iOS/tvOS have no such API, so the
**host** performs the equivalent — uninstall + reinstall the app under test
before the runner launches it. `clear_app_data = true` plus `options.app_path`
selects the tool automatically:

| Device | Tool |
| --- | --- |
| simulator | `xcrun simctl uninstall` + `install` |
| real device, Xcode 15+ | `xcrun devicectl device uninstall` + `install` |
| real device, older host | `ideviceinstaller` (from `libimobiledevice`) |

When the host cannot do it (`app_path` unset, no tool installed), the run
continues: `clear_app_data` degrades to a once-per-session `WARN` log line plus
a cold start (`terminate` + `launch`), and the reason is reported in the run's
log. Nothing else about the agent changes.

## 6. Real devices over WLAN: what is automatic, what is not

The first run after an install has to pass Apple's local-network gate. The
agent does as much as it can on its own (`docs/DECISIONS.md` I-88):

* `NSLocalNetworkUsageDescription` + `NSBonjourServices` are in the Info.plist
  of both the runner and the host app, so the process is *allowed* to ask
  instead of being killed on first use;
* `SystemAlerts` registers an XCUITest interruption monitor **and** polls
  SpringBoard right after the server starts, tapping Allow when it can find the
  button ("Allow", "Erlauben", … and the "[Don't Allow, Allow]" two-button
  case);
* the agent binds `0.0.0.0`, advertises `_impresario._tcp` with a
  `platform`/`protocol`/`agent` TXT record, and prints every local address it
  has, so the operator (or a future discovery step in the master) can copy the
  address straight into `agent_address`.

Still human, once per device: if the alert's buttons are not reachable from the
test process (Apple does not guarantee SpringBoard's hierarchy), the runner logs

```
impresario agent: a system alert is up but its buttons are unknown (...) — tap the allow button on the device once
```

and somebody taps Allow on the device (an Apple TV remote can do it). After
that the permission is remembered and runs need no human. `IMPRESARIO_NO_BONJOUR=1`
skips the advertisement if a network blocks Bonjour; the master can always
connect to `agent_address`.

## 7. Semantics worth knowing (details in TASK.md §5–6, docs/PARITY.md)

* `scroll <dir>` reveals content in `<dir>` (one step = a drag across 80% of the
  container), so the finger moves the other way; `swipe <dir>` moves the finger
  in `<dir>`.
* On tvOS `click` = D-pad focus traversal + Select, falling back to a touch tap
  when the element is hittable but unreachable (`FOCUS_UNREACHABLE` only when
  neither works); `swipe`/`select` are tvos-only/ios-only respectively and fail
  with `UNSUPPORTED_ACTION` where the platform has no equivalent.
* Keys (`key` action): tvOS `ok`=Select, `back`=Menu, arrows, `playpause`, `home`;
  iOS only `home`.
* `type`/`append`/`clear` read the field first, use the field's focus, delete
  the current content and type the new text, then read it back and verify.
* Predicate selectors are compiled through the ObjC shim: a syntax error is
  `INVALID_SELECTOR`, never a crashed runner.
* `animations_disabled` is passed to the app under test as the launch argument
  `-impresario-animations-disabled` (Apple has no API to disable another app's
  animations; the stub app honours it).

## 8. Troubleshooting

| Symptom | Cause / fix |
| --- | --- |
| `set IMPRESARIO_XCTESTRUN …` | the transport cannot find the runner's `.xctestrun`; build it (`build-for-testing`) and export the path |
| runner exits with "Address already in use" | a previous runner still holds the port; kill the old `xcodebuild`/runner app or change `IMPRESARIO_AGENT_PORT` **and** the master's port |
| `the play did not name an app under test (app_id)` | the play has no `app_id`; the Apple agent cannot guess the target app |
| `UNSUPPORTED_ACTION … tvos-only / ios-only` | platform gap by design, see `docs/PARITY.md` |
| `clear_app_data` warns "reinstall unavailable" | set `options.app_path` (see §5) |
| screenshots/logs empty on a device | the runner needs `NSLocalNetworkUsageDescription`; regenerate the project with `xcodegen` after pulling |

## 9. Verification status

`swift build`/`swift test` are green on Linux (this repository's CI-less
environment). The Xcode targets cannot be compiled on Windows: they are written
against the documented XCTest API and must be built once on a macOS host
(`docs/STATUS.md` lists this as an open gap, together with the Phase-0 device
spike).
