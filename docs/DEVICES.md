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

`impresario device check` probes one device end to end: transport →
handshake → `Shutdown` (no `Session` stream). `devices check` remains an alias.

**Devices and apps are separate** (D-21, TASK-2 §5). A device owns how to reach
the hardware; an app owns what runs on it. Fields used below:

```yaml
# impresario.devices.config (workspace) / ~/.impresario/devices.yaml (global)
devices:
  - name: living_room_tv
    platform: webos            # web | webos | tizen | android | androidtv | ios | tvos
    connection:
      transport: ssh           # auto | usb | tcp | network | cdp | launch | ssh | sdb
      host: 192.168.1.50       # where the master reaches the platform service
      port: 9922               # optional; defaults: webOS 9922, Tizen 26101, Android 5555
      endpoint: ws://192.168.1.50:9998   # device-side CDP/agent endpoint
    auth: { key: living_room_tv }        # references, never secret values
    labels: [tv, living_room]
    options:
      connector_address: 127.0.0.1:47654  # where the master's connector listens
      cdp_backend: raw                    # web family only
      key_dispatch: cdp                   # web family only
      page_match: "index.html"            # web family only

  - name: pocket_phone
    platform: android
    connection:
      transport: tcp
      host: 192.168.1.61
      port: 5555
      # serial: emulator-5554            # USB instead of TCP
    options:
      agent_port: 50061                   # Android/Apple agent port

  - name: iphone
    platform: ios
    connection:
      transport: usb
      udid: 00008110-...                  # Apple UDID
      # endpoint: 192.168.1.42:50062      # WLAN agent override
    options:
      animations_disabled: true           # iOS/tvOS only
```

```yaml
# impresario.apps.config (workspace) / ~/.impresario/apps.yaml (global)
apps:
  - name: shop
    platforms:
      android: { id: com.example.shop, package: ./build/app.apk }
      webos:   { id: com.example.shop, package: ./build/shop.ipk }
      web:     { url: http://localhost:8080/index.html }
    options:
      clear_data: true
```

A play binds with `App: shop` (after `Play:`/`Description:`, before `Var:`);
`--app shop` overrides it. Global and workspace layers merge by name, and the
workspace wins. `impresario device add`/`app add` default to the global layer;
pass `--workspace` to edit the workspace file. Secrets stay in the OS
credential store and are referenced by `auth.key`/`auth.identity`/
`auth.profile`.

---

## 1. Web / Chromium — platforms `web`, `webos`, `tizen` (connector)

### Setup — verified on this host (headless Chrome)

- A Chromium-based browser is required: `$CHROME_BIN` (must name a file), else
  the well-known names on `PATH` (`google-chrome`, `chrome`, `chromium`, …),
  else OS-specific install locations (I-35). Nothing found → `DEVICE_UNAVAILABLE`
  with an explanation.
- `impresario-connector` is resolved as `$IMPRESARIO_CONNECTOR_BIN`, else a
  sibling of the master binary, else `impresario-connector` on `PATH` (I-33).
  A loopback `options.connector_address` that already has something listening
  is used directly without spawning anything.
- `connection.endpoint` values:
  - `launch` / `launch:headless` — the connector starts Chromium with a private
    profile and owns the process;
  - `ws://host:port` — an existing CDP endpoint (Chrome started with
    `--remote-debugging-port=9222`); an endpoint without `/devtools/page/…`
    resolves its page through `/json/list`, filtered by `options.page_match`
    (I-38).

Example device and app:

```yaml
devices:
  - name: desktop_chrome
    platform: web
    connection:
      transport: launch
      endpoint: launch:headless

apps:
  - name: desktop_page
    platforms:
      web:
        url: file:///D:/fixtures/web/index.html
```

Commands (verified):

```powershell
cargo build --workspace
cargo test -p impresario-connector     # headless-Chrome fixture suite (spec 10.3)
impresario device check --device desktop_chrome
impresario run --play plays/login.play --device desktop_chrome --app desktop_page
```

Phase 2 theatrical acceptance (2026-10-20): the legacy web fixture workspace
was migrated with `impresario migrate` (table scene, generated
`theatres/web_smoke.theatre`, play rewritten to `Perform:`), validated with 0
warnings, and run green — `run 20261020-133846-23c0: passed (1.87 s, exit 0)`,
9 steps, 0 failed, including click/type/select/scrollto. The run used the
`launch:headless` device and the local `fixtures/web/index.html`.

Notes:

- This host's port `50051` is held by an unrelated service (`multipassd`); a
  workspace can pin the connector with
  `options.connector_address: 127.0.0.1:47654`. Without the pin the master sees
  the unrelated listener and the handshake fails; this was hit and fixed during
  the Phase 2 run.
- `cdp_backend: chromiumoxide` is an option-level switch (D-03); the typed
  backend needs the browser websocket from `/json/version` (I-39).

### webOS (TV) — not verified here (no TV hardware)

1. On the TV: install/enable the **Developer Mode** app, turn on Dev Mode
   Status and the Key Server, note the passphrase.
2. Host: point `connection.host` at the TV and keep the default SSH port
   (`9922`). Impresario's own Rust client fetches
   `http://<tv>:9991/webos_rsa` and authenticates as `prisoner` (I-99); the
   built-in key/install flow is Phase 2+ work, so today the CDP endpoint must
   already be reachable.
3. Point `connection.endpoint` at the inspector's CDP endpoint
   (`ws://<tv-or-forwarded-host>:<port>` — page resolution picks the page) and
   set the app's `platforms.webos.id` (and `package` when it should be
   installed). Keep `options.cdp_backend: raw`: old TV Chromium builds break
   strict typed bindings (D-03).

### Tizen (TV) — not verified here (no TV hardware)

1. On the TV: **Developer mode** (Apps panel; enter the host PC's IP), then
   reboot the TV so the developer mode daemon accepts connections.
2. Host: connect the SDB bridge with impresario's own Rust client (I-93;
   `connection.transport: sdb`, default port `26101`). The built-in install and
   forward commands are Phase 2+ work, so today the CDP endpoint must already
   be reachable.
3. Same CDP shape as webOS: `connection.endpoint: ws://<tv>:<inspector-port>`,
   `options.cdp_backend: raw`, and the app's `platforms.tizen.id`/`package`.

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
`connection.endpoint` first, else the adb serial's own host when it is
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

Device: **Smart TV Pro** (`G03_4K_GB` / `BeyondTV4`), currently
`adb` serial `10.102.93.233:5555` (adb itself over TCP; the port/address
follow the network), Android 11 (SDK 30), armeabi-v7a,
`ro.build.characteristics=nosdcard,tv`. The 2026-10-18 commands below used
`192.168.93.84:5555`; substitute the current serial. There is no USB
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

**Check the Android user profile after an install.** This TV has a secondary
user 10, and `adb install` once landed the agent only there while
`adb shell am instrument` ran as user 0, producing
`INSTRUMENTATION_FAILED: … Unable to find instrumentation target package:
com.impresario.agent` even though `pm list packages` showed the APK (observed
2026-10-20). Reinstall all three for the owner user:

```powershell
$tv = "10.102.93.233:5555"
& $adb -s $tv install --user 0 -r -t agents/android/agent/build/outputs/apk/debug/agent-debug.apk
& $adb -s $tv install --user 0 -r -t agents/android/agent-instrumentation/build/outputs/apk/debug/agent-instrumentation-debug.apk
& $adb -s $tv install --user 0 -r -t agents/android/agent-target-stub/build/outputs/apk/debug/agent-target-stub-debug.apk
& $adb -s $tv shell pm list users          # 0 = Owner
& $adb -s $tv shell pm list packages --user 0 | Select-String impresario
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

Revert them when the TV goes back to normal duty (restores the framework
defaults — `settings delete` where a stock default exists, `0`/`default`
otherwise):

```powershell
& $adb -s 192.168.93.84:5555 shell svc power stayon false
& $adb -s 192.168.93.84:5555 shell settings put global stay_on_while_plugged_in 0
& $adb -s 192.168.93.84:5555 shell settings delete system screen_off_timeout
& $adb -s 192.168.93.84:5555 shell settings put secure screensaver_enabled 1
& $adb -s 192.168.93.84:5555 shell dumpsys deviceidle whitelist -com.impresario.agent
& $adb -s 192.168.93.84:5555 shell cmd appops set com.impresario.agent RUN_ANY_IN_BACKGROUND default
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
    connection:
      transport: tcp
      host: 10.102.93.233                # adb over TCP; use serial for USB
      port: 5555
      # endpoint: 10.102.93.233:50061    # optional agent-address override

apps:
  - name: stub
    platforms:
      androidtv:
        id: com.impresario.stub
    options:
      clear_data: true                   # `pm clear` before each reset
```

- The master installs the three APKs, starts the instrumentation with
  `-e port`, then **dials the agent over the network**: `connection.endpoint`
  if set, else the serial's own host for `ip:port` serials, else `ip route`/
  `ip -f inet addr show` on the device; `adb forward` is the fallback when the
  device has no reachable address. All commands, checks,
  events and screenshots travel over that TCP connection — adb is used only
  for device checks, installs, `am instrument` and logcat.
- Every reset policy works against any target (I-62): the agent is its own
  process, so `terminate`/`restart`/clear-data/`background` of the app under
  test never kill the agent. The E2E workspace sets `app_reset: per_play`
  plus the app's `clear_data: true` and the smoke play asserts `taps=` counters
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
- Phase 2 theatrical re-run (2026-10-20): the legacy TV workspace was migrated
  with `impresario migrate` (table scene, generated
  `theatres/tv_smoke.theatre`, play rewritten to `Perform:`), validated with 0
  warnings, and run green — `run 20261020-133548-61d9: passed (1m38s, exit 0)`,
  2 acts, 89 steps, 0 failed, including D-pad `scrollto`, text entry,
  selection, key log and the app-control lifecycle. The initial `device check`
  failed on the TV user-profile quirk above (agent installed only for user 10);
  after `install --user 0` for all three APKs it passed.
- `StreamLogs` opens with a marker entry (I-74): grpc-java sends response
  headers only with the first message, and the master awaits the log stream
  before its first command — an idle log bus would hang the run forever with
  no output (observed 2026-10-18). The agent emits "log stream opened at min
  level …" immediately, and the master gives up after `LOG_STREAM_OPEN_TIMEOUT`
  (5 s) and continues without agent logs.

### 2.7 Android phones (Samsung Galaxy A51 + Realme RMX3997, verified on this host)

The same three APKs serve phones; the platform comes from the handshake
(`android` selects the touch path: no D-pad, `swipe` allowed, `focus`
unsupported — D-13/I-57).

```yaml
devices:
  - name: pocket_phone
    platform: android
    connection:
      transport: tcp
      host: 10.102.93.196                 # wireless debugging
      port: 37565                        # random; re-read on the phone

apps:
  - name: stub
    platforms:
      android:
        id: com.impresario.stub
    options:
      clear_data: true
```

- Connect first: `adb connect 10.102.93.196:37565` (2026-10-20; the address
  and port follow the session). Wireless-debugging ports are random and rotate
  per session, so re-read the port on the phone when the connection drops.
  Verified devices: Realme RMX3997, Android 16 (SDK 36), 720×1604 @ 2.0, and
  SM-A515F, Android 13 (SDK 33), 1080×2400 @ 420 dpi.
- Touch semantics on this path: `click`/`longpress` tap the element,
  `scroll`/`swipe` are real gestures, `scrollto` searches down and reverses
  as soon as the container stops moving (I-75: the driver reports movement
  from uiautomator's scroll-finished signal), `key backspace`/`back`/`home`
  dispatch KeyEvents, `app background` presses HOME.
- Keep the screen unlocked for a run (the app must be visible); the stock
  10-minute screen-off timeout is ample for a ≈2-minute play.
- Verified end-to-end on 2026-10-18: `impresario run` drove the full touch
  smoke play over Wi-Fi — `run 20261018-103702-ee29: passed (1m53s, exit 0)`,
  1 device, 1 play, 2 acts, 86 steps, 0 failed. Act 1 "Scenario tour"
  (1m19s) covers basic controls, flags, the touch scroller (`scroll down 2`,
  `swipe up`/`swipe down`, `scrollto` to row 29 and back to row 0), text
  entry (`type`/`append`/`clear`, `key backspace` with echo checks),
  selection, hardware keys (`key back`, `key home` + relaunch) and device
  info (`tv=false`); Act 2 "App control lifecycle" (16.3s) covers
  `background`/`launch`, `restart` (counter reset) and `terminate`/relaunch.
  The first phone attempts failed `scrollto` both ways and produced the I-75
  fix (the TV regression was re-run green afterwards: wire suite 26+2, TV
  E2E `run 20261018-104405-4f10`).
- Second verified phone (2026-10-19): **Realme RMX3997, Android 16 (SDK 36)**,
  720×1604 @ 2.0, wireless-debugging serial `10.102.93.196:37565` on
  2026-10-20 (the port rotates per session). Driven
  end-to-end **through the MCP server alone** (decision I-80): `devices_check`
  reported the device, then `run` of the bare play name `mobile_smoke.play`
  passed — `run 20261019-061207-92f9: passed (1m43s, exit 0)`, 2 acts,
  86 steps, 0 failed; the screenshot came back through
  `impresario_artifact_get` as an image and the report through
  `impresario_report_get`.
- **`pm clear` is denied on this ROM** (found on the Realme while re-running a
  smoke play): `pm clear com.impresario.stub` answers
  `SecurityException: PID … does not have permission
  android.permission.CLEAR_APP_USER_DATA to clear data of package …` for
  *every* package, debuggable included; `cmd package clear`, `pm clear
  --user 0` and clear-after-force-stop behave identically, so the device
  policy (not the package) is the cause. The agent treats a refused clear as
  a WARN on the log stream plus a force-stop cold start (I-82): the
  `clear_data: true` app option still relaunches the app but cannot guarantee
  empty data on this device. `run 20261019-064750-dca4` (the build with the
  I-82 fix) passed through MCP afterwards.
- Phase 2 theatrical re-run (2026-10-20) on the same Realme: the legacy phone
  workspace was migrated with `impresario migrate` (table scene, generated
  `theatres/mobile_smoke.theatre`, play rewritten to `Perform:`). The migrated
  scene declared four TV-only pieces the touch play never used, so they were
  removed to clear `IMP-DSL-043`; it then validated with 0 warnings and ran
  green — `run 20261020-134217-2dfe: passed (1m35s,
  exit 0)`, 2 acts, 86 steps, 0 failed. The expected `pm clear` denial warning
  above appeared once and the I-82 cold-start fallback kept the run green.

### 2.8 Platform notes

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
  (`options.agent_port`, `connection.endpoint` overrides the discovery).
- Only the agent app needs `INTERNET` (I-62); the app under test is driven
  cross-app and needs no permissions (the stub declares none).
- `terminate`/`restart`/clear-data/`background` of the app under test never
  touch the agent (it is a separate process, I-62) — all reset policies work
  against any target, including the stub.

---

## 3. Apple — platforms `ios`, `tvos`

Two halves with different reachability:

* the **SwiftPM package** (`agents/apple`) builds and unit-tests on any Swift
  host, including Linux — verified here (WSL Ubuntu 22.04, Swift 6.4.1, §3.1);
* the **Xcode targets** (the XCUITest runner + host/stub apps) need macOS with
  Xcode — not available here, so §3.2 is written from the sources and the
  command reference, not from a run (recorded gap in `docs/STATUS.md`).

### 3.1 Verified here: build and unit-test the agent package (Linux/macOS)

```bash
# Ubuntu 22.04 host prerequisites, if the toolchain is fresh:
#   apt-get install zlib1g-dev libstdc++-12-dev   # headers grpc-swift/NIOSSL compile against
cd agents/apple
swift build            # AgentCore + ImpresarioWire + AgentService
swift test             # executor/dispatcher/focus (fakes) + full wire protocol over loopback gRPC
```

Measured on this host (WSL Ubuntu, Swift 6.4.1): `swift build` ≈ 15 s
incremental / ~4 min cold, `swift test` right after it. On a host with no
network access to GitHub, unpack the pinned checkouts from a mirror into
`.build/checkouts` (the `swift-nio`/`grpc-swift`/`swift-protobuf` sources) or
copy `Package.resolved` and the checkouts from another machine.

Interpreter of the two halves: `swift test` speaks the real protocol (handshake,
ordered commands, heartbeats, logs, screenshots, shutdown) against an in-process
grpc-swift server, so a wire-level regression fails here before any Mac is
involved. `SWIFTNIO_STRICT=1 swift test` (the same run with SwiftNIO's strict
mode) turns every task scheduled on an already-stopped event loop into a crash
with a stack trace — the shutdown ordering of I-90 is verified that way.

### 3.2 Not verified here: the Xcode targets (macOS host)

```bash
brew install xcodegen
cd agents/apple
xcodegen generate                                  # ImpresarioApple.xcodeproj

# the fixture app under test (what the app definition's id/package point at)
xcodebuild build -scheme AgentTargetStub-iOS \
  -destination 'platform=iOS,id=<udid>' -derivedDataPath build
xcodebuild build -scheme AgentTargetStub-tvOS \
  -destination 'platform=tvOS,id=<udid>' -derivedDataPath build

# the runner (the master starts this itself when IMPRESARIO_XCTESTRUN is set)
xcodebuild build-for-testing -scheme AgentUITests-iOS \
  -destination 'platform=iOS,id=<udid>' -derivedDataPath build
xcodebuild build-for-testing -scheme AgentUITests-tvOS \
  -destination 'platform=tvOS,id=<udid>' -derivedDataPath build
```

Device runs need a development team (set it in Xcode or a local xcconfig; the
repo pins none) and the device must trust the host. `xcrun devicectl list
devices` / `xcrun xctrace list devices` prints the udids (`xcodebuild
-destination 'platform=iOS,id=<udid>'` takes the same value).

Runner environment (the `.xctestrun`'s `TestingEnvironmentVariables`; the
master drains the `xcodebuild` output, where the runner logs its port and
addresses):

```bash
plutil -insert TestingEnvironmentVariables.IMPRESARIO_AGENT_PORT -string 50062 \
  build/Build/Products/AgentUITests-iOS_iphoneos16.0-arm64.xctestrun
plutil -insert TestingEnvironmentVariables.IMPRESARIO_AGENT_TOKEN -string <token> \
  build/Build/Products/AgentUITests-iOS_iphoneos16.0-arm64.xctestrun   # optional
plutil -insert TestingEnvironmentVariables.IMPRESARIO_NO_BONJOUR -string 1 \
  build/Build/Products/AgentUITests-iOS_iphoneos16.0-arm64.xctestrun   # optional
```

Master side:

```yaml
devices:
  - name: iphone
    platform: ios                     # or tvos
    connection:
      transport: usb                  # network over WLAN
      udid: "<udid>"                  # what xcodebuild -destination takes
      # endpoint: "192.168.1.42:50062"  # WLAN agent address; omit over USB

apps:
  - name: stub
    platforms:
      ios:
        id: com.impresario.agent.AgentTargetStub-iOS
        package: /abs/path/build/Build/Products/Debug-iphoneos/AgentTargetStub-iOS.app
    options:
      clear_data: true
```

```bash
IMPRESARIO_XCTESTRUN=build/Build/Products/AgentUITests-iOS_iphoneos16.0-arm64.xctestrun \
  impresario run --play plays/login.play --device iphone --app stub
```

Endpoint resolution for Apple devices (`AppleTransport`): a **simulator** is
reached on loopback; an **iOS device** over USB gets an `iproxy` tunnel the
transport starts itself (port `50062` by default, `IMPRESARIO_AGENT_PORT`
changes both sides); an iOS device or Apple TV over **WLAN** uses
`connection.endpoint` — the address the runner logs at start
(`impresario agent: addresses en0=…`). macOS only: without `xcodebuild`/
`iproxy` the transport answers `DEVICE_UNAVAILABLE` (I-84). These host tools
are a **recorded D-19 gap**; replacing them with the Rust `idevice` path is
Phase 2/3 work.

Clean state per test is a host-side uninstall + reinstall (`clear_data` +
the app platform's `package`, I-84); the tool is chosen automatically
(`simctl` → `devicectl` → `ideviceinstaller`, the latter from
`brew install libimobiledevice`). When none is available the option degrades to
a cold start and one WARN line.

First run after an install: iOS/tvOS ask for local-network permission. The
runner declares the usage description and tries to dismiss the alert itself
(interruption monitor + SpringBoard poll, `SystemAlerts`); if the buttons are
not reachable it logs the labels it saw and one human tap on the device grants
it for good (I-88). Uninstall + reinstall does **not** reset that permission,
so it is a one-time cost per device.

Apple TV specifics: no USB path, so WLAN is the only way (put the logged
address into `connection.endpoint`); the runner presses remote buttons through
`XCUIRemote` (D-pad/Select/Menu/PlayPause; `home` = `XCUIDevice.press(.home)`),
and the stub app's focused row is visible in its `stub_key_log`/`stub_status`
labels while a play runs.

