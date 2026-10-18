# Devices: setup per platform

How to prepare each platform, and how to run the automated checks against it.
"Verified" means the commands ran successfully on the development host of this
repository (Windows 11, Android SDK 36, JDK 24, Rust stable 1.94.x). Sections
that need hardware this host does not have say so explicitly (see
`docs/STATUS.md` for the verification matrix).

## The model (D-01, D-02)

The master is always the gRPC **client**. Every executor is a gRPC **server**:

- web/webOS/Tizen → the `impresario-connector` process (spawned or dialed by
  the master),
- Android → an **instrumentation process** started over `adb`,
- iOS/tvOS → a UI-test runner started with `iproxy`/direct network.

`impresario devices check` probes one device end to end: transport →
handshake → `Shutdown` (no `Session` stream).

Device config fields used below: `target_endpoint` (platform-specific address),
`app_id` (Android package / Apple bundle id), `app_url` (web family),
`agent_address` (only where the master dials directly), `agent_port`,
`options`.

---

## 1. Web / Chromium — platforms `web`, `webos`, `tizen` (connector)

### Setup — verified on this host (headless Chrome)

- A Chromium-based browser is required: `$CHROME_BIN` (must name a file), else
  the well-known names on `PATH` (`google-chrome`, `chrome`, `chromium`, …),
  else OS-specific install locations (I-35). Nothing found → `DEVICE_UNAVAILABLE`
  with an explanation.
- `impresario-connector` is resolved as `$IMPRESARIO_CONNECTOR_BIN`, else a
  sibling of the master binary, else `impresario-connector` on `PATH` (I-33).
  A loopback `agent_address` that already has something listening is used
  directly without spawning anything.
- `target_endpoint` values:
  - `launch` / `launch:headless` — the connector starts Chromium with a private
    profile and owns the process;
  - `ws://host:port` — an existing CDP endpoint (Chrome started with
    `--remote-debugging-port=9222`); an endpoint without `/devtools/page/…`
    resolves its page through `/json/list`, filtered by `options.page_match`
    (I-38).

Example device:

```yaml
devices:
  - name: desktop_chrome
    platform: web
    target_endpoint: launch:headless
    app_url: file:///D:/fixtures/web/index.html
```

Commands (verified):

```powershell
cargo build --workspace
cargo test -p impresario-connector     # headless-Chrome fixture suite (spec 10.3)
impresario devices check --device desktop_chrome
```

Notes:

- This host's port `50051` is held by an unrelated service (`multipassd`); the
  example workspace pins the connector to `127.0.0.1:47654` via
  `agent_address`.
- `cdp_backend: chromiumoxide` is an option-level switch (D-03); the typed
  backend needs the browser websocket from `/json/version` (I-39).

### webOS (TV) — not verified here (no TV hardware)

1. On the TV: install/enable the **Developer Mode** app, turn on Dev Mode
   Status and the Key Server, note the passphrase.
2. Host: register the TV with the webOS CLI (`ares-setup-device`, SSH port
   9922, passphrase from step 1) and check it with `ares-device -i`.
3. Forward the app's inspector socket with `ares-inspect` (it prints the
   forwarded local port and opens the app with inspection enabled).
4. Point `target_endpoint` at the forwarded CDP endpoint
   (`ws://127.0.0.1:<forwarded-port>` — page resolution picks the page), set
   `app_url`, and keep `options.cdp_backend: raw`: old TV Chromium builds break
   strict typed bindings (D-03).

### Tizen (TV) — not verified here (no TV hardware)

1. On the TV: **Developer mode** (Apps panel; enter the host PC's IP), then
   reboot the TV so the developer mode daemon accepts connections.
2. Host: connect the SDB bridge (`sdb connect <tv-ip>:26101`) — Tizen Studio's
   `sdb` (or the standalone binary) must be on `PATH`.
3. Forward the app's web-inspector port to a local port (`sdb forward
   tcp:<local> tcp:<remote>` once the app is running and exposes it).
4. Same as webOS: `target_endpoint: ws://127.0.0.1:<local>`,
   `options.cdp_backend: raw`.

In both cases the D-pad verbs (`key`, `focus`, `scroll`) are supported through
the connector's JS key dispatch; `options.key_dispatch` picks the mechanism
(I-46).

---

## 2. Android — platforms `android`, `androidtv` (adb + instrumentation agent)

The Android executor is an **instrumentation process launched from the agent's
own app** (I-62): the master installs three APKs — the agent app `:agent`
(`com.impresario.agent`), its `am instrument` test APK and, for the acceptance
fixture, the separate app-under-test stub (`com.impresario.stub`) — starts
`AgentRunner` over `adb`, then **dials the agent over the network**:
`options.agent_address` first, else the adb serial's own host when it is
`ip:port` (adb-over-TCP), else the device's address from `ip route` /
`ip -f inet addr show`. `adb forward` remains the fallback for devices with no
reachable address. The agent binds `0.0.0.0`, so every command travels over
TCP, never through adb; `adb` is used only for device checks, APK installs,
`am instrument` and logcat. The JVM-library parts (`agent-core`) are plain
Kotlin tests.

### 2.1 Prerequisites

- Android SDK with `platform-tools` (`adb`), `emulator`, `cmdline-tools`
  (`sdkmanager`, `avdmanager`) and `build-tools;36.1.0` (for `aapt`).
  On this host: `C:\Users\user\AppData\Local\Android\Sdk`.
- JDK 17 or newer (this host: JDK 24).
- Environment: `ANDROID_HOME`/`ANDROID_SDK_ROOT` pointing at the SDK (the
  Gradle build reads it or a local `local.properties`), `adb` on `PATH` or
  called by absolute path.

### 2.2 Build the agent (exact commands, verified)

```powershell
cd agents/android
.\gradlew.bat test              # agent-core JVM tests (dispatcher, focus, sessions)
.\gradlew.bat assembleDebug     # all three APKs
```

Outputs:

```
agents/android/agent/build/outputs/apk/debug/agent-debug.apk                            # agent app (instrumentation target)
agents/android/agent-instrumentation/build/outputs/apk/debug/agent-instrumentation-debug.apk
agents/android/agent-target-stub/build/outputs/apk/debug/agent-target-stub-debug.apk    # acceptance stub (the app under test)
```

Notes:

- This host's Gradle needs `-Djava.net.preferIPv6Addresses=true`
  (pre-recorded in `gradle.properties`): IPv4 egress to the plugin repositories
  times out here, IPv6 works.
- The Android modules request `agent-core`'s **jar** artifact explicitly: with
  AGP 9.4.1 a Kotlin/JVM project dependency is otherwise dexed from only
  `build/classes/java/main` of its `classes` variant, dropping the Kotlin
  classes from the APK (runtime `NoClassDefFoundError`; I-56).
- The first build downloads the wrapper distribution and the plugin set; a
  full `assembleDebug` takes minutes, later builds are incremental.

### 2.3 Create the emulator (exact commands, verified)

```powershell
$env:SKIP_JDK_VERSION_CHECK = "1"        # avdmanager check vs JDK 24
sdkmanager "platform-tools" "emulator" "build-tools;36.1.0" `
  "system-images;android-36.1;google_apis_playstore;x86_64"
avdmanager create avd -n impresario36 `
  -k "system-images;android-36.1;google_apis_playstore;x86_64" -d pixel_6
```

Then raise the AVD's memory — the stock 2 GB panics the guest during boot
(`Kernel panic - not syncing: System is deadlocked on memory`) — in
`%USERPROFILE%\.android\avd\impresario36.avd\config.ini`:

```ini
hw.ramSize = 4096
vm.heapSize = 512
```

Start it and wait for the framework (add `-no-window` to run headless):

```powershell
emulator -avd impresario36 -no-audio -no-boot-anim -no-snapshot `
  -gpu swiftshader_indirect -port 5554
adb wait-for-device
adb shell getprop sys.boot_completed       # "1" when the framework is up
```

- **Guest graphics bug of this image + emulator pair (I-57), independent of
  the GPU mode:** SystemUI registers a `SurfaceControl` sampling listener
  (`dumpsys SurfaceFlinger` shows the sampler busy while it runs) and the
  sampling thread trips

  `Assertion failed: !rcEnc->featureInfo()->hasReadColorBufferDma`
  (`GoldfishMapper::readFromHost` under `RegionSamplingThread`), aborting
  SurfaceFlinger every ~10 s and restart-looping the framework; `am
  instrument` then dies with `DeadObjectException`. `-gpu guest`, `-gpu host`
  and `-gpu swiftshader_indirect` all reproduce it; the emulator's `-feature`
  switches (`-GLDMA`, `-GLDMA2`) change nothing.
- **Workaround — disable the sampling client** (persists on the AVD; `pm
  enable` undoes it):

  ```powershell
  adb shell pm disable-user --user 0 com.android.systemui
  adb shell pidof surfaceflinger     # stable for minutes afterwards
  ```

  With SystemUI disabled the framework is stable in windowed and headless
  mode and the instrumented suite completes.
- First boot on a nested-virtualisation host is slow (several minutes); wait
  for `sys.boot_completed=1` before running anything.
- The AVD is **retired**: the current acceptance device for this repository is
  the real Android TV in section 2.5. Keep this emulator section for hosts
  without hardware.

### 2.4 Run the instrumented tests (exact commands, verified)

```powershell
$adb = "$env:ANDROID_HOME\platform-tools\adb.exe"
# Order matters: the agent app (the instrumentation target) is installed first.
& $adb install -r agents/android/agent/build/outputs/apk/debug/agent-debug.apk
& $adb install -r agents/android/agent-instrumentation/build/outputs/apk/debug/agent-instrumentation-debug.apk
& $adb install -r agents/android/agent-target-stub/build/outputs/apk/debug/agent-target-stub-debug.apk

# acceptance suite (driver + gRPC end-to-end); on the TV give each class its
# own invocation instead - one agent process must stay under ~223 s (I-68, 2.5)
& $adb shell am instrument -w -r `
  -e class com.impresario.agent.DriverInstrumentedTest,com.impresario.agent.AgentServerInstrumentedTest `
  com.impresario.agent.test/androidx.test.runner.AndroidJUnitRunner

# one class
& $adb shell am instrument -w -r -e class com.impresario.agent.DriverInstrumentedTest `
  com.impresario.agent.test/androidx.test.runner.AndroidJUnitRunner

# what the Rust transport starts (blocking server, no JUnit reporting)
& $adb shell am instrument -w -r -e class com.impresario.agent.AgentRunner `
  -e port 50061 com.impresario.agent.test/androidx.test.runner.AndroidJUnitRunner
```

`AgentRunner.serve` is an entry point, not a test: a package-wide run would
execute it and block forever (it serves until a `Shutdown` RPC arrives), so
select the test classes explicitly (or `-e notClass
com.impresario.agent.AgentRunner`).

Package layout (I-62), matching the Rust constants:

| APK | Package | Notes |
|---|---|---|
| agent app | `com.impresario.agent` | instrumentation target; the only APK with `INTERNET` |
| instrumentation | `com.impresario.agent.test` | `targetProjectPath = ":agent"` |
| stub (app under test) | `com.impresario.stub` | no permissions; view ids resolve as `com.impresario.stub:id/...` at runtime (I-58) |
| runner class | `com.impresario.agent.AgentRunner` | started with `-e class … -e port <n>` |
| JUnit runner | `androidx.test.runner.AndroidJUnitRunner` | |

You can confirm both packages with
`aapt dump badging <apk> | Select-String package`.

### 2.5 Real Android TV (verified on this host)

Device: **Smart TV Pro** (`G03_4K_GB` / `BeyondTV4`), `adb` serial
`192.168.93.84:5555` (adb itself over TCP), Android 11 (SDK 30),
armeabi-v7a, `ro.build.characteristics=nosdcard,tv`. There is no USB
connection; `adb connect` first:

```powershell
$adb = "$env:ANDROID_HOME\platform-tools\adb.exe"
& $adb connect 192.168.93.84:5555
# install order: agent app (the instrumentation target) → instrumentation → stub
& $adb -s 192.168.93.84:5555 install -r agents/android/agent/build/outputs/apk/debug/agent-debug.apk
& $adb -s 192.168.93.84:5555 install -r agents/android/agent-instrumentation/build/outputs/apk/debug/agent-instrumentation-debug.apk
& $adb -s 192.168.93.84:5555 install -r agents/android/agent-target-stub/build/outputs/apk/debug/agent-target-stub-debug.apk

& $adb -s 192.168.93.84:5555 shell am instrument -w -r `
  -e class com.impresario.agent.DriverInstrumentedTest `
  com.impresario.agent.test/androidx.test.runner.AndroidJUnitRunner
& $adb -s 192.168.93.84:5555 shell am force-stop com.impresario.agent   # fresh process
& $adb -s 192.168.93.84:5555 shell am instrument -w -r `
  -e class com.impresario.agent.AgentServerInstrumentedTest `
  com.impresario.agent.test/androidx.test.runner.AndroidJUnitRunner
```

Give every test class its own `am instrument` invocation (one agent process
per class): the vendor freezer's first strike on the agent lands ~223 s after
the process starts, so a process that runs both classes always crosses that
line mid-wire-test and wedges (I-68).

**Keep the TV awake for the whole run, and keep every agent process short**
(`docs/DECISIONS.md` I-67, I-68, I-69: TCL's `FreezeManager`
cgroup-freezes background processes — while the TV dreams, and even with the
screen on, because the agent hosts no visible activity — and a frozen agent
shows no logs, no ANR and no progress until an input event wakes the panel;
freezes landing on in-flight calls wedged whole suite runs). The stay-awake
settings below remove the dream; `ProcessGuardService` (started automatically
by `AgentRunner.serve` and the suites, same process) defers the freezer's
first strike on the agent to ~223 s after the process starts, and the
per-class invocations above keep every process under that line:

```powershell
& $adb -s 192.168.93.84:5555 shell svc power stayon true
& $adb -s 192.168.93.84:5555 shell settings put global stay_on_while_plugged_in 7
& $adb -s 192.168.93.84:5555 shell settings put system screen_off_timeout 2147483647
& $adb -s 192.168.93.84:5555 shell settings put secure screensaver_enabled 0
& $adb -s 192.168.93.84:5555 shell dumpsys deviceidle whitelist +com.impresario.agent
& $adb -s 192.168.93.84:5555 shell cmd appops set com.impresario.agent RUN_ANY_IN_BACKGROUND allow
```

Diagnose a suspected freeze with
`& $adb -s 192.168.93.84:5555 logcat -d -v time | Select-String 'FreezeManager'`:
`doFreeze <pid> pro com.impresario.agent res true` means the vendor power
manager froze the agent; any remote/key input wakes the panel and unfreezes it.
With the guard service live (I-69) such lines still come, but at most once per
process, ~223 s after it started — which is why the runner keeps processes
short instead of trusting the guard alone.

**Run the suites through the watchdog script** — it is the safety net for this
TV's vendor power management: it runs each class in its own fresh
`am instrument` process (freeze timer restart), and if a class ever stops
producing output (a freeze landing on an in-flight call looks exactly like
this — I-68), it force-stops the agent and retries instead of hanging the
cycle. It runs the whole cycle automatically (preflight with a11y probe,
force-stop before every set, per-set stall watchdog, retries):

```powershell
powershell -ExecutionPolicy Bypass -File agents/android/tools/run-tv-suite.ps1
```

Each set keeps `attempt-N-<set>-*.log` (instrumentation output) and
`attempt-N-<set>-*.log.device` (filtered logcat: a11y, FreezeManager, ANR)
under `%TEMP%\impresario-suite`; a stalled set is force-stopped, and the exit
code is 0 only when every set ran to completion with 0 failures (JUnit prints
`OK (N tests)` for that; `Tests run: N,  Failures: M` appears only on failure).

The suite is green end to end on this TV (acceptance run, attempt 1): each
class runs in its own fresh process (I-68) — the driver class **26 tests in
160 s** and the wire class **2 tests in 100 s**, both `OK`, with **zero**
`doFreeze` on any impresario package in either window while the freezer swept
other processes 16 times (before the guard the agent was frozen every ~20–40 s;
with it the first strike lands ~223 s after process start, which per-class
processes stay under).

What the suites prove on the TV (the class kdocs carry the full matrix):

- **Everything the phone run proved** (section 2.4's list) except the
  touch-only verbs, which are rejected instead of skipped: `swipe` is
  `UNSUPPORTED_ACTION` on `androidtv`.
- **D-pad semantics**: `click` = move focus onto the element + OK; `focus`
  check reads D-pad focus; `type` = focus + OK (opens the leanback IME) +
  `setText` with read-back; `scroll` = D-pad presses on the focused element;
  TV-only keys (`ok`, colours, media) work while phone-only negatives
  (`ok` on `android`) stay `UNSUPPORTED_ACTION`.
- **App lifecycle over the wire** (I-62): `launch`, `background` (pressHome;
  `launch` resumes the task), `restart`, `terminate` and clear-data all run
  inside the instrumented suites without touching the agent, plus the master
  E2E (section 2.6) drives the same lifecycle from the workspace config.
- **Cross-app control**: the agent app (`com.impresario.agent`) drives the
  stub (`com.impresario.stub`) through UiAutomator; runtime ids resolve to
  `com.impresario.stub:id/...` (I-58) and the agent declares
  `QUERY_ALL_PACKAGES` so it can see any app under test (I-62).
- The stub is focus-first on every screen (all rows/buttons focusable, no
  touch-only affordances), so one APK serves both phone and TV.

`docs/DECISIONS.md` I-62 records the architecture change that made this
possible; the earlier in-process phone run is superseded (its host device
was removed).

### 2.6 Drive it from the master

```yaml
devices:
  - name: living_room_tv
    platform: androidtv                  # `android` for phones
    target_endpoint: 192.168.93.84:5555  # adb serial (host:5555 = adb over TCP)
    app_id: com.impresario.stub          # the app under test
    options:
      clear_app_data: true               # `pm clear` before each reset
```

- The master installs the three APKs, starts the instrumentation with
  `-e port`, then **dials the agent over the network**: `options.
  agent_address` if set, else the serial's own host for `ip:port` serials,
  else `ip route`/`ip -f inet addr show` on the device; `adb forward` is the
  fallback when the device has no reachable address. All commands, checks,
  events and screenshots travel over that TCP connection — adb is used only
  for device checks, installs, `am instrument` and logcat.
- Every reset policy works against any target (I-62): the agent is its own
  process, so `terminate`/`restart`/clear-data/`background` of the app under
  test never kill the agent. The E2E workspace sets `app_reset: per_play`
  plus `clear_app_data: true` and the smoke play asserts `taps=` counters
  reset across `app restart`.
- The master runs `pm clear` on the app under test before a reset and
  `am force-stop` on the agent package at teardown (spec 9).
- Verified end-to-end on 2026-10-18: `impresario run` drove the full smoke
  play over Wi-Fi — `run 20261018-094855-ddb9: passed (1m46s, exit 0)`,
  1 device, 1 play, 89 steps, 0 failed. Act 1 "Scenario tour" (1m02s) covers
  basic controls, flags, D-pad `scrollto` to row 29 and back, text entry,
  selection, key log and device info; Act 2 "App control lifecycle" (19.5s)
  covers `background`/`launch`, `restart` (counter reset) and
  `terminate`/relaunch. The agent process lived ≈ 100 s — inside the ~223 s
  freeze budget (I-68).
- `StreamLogs` opens with a marker entry (I-74): grpc-java sends response
  headers only with the first message, and the master awaits the log stream
  before its first command — an idle log bus would hang the run forever with
  no output (observed 2026-10-18). The agent emits "log stream opened at min
  level …" immediately, and the master gives up after `LOG_STREAM_OPEN_TIMEOUT`
  (5 s) and continues without agent logs.

### 2.7 Platform notes

- Phone vs TV behaviour is chosen at runtime from the handshake platform
  (D-13): `androidtv` adds `DPAD_FOCUS` and routes `click`/`focus` through the
  focus navigator; `swipe` on TV is `UNSUPPORTED_ACTION`.
- `longpress` on `androidtv` is a real key hold: after D-pad focus the driver
  injects down → `FLAG_LONG_PRESS` repeat → up over ≥ 600 ms, so the focused
  view's own long-press timer fires and no trailing click is delivered
  (I-66); `input keyevent --longpress` cannot express a hold — all three
  events carry the same timestamp and the up degrades into a click.
- Screenshots use `UiDevice.takeScreenshot()`; windows with `FLAG_SECURE`
  render black (documented limitation).
- App logs are not part of the agent's `StreamLogs` (agent-internal logs
  only, D-11) — collect `adb logcat` host-side in the master.
- `adb` over Wi-Fi: `adb connect <device-ip>:5555` makes the serial address a
  network endpoint; the master then dials `192.168.x.y:50061` directly
  (`agent_port`, `options.agent_address` overrides the discovery).
- Only the agent app needs `INTERNET` (I-62); the app under test is driven
  cross-app and needs no permissions (the stub declares none).
- `terminate`/`restart`/clear-data/`background` of the app under test never
  touch the agent (it is a separate process, I-62) — all reset policies work
  against any target, including the stub.

---

## 3. Apple — platforms `ios`, `tvos`

Setup, signing and the `iproxy` tunnel are documented together with the Apple
agent sources in Phase 6; the host of this repository has no Swift/Xcode
toolchain, so nothing in this section has been executed here yet (see
`docs/STATUS.md`).
