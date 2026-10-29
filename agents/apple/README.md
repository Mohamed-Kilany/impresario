# Impresario Agent for Apple (iOS / tvOS)

The Apple side of Impresario: **Impresario Agent** runs inside an XCUITest
bundle and drives iOS/tvOS apps over gRPC. The host pipeline is pure Rust:
pairing, RSD tunnels, signing, AFC upload, InstallationProxy, Developer Disk
Image mounting and XCUITest all live in `impresario-apple-device`. No
`xcodebuild`, `iproxy`, `simctl`, `devicectl`, `ideviceinstaller` or
libimobiledevice binary runs at runtime (D-19).

Spec: `TASK.md` §12. Verification: `docs/STATUS.md` and `docs/DEVICES.md` §3.3.

## Layout

```
agents/apple/
  Package.swift                 AgentCore + ImpresarioWire + AgentService
  Sources/AgentCore/            wire model, selectors, keys, focus, dispatcher
  Sources/ImpresarioWire/       committed protobuf/gRPC code
  Sources/AgentService/         proto <-> core bridge, grpc-swift server, Bonjour
  Tests/                        Swift tests (no device needed)
  Xcode/ImpresarioAgent/        UI-test host app
  Xcode/ImpresarioAgentTests/   XCUITest bundle: XCUIDriver, AgentEnvironment,
                                ImpresarioAgentTests/testServe, ObjC shim
  Xcode/ImpresarioStub/         fixture app under test
  Xcode/Shared/Assets.xcassets/ tvOS app icon (from logos/impresario-wide.png;
                                letterboxed on black, not stretched)
  project.yml                   XcodeGen spec (the .xcodeproj is generated)
```

The XCUITest process is an implementation detail. The installed product is
always **Impresario Agent** (`com.impresario.agent`); the `-Runner` and
`.xctrunner` names are Xcode artifacts.

## 1. Build and test

```bash
cd agents/apple
swift build
swift test          # AgentCoreTests + AgentServiceTests over loopback gRPC
```

Both run on Linux and macOS. `swift test` starts a real grpc-swift server and
exercises handshake, actions, checks, logs, screenshots and shutdown without an
Apple device.

Generate and build the Xcode deliverables (macOS only):

```bash
brew install xcodegen
xcodegen generate                       # writes ImpresarioAgent.xcodeproj

xcodebuild -project ImpresarioAgent.xcodeproj \
  -scheme ImpresarioAgentTests-tvOS -configuration Debug \
  -destination 'generic/platform=tvOS' -derivedDataPath build-tvos \
  CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO \
  CODE_SIGN_IDENTITY= PROVISIONING_PROFILE_SPECIFIER= build-for-testing
xcodebuild -project ImpresarioAgent.xcodeproj \
  -scheme ImpresarioStub-tvOS -configuration Debug \
  -destination 'generic/platform=tvOS' -derivedDataPath build-tvos \
  CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO \
  CODE_SIGN_IDENTITY= PROVISIONING_PROFILE_SPECIFIER= build
```

Use `-iOS` schemes and `generic/platform=iOS` for iPhone/iPad. The GitHub
`Apple Xcode` workflow builds both platforms unsigned; signing happens on the
machine that owns the development identity/profiles.

| Target | Bundle id | Artifact |
| --- | --- | --- |
| `ImpresarioAgent-iOS` / `-tvOS` | `com.impresario.agent` | host app |
| `ImpresarioAgentTests-iOS` / `-tvOS` | `com.impresario.agent.tests` | nested `.xctest` UI-test bundle |
| `ImpresarioAgentTests-iOS` / `-tvOS` | `com.impresario.agent.tests.xctrunner` | Xcode's outer `-Runner.app` |
| `ImpresarioStub-iOS` / `-tvOS` | `com.impresario.stub` | fixture app |

The outer runner and the nested test bundle are two different bundle ids and
both need signing material. A wildcard profile for
`com.impresario.agent.tests*` covers both.

`testServe` is the entry point. The agent platform is chosen at compile time;
the test bundle identifier is the same on iOS and tvOS.

## 2. One-time machine setup

Everything is machine-global under `~/.impresario/apple/` (or
`%USERPROFILE%\.impresario\apple\` on Windows). Projects never contain
pairing secrets, certificates or profiles.

### 2.1 Pair the device

```powershell
impresario apple pair --host <apple-tv-host>
```

Or import an existing pymobiledevice3 pair plist and key it by the device UDID:

```powershell
impresario apple import-pair `
  "$env:USERPROFILE\.pymobiledevice3\remote_....plist" `
  --save <udid>
```

`impresario apple status` lists what the machine has without touching a device.

### 2.2 Import signing materials

Create a directory with a PKCS#12 identity and one profile + entitlements pair
per bundle id. Profiles may be wildcard; all bundles share one certificate.

```
signing-source/
  identity.p12
  host/     profile.mobileprovision  entitlements.plist  # com.impresario.agent
  stub/     profile.mobileprovision  entitlements.plist  # com.impresario.stub
  tests/    profile.mobileprovision  entitlements.plist  # com.impresario.agent.tests
  runner/   profile.mobileprovision  entitlements.plist  # com.impresario.agent.tests.xctrunner
```

One wildcard profile for `com.impresario.agent.tests*` can replace the two test
directories.

```powershell
impresario apple signing import .\signing-source --name default
```

The PKCS#12 password is read interactively and stored in the OS credential
store, never on disk. For a headless import only, set
`IMPRESARIO_APPLE_P12_PASSWORD`; unset it afterwards.

The profile's `Entitlements.application-identifier` names the bundle id, so the
directory names are arbitrary. Free Apple ID profiles expire after seven days
and must be regenerated; `impresario apple signing status --name default` shows
expiry. Validity can be checked without a device:

```powershell
impresario apple signing verify --name default --products current `
  --output $env:TEMP\signed-check
```

### 2.3 Import the unsigned agent build

```powershell
impresario apple products import `
  .\agents\apple\build-tvos\Build\Products\Debug-appletvos `
  --name current
```

The import requires exactly one host app, one stub and one `-Runner.app`, copies
them as `host.app`, `stub.app`, `runner.app`, and records the real bundle ids.
`impresario apple products status --name current` prints them.

### 2.4 Import a personalized DDI

The device reports the exact OS build. Supply the three files produced for that
build:

```
ddi-source/
  Image.dmg
  Image.dmg.trustcache
  BuildManifest.plist
```

```powershell
impresario apple ddi import .\ddi-source --build <build>
```

No DDI is downloaded and no external personalization tool runs.

### 2.5 Sign in to Apple once

The Apple ID session, used by future provisioning work, is machine-global and
encrypted. Password entry is interactive only; the password is never stored.

```powershell
impresario account login          # Apple ID + password + trusted-device/SMS code
impresario account status
impresario account logout
```

## 3. Configure a device and run

```yaml
devices:
  - name: rama
    type: tvOS
    ip: <apple-tv-host>
```

The `ip` is the LAN host used for RemotePairing and the RSD tunnel. Pairing is
looked up by host, the UDID is learned from the tunnel, and the agent port is
allocated dynamically. Select a non-default signing/product set with
environment variables:

| Variable | Default | Meaning |
| --- | --- | --- |
| `IMPRESARIO_APPLE_SIGNING` | `default` | signing set under `apple/signing/` |
| `IMPRESARIO_APPLE_PRODUCTS` | `current` | product set under `apple/products/` |
| `IMPRESARIO_APPLE_DDI` | detected build | explicit DDI directory |
| `IMPRESARIO_APPLE_START_TIMEOUT_SECS` | `120` | wait for the agent port |

Smoke-test the whole path (pairing, signing, install, DDI, XCUITest,
handshake):

```powershell
impresario device check rama --plain
```

Start the agent without a play:

```powershell
impresario apple session --host <apple-tv-host> `
  --udid <udid>
```

It prints `Agent endpoint: <host>:50062` and waits until a master sends
`Shutdown` (or Ctrl-C). A run uses the same pipeline through `AppleTransport`.

The app under test is optional during install. A signed `.app`/`.ipa` can be
installed with `impresario apple session --app ... --app-bundle-id ...`; normal
runs name the already-installed app in the play.

## 4. Clean state per test (I-84)

iOS/tvOS have no `pm clear`. With `clear_data = true`, the host uninstalls and
reinstalls the app under test over the live RemotePairing control tunnel before
the agent launches it. Set the app's signed package and id:

```yaml
apps:
  - name: fixture
    platforms:
      tvos:
        id: com.impresario.stub
        package: C:\build\ImpresarioStub.app
    options:
      clear_data: true
```

No `simctl`, `devicectl` or `ideviceinstaller` is used. If the package is not
configured, the agent cold-starts the app and logs that the clean reinstall was
unavailable. Simulators are rejected: the pure-Rust path cannot drive them and
D-19 forbids the host tools that could.

## 5. First-run local-network permission

The host app and the test bundle declare `NSLocalNetworkUsageDescription` and
`_impresario._tcp`. iOS may show a permission prompt on first run. On tvOS the
`SystemAlerts` workarounds are compiled out: accept the prompt once with the
Apple TV remote. The permission is remembered afterwards. The agent binds
`0.0.0.0`, publishes `_impresario._tcp` when Bonjour is available, and prints
its addresses in the device log.

## 6. Evidence and open work

* On 2026-10-26 the physical Apple TV `RAMA (4)` (`AppleTV5,3`, tvOS 26.1
  `23J582`, UDID `a4d6ff71e98403878ba98fe85cb77e8d98d622e5`,
  `10.102.93.222`) ran the full pure-Rust path: pair-verify, RSD, DDI mount,
  signing, AFC + InstallationProxy install, direct outer-RSD
  `testmanagerd`/`dtservicehub`, and an XCUITest agent session. A real gRPC
  `Handshake` and `Shutdown` completed (session
  `47123c1c-df6e-44b8-8efd-0043cdb3d001`, capabilities
  `2,3,5,7,8,9,10,11`).
* The same path was repeated through the productized `AppleTransport`
  (`impresario device check`) with the pre-rename old bundle ids.
* The product rename to `com.impresario.agent` / `com.impresario.agent.tests` /
  `com.impresario.stub` is implemented in `project.yml`; those new bundles have
  **not** yet been signed and installed on hardware because their profiles must
  be generated after the rename.
* Still unverified on hardware: the full action/check matrix, `XCUIRemote`
  focus, local-network prompt handling, `HookReceiver`, playback/probes, and
  clean-state reinstall via `reset_app_data`.
* The pair record used for the evidence came from an earlier pymobiledevice3
  setup; `impresario apple pair` has not yet produced a hardware-verified pair.
* The Apple ID wizard is implemented and unit-tested offline; no live Apple
  login/2FA has been exercised.

## 7. Troubleshooting

| Symptom | Cause / fix |
| --- | --- |
| `no pairing for Apple device …` | run `impresario apple pair` or `import-pair` for that host |
| `signing set 'default' is not usable` | import the `.p12`, profiles and entitlements; check expiry with `signing status` |
| `Apple product set 'current' is not usable` | import the unsigned Xcode output with `products import` |
| `Image.dmg` / `BuildManifest` missing | import the personalized DDI for the OS build RSD reports |
| `simulator` / `DEVICE_UNAVAILABLE` | the pure-Rust transport supports physical devices only |
| agent port already in use | the port is allocated dynamically; if a firewall blocks the ephemeral range, allow the host and Apple TV to reach each other |
| `UNSUPPORTED_ACTION … tvos-only / ios-only` | platform gap by design; see `docs/PARITY.md` |
| `clear_app_data` warns the reinstall is unavailable | give the app a signed `package` and `id` (see §4) |
| profile expired | generate new profiles for the current bundle ids and re-import the signing set |
| Apple ID login not stored | `impresario account status`; log in with trusted-device or SMS 2FA |
