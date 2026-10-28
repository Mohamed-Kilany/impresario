# Devices: setup per platform

How to prepare each platform, and how to run the automated checks against it.
"Verified" means the commands ran successfully on the development host of this
repository (Windows 11, Android SDK 36, JDK 24, Rust stable 1.94.x). Sections
that need hardware or host credentials not available here say so explicitly
(see `docs/STATUS.md` for the verification matrix and the 2026-10-25 access
log).

## The model (D-01, D-02)

The master is always the gRPC **client**. Every executor is a gRPC **server**:

- web/webOS/Tizen → the `impresario-connector` process (spawned or dialed by
  the master),
- Android -> an **instrumentation process** started over Impresario's native
  ADB transport (no `adb` binary at runtime; I-121),
- iOS/tvOS → Impresario Agent inside an XCUITest bundle, started by the
  pure-Rust RemotePairing/RSD transport.

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

- A Chromium-based browser is required. Lookup order is `$CHROME_BIN` (must
  name a file), then the managed Chrome for Testing install
  (`~/.impresario/tools/chromium/<version>/<platform>`), then `PATH`
  (`google-chrome`, `chrome`, `chromium`, ...), then OS-specific install
  locations (I-35/I-123). Nothing found -> `DEVICE_UNAVAILABLE` with an
  explanation.
- Optional managed browser: `impresario tools install chromium` downloads the
  pinned Chrome for Testing build after consent (or `--yes`), verifies its
  SHA-256 (the pinned size is shown for consent), and extracts it under the
  Impresario home. `doctor` only
  reports whether it is present; a run never downloads. On Windows the
  installer also grants the AppContainer RX ACEs the sandbox needs (I-123).
  The managed build is live-verified here; `CHROME_BIN` and system Chrome
  remain supported.
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

### Media probe fixture

`fixtures/web/media.html` (with locally generated `sample.webm` VP8/Opus and
`sample.png`) contains a playing video, an audio-only element using the Opus
track, a decoded image, an image-like slot and an intentionally missing piece.
It is exercised by
`cargo test -p impresario-connector --test fixtures_suite
media_probes_collect_showing_player_and_drm_evidence`, which covers

- `Showing` coverage/decoded/source and the missing-piece FAIL;
- `Player` playing/position/duration/resolution/muted/error for video and
  audio-only playback;
- `DRM-disabled` (no EME), plus image decode/source.

`fixtures/web/hook.html` loads `sdk/web/impresario-hook.js` and reports the real
`HTMLMediaElement` state through `window.__impresarioHook`; use it to check the
optional hook path. The fixture suite skips when no Chromium is available, so
CI on a Chrome-less host is not evidence. For a real TV, the browser must be
started with a reachable CDP endpoint (`connection.endpoint: ws://…`) and the
same probes apply; that path is HD-7.

Notes:

- This host's port `50051` is held by an unrelated service (`multipassd`); a
  workspace can pin the connector with
  `options.connector_address: 127.0.0.1:47654`. Without the pin the master sees
  the unrelated listener and the handshake fails; this was hit and fixed during
  the Phase 2 run.
- `cdp_backend: chromiumoxide` is an option-level switch (D-03); the typed
  backend needs the browser websocket from `/json/version` (I-39).

### 1.3 webOS (TV) — native SSH transport (fixture-verified; LG host reachable 2026-10-25, auth blocked)

The executor is still `impresario-connector` (D-03), but the host-side device
work is Impresario's own SSH/SFTP/luna client. No `ares-*`, `ssh`, `scp`,
`openssl` or `luna-send` process runs on the host (D-19/I-138).

**TV setup**

1. Install/enable the **Developer Mode** app, turn on Dev Mode Status and the
   Key Server, and note the six-character passphrase.

**Device definition**

```yaml
devices:
  - name: living_room_tv
    platform: webos
    connection:
      transport: ssh          # auto also resolves to ssh for webos
      host: 192.168.1.50
      port: 9922              # default
    auth:
      identity: SHA256:...    # optional host-key pin (recommended)
      # key: C:\keys\living-room.pem   # optional private-key override
```

- The first connection fetches `http://<tv>:9991/webos_rsa` and stores the
  encrypted key at `~/.impresario/keys/webos/<host>.pem`; the TV passphrase is
  read from `IMPRESARIO_WEBOS_PASSPHRASE` or from keychain service
  `impresario.webos` (account = host). The environment variable is the
  headless override; do not put the passphrase in YAML.
- `auth.identity` pins the SSH host key using a `SHA256:` fingerprint, an
  OpenSSH public-key string, or a path under `keys/`. Without it the first key
  is remembered in `keys/webos_known_hosts`; a later change is rejected.
- `auth.key` may name a private-key file or `keychain:<entry>` whose stored
  value is the key itself. Usually it is omitted.

**App definition**

```yaml
apps:
  - name: shop
    platforms:
      webos:
        id: com.example.shop
        package: ./build/web      # directory or an existing .ipk
```

A directory containing `appinfo.json` is packaged deterministically into the
cache. A `.ipk` is inspected before use. `impresario package webos ./build/web`
produces the same package explicitly.

**What a run does**

1. Connects SSH as `prisoner`, verifying the pinned/TOFU host key.
2. Prepares the local connector listener.
3. Uploads the `.ipk` to `/media/developer/temp` (SFTP, exec `cat` fallback).
4. Streams `com.webos.appInstallService/dev/install` until a terminal state and
   verifies the app through `dev/listApps`; presence of a pre-existing app is
   not accepted as proof of an update.
5. Launches through `com.webos.applicationManager/launch`, waits for the
   inspector port (default 9998; `IMPRESARIO_WEBOS_INSPECTOR_TIMEOUT_MS`,
   default 20 s), and forwards it over SSH.
6. Hands `ws://127.0.0.1:<local-port>` to the connector handshake. With no
   `package`, the transport attaches to an already-installed app.

```powershell
impresario device check --device living_room_tv
impresario run --play plays/smoke.play --device living_room_tv --app shop
```

`device check` is read-only: it connects, queries `deviceInfo`/`listApps`, and
tears down; it does not install or launch. It reports a warning if luna cannot
answer rather than inventing device details.

**Hardware runbook (HD-3, HD-7 — install/launch not executed here)**

1. Record TV model/firmware, Developer Mode version, `device check` output and
   the host-key fingerprint.
2. Package a fixture app with `impresario package webos`; record its SHA-256.
3. Run `device check`, then a real run. Record the install terminal line, the
   app-list verification, the forwarded inspector port and the play report.
4. Repeat with an app already installed and a changed package to prove update
   verification; then test an invalid package and confirm the run fails.
5. Only after these pass should `HD-3`/`HD-7` be marked hardware-verified.

### 1.4 Tizen (TV) - native SDB transport (fixture-verified; Samsung SDB/upload/install path exercised 2026-10-25, certificate rejected)

The executor is still `impresario-connector`; the host-side SDB protocol,
package signing and forwarding are native Rust. No `sdb` or `tizen` process
runs on the host (D-19/I-140...I-142).

**TV setup**

1. On the TV enable **Developer mode** (Apps panel) and enter the host PC's IP.
2. Reboot the TV so the developer-mode daemon accepts connections on TCP
   `26101`.

**Device definition**

```yaml
devices:
  - name: living_room_tv
    platform: tizen
    connection:
      transport: sdb         # auto also resolves to sdb for tizen
      host: 192.168.1.51
      port: 26101            # default
    auth:
      profile: living-room   # required on Tizen 7.0+; see docs/SIGNING.md
```

SDB authenticates with a locally generated RSA key at
`~/.impresario/keys/tizen_adbkey`; the TV may show a confirmation prompt on the
first connection. `auth.profile` names an imported signing profile; older Tizen
does not require one.

**Signing profile (Tizen 7.0+)**

```powershell
$env:IMPRESARIO_TIZEN_AUTHOR_PASSWORD = '...'
$env:IMPRESARIO_TIZEN_DISTRIBUTOR_PASSWORD = '...'
impresario sign tizen profile add living-room `
  --author .\keys\author.p12 --distributor .\keys\distributor.p12
impresario sign tizen profile show living-room
```

The distributor certificate must contain the TV's DUID
(`URN:tizen:deviceid=<DUID>`). `device check` prints the DUID and warns when
the profile does not cover it or is expired.

On 2026-10-25 a temporary **self-signed** profile passed the local profile and
DUID checks and the TV accepted the upload and `0 vd_appinstall` command, but
the installer rejected the package with
`[118, -12] Invalid certificate chain with certificate in signature`.
Self-signed certificates cannot install apps on a retail TV; use a
Samsung-issued author/distributor profile that covers the DUID.

**App definition**

```yaml
apps:
  - name: shop
    platforms:
      tizen:
        id: org.example.shop
        package: ./build/shop.wgt   # .wgt; an app directory must be packaged first
```

**What a run does**

1. Connects over SDB, authenticating with the local RSA key.
2. Reads capabilities, `platform_version`, staging (`sdk_toolpath`) and DUID.
3. On Tizen 7.0+: requires `auth.profile`, signs the package for the DUID,
   validates it, and caches it by package hash/profile/DUID. On older versions:
   installs the original package.
4. Pushes to a unique, space-free `impresario-<pid>-<ms>.wgt` in the staging
   directory. Every SDB WRTE is acknowledged; unacknowledged or oversized sync
   frames make sdbd drop the connection. Runs
   `0 vd_appinstall <package-id> <remote>` using the
   `<tizen:application package=...>` id (not the dotted app id). Success is
   confirmed by `app_tizen_id`/`app_package_name` in `0 vd_applist`, not by
   trusting the install output.
5. Runs `0 debug <app-id>` for the inspector port and forwards it with a native
   SDB `tcp:` channel.
6. Hands `ws://127.0.0.1:<local-port>` to the connector handshake. Teardown
   uses `0 was_kill`.

```powershell
impresario device check --device living_room_tv
impresario run --play plays/smoke.play --device living_room_tv --app shop
```

`device check` reads capabilities/DUID/app list and profile status only; it
does not install, launch or sign.

**Hardware runbook (HD-4, HD-7 — install/launch not executed here)**

1. Record TV model/firmware, `device check` output, DUID and profile coverage.
2. On Tizen 7+, sign a fixture `.wgt`, record its SHA-256, install through a
   run, and confirm `0 vd_applist` shows the app and the play connects.
3. Repeat on Tizen < 7 with an unsigned fixture to verify the original-package
   branch (if such hardware is available).
4. Test a profile without the TV DUID, an expired certificate and a corrupted
   `.wgt`; each must fail with the documented `IMP-SIGN-*`/transport error.
5. Record the exact `0 vd_appinstall` output and whether AUTH was requested.
6. Only after these pass should `HD-4`/`HD-7` be marked hardware-verified.

**Hardware access log (2026-10-25)**

- **Tizen** — `192.168.93.37:26101`, `device check --device samsung_tizen`
  passed with the native SDB transport:
  `Tizen TV · Tizen 9.0 · 0x0 @1x · SDB (native)`; the banner is
  `device::UA55CU8500UXSA::0` (55" CU8500). The handshake completed **without
  an AUTH challenge**, so capability parsing, DUID and the app-list read are
  hardware-verified, but the RSA AUTH path is not. It also reports
  `signing required (Tizen 7.0+)`, and no profile is imported. Package signing,
  upload, `0 vd_appinstall`, `0 debug`, forwarding and playback are not
  verified. `0 vd_appinstall` success semantics remain unverified.
- **webOS** — `192.168.93.116`. TCP 9922 and 9991 are reachable. The key server
  served `/webos_rsa`; the encrypted key is
  `~/.impresario/keys/webos/192.168.93.116.pem`. `device check` fails before
  SSH authentication with `webOS key passphrase: no passphrase in the OS
  keychain`; set `IMPRESARIO_WEBOS_PASSPHRASE` from the TV's Developer Mode app
  for the session and rerun (there is no CLI command that stores the secret).
  No install/launch/CDP is verified on this TV. The 2026-10-21 four-device run used the web connector
  path, not `WebOsTransport`.
- **Apple TV** – now `192.168.93.19` (the 2026-10-27 P0 run; the
  2026-10-26 run used `10.102.93.222`).
  AirPlay `/info` reports `AppleTV5,3` (Apple TV HD), `RAMA (4)`, device ID
  `B6:FD:1D:0C:99:3C`, build `23J582`; the developer record reports UDID
  `a4d6ff71e98403878ba98fe85cb77e8d98d622e5`. Impresario Agent, the stub and
  the UI-test bundle are installed, the personalized DDI is mounted, and
  `testServe` passed with a real handshake/shutdown over the direct-RSD path
  (I-150). `AppleTransport` now uses the same pure-Rust path; `device check`
  passed on this Windows host with the pre-rename bundle ids. The renamed
  bundle ids need new profiles, and full tvOS actions/focus/playback are not
  verified.

In both TV cases the D-pad verbs (`key`, `focus`, `scroll`) are supported
through the connector's JS key dispatch; `options.key_dispatch` picks the
mechanism (I-46). Keep `options.cdp_backend: raw`: old TV Chromium builds break
strict typed bindings (D-03). On webOS, `connection.endpoint` may override the
inspector port; a native run always hands the connector its SSH-forwarded
target endpoint. On Tizen, an explicit inspector port is only a fallback when
`0 debug` does not return one.

---

## 2. Android - platforms `android`, `androidtv` (native ADB + instrumentation agent)

The Android executor is an **instrumentation process launched from the agent's
own app** (I-62): the master installs three APKs - the agent app `:agent`
(`com.impresario.agent`), its `am instrument` test APK and, for the acceptance
fixture, the separate app-under-test stub (`com.impresario.stub`) - starts
`AgentRunner` through Impresario's native `libadb` transport, then **dials the
agent over the network**: `connection.endpoint` first, else the ADB target's
own host when it is `host:port` (ADB-over-TCP), else the device's address from
`ip route` / `ip -f inet addr show`. A direct native `tcp:` forwarding channel
remains the fallback for devices with no reachable address. The agent binds
`0.0.0.0`, so every command travels over TCP; the native transport is used only
for device checks, APK installs, `am instrument`, logcat and screenshots. No
`adb` process is spawned (I-121). The JVM-library parts (`agent-core`) are
plain Kotlin tests.

### 2.1 Prerequisites

- For **using** an Android device with the product: nothing beyond the device
  and network/USB access. The product creates its own ADB host key under
  `~/.impresario/keys/adbkey` and speaks the protocol itself (D-19/I-121).
- For **building or manually debugging** the agent: Android SDK with
  `platform-tools` (`adb`), `emulator`, `cmdline-tools` (`sdkmanager`,
  `avdmanager`) and `build-tools;36.1.0` (for `aapt`). On this host:
  `C:\Users\user\AppData\Local\Android\Sdk`. These tools are development-only
  and are not invoked by a run.
- JDK 17 or newer (this host: JDK 24).
- Environment: `ANDROID_HOME`/`ANDROID_SDK_ROOT` pointing at the SDK (the
  Gradle build reads it or a local `local.properties`); the optional manual
  commands below call `adb` by absolute path.

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
      host: 10.102.93.233                # ADB-over-TCP; use serial for USB
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
  if set, else the ADB target's own host for `host:port` targets, else
  `ip route`/`ip -f inet addr show` on the device; a direct native `tcp:`
  forwarding channel is the fallback when the device has no reachable
  address. All commands, checks, events and screenshots travel over that TCP
  connection; the native ADB transport is used only for device checks,
  installs, `am instrument`, logcat and screenshots (I-121).
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
      port: 40997                        # random; re-read on the phone

apps:
  - name: stub
    platforms:
      android:
        id: com.impresario.stub
    options:
      clear_data: true
```

- Add the device with `impresario device wizard` (route `m` for an mDNS
  connect endpoint, or `p` for a fresh pairing code), or non-interactively
  with `impresario device add`; `adb connect` is only a development
  convenience and is not used by the product (I-121/I-124). Wireless-debugging
  ports are random and rotate per session, so re-read the port on the phone
  when the connection drops (the Phase 3 E2E used `:40997`). Verified devices:
  Realme RMX3997, Android 16 (SDK 36), 720x1604 @ 2.0, and SM-A515F,
  Android 13 (SDK 33), 1080x2400 @ 420 dpi.
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
  720×1604 @ 2.0, wireless-debugging serial `10.102.93.196:40997` on
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
- Phase 3 native-ADB re-run (2026-10-20) with `PATH` reduced to
  `C:\Windows\System32;C:\Windows` (`Get-Command adb` empty):
  `run 20261020-153947-986f` passed 2 acts/86 steps in 1m43s using the native
  transport, with the same expected `pm clear` WARN. A fresh Impresario key
  paired through the wireless-debugging pairing-code screen in the wizard and
  then passed `device check` (see 2.9).

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
- App logs are not part of the agent's `StreamLogs` (agent-internal logs only,
  D-11); the master collects device logcat natively for diagnostics. Over USB,
  host-side logcat is skipped because the single USB ADB session is busy with
  instrumentation; use TCP for full logcat/forwarding, or the agent log stream
  (I-121).
- ADB-over-TCP devices are the normal path; `adb connect` is development-only
  and not needed by the product. The master derives the agent address from
  `connection.endpoint`, the target host or `ip route` and dials
  `<device>:50061` directly (`options.agent_port`).
- USB: `libadb`/`nusb` needs a WinUSB-compatible ADB driver on Windows. USB
  discovery is implemented but not live-verified here; host logcat is skipped
  and the forwarding fallback refuses to run with instrumentation, returning
  explicit guidance.
- Only the agent app needs `INTERNET` (I-62); the app under test is driven
  cross-app and needs no permissions (the stub declares none).
- `terminate`/`restart`/clear-data/`background` of the app under test never
  touch the agent (it is a separate process, I-62) — all reset policies work
  against any target, including the stub.

### 2.9 Native ADB transport, discovery, pairing and doctor (Phase 3)

- No `adb` executable is started. The master opens the ADB connection with
  `libadb` over TCP or USB (`nusb`), authenticates with the key at
  `~/.impresario/keys/adbkey` (created on first use), and uses shell v2, sync
  install and native `tcp:` forwarding. On Windows, a fresh key is created by
  Impresario because of the `libadb` key-store path bug recorded as I-122.
- `impresario device discover [--json] [--timeout SECS]` browses USB, classic
  TCP and mDNS connect/pairing services. The wizard runs the same discovery
  with a 5 s mDNS window and offers `m` (manual/mDNS connect), `p` (pairing
  code), `r` (re-scan), `u` (USB serial) and numbered routes.
- `impresario device pair HOST:PORT --code 123456 [--json]` performs the
  Android 11+ SPAKE2/TLS pairing. The pairing port is different from the
  connect port: after pairing, the device shows a new wireless-debugging port
  that must be used for `device check`/runs. The wizard asks for both.
- `impresario device wizard [--edit NAME] [--workspace] [--no-check]` writes
  only after the final preview; Enter accepts every default, `b` goes back
  and `q` cancels without writing. On a phone it detects the real form factor
  and offers to switch `android`/`androidtv` if the declaration disagrees. On
  `tvos` it asks for the physical UDID and the XCUITest runner's LAN endpoint,
  writes `transport: network`, rejects simulator UUIDs, and states that
  pairing/signing/runner startup need macOS; the live check is skipped on
  non-macOS hosts.
- The TCP forwarding fallback was live-checked on the phone on 2026-10-20 by
  setting `connection.endpoint` to `127.0.0.1:59999` (unreachable). After the
  30 s network wait the transport logged the fallback, opened a native
  `tcp:` forward and `device check pocket_phone` exited 0.
- `impresario device check NAME` and `impresario doctor [--json]` are the
  non-interactive diagnostics. `doctor` reports each platform as
  `ready`/`degraded`/`unavailable`/`planned`, shows the chosen rung and fix,
  probes configured devices, and never downloads or installs. With `adb`
  absent from `PATH` on this host it reported Android, Android TV and web all
  `ready` (exit 0; `target/phase3-doctor.json`).
- Keys are per Impresario home, not the system `~/.android` store. A pairing
  code is shown on the phone under Developer options -> Wireless debugging ->
  Pair device with pairing code; it expires quickly. Do not store the phone
  PIN or the pairing code in configuration.

### 2.10 Playback probe harness (fixture-verified)

The media stub is `agent-target-stub`'s `MediaActivity`: a looping local
H.264/AAC MP4, a decoded PNG, an empty image slot and a real `MediaSession`
with play/pause/seek and D-pad focus. Build and install all three APKs as in
§2.2/§2.5; no extra permission is needed. The agent advertises
`PROBE_SHOWING`, `PROBE_PLAYER`, `PROBE_DRM` and its handshake lists
`a11y_tree`, `screenshot_region`, `media_session`, `media_drm` and `app_hook`;
`audio_playback` is explicitly unavailable because `AudioManager` is
system-wide, not app-scoped.

Enable the debug-only SDK by depending on `:impresario-hook`, calling
`ImpresarioHook.start(context)` once (the stub does it in `MediaActivity`) and
then `ImpresarioHook.player/drm/showing`; the stub reports every 250 ms. The
agent reads the latest `IMPRESARIO_HOOK <json>` line with shell identity and
ignores snapshots from another package or older than a minute. It does not
parse app private files and does not require root. Without the SDK,
decoded/source, license/key status and security level usually stay
UNVERIFIABLE; MediaSession can still establish `Playing`/`Paused`.

Expected probe behaviour on this hardware class (verify on the device, do not
assume):

- `Showing => stub_media_video with coverage>=50%` passes only when the video
  below the controls paints a non-uniform region; a black/uniform region is
  not proof of decoding.
- `Player => stub_media_video Playing` uses the MediaSession state; pausing
  through the buttons must flip it to Paused.
- `DRM-disabled` needs the hook or a web `mediaKeys` observation; an empty
  `dumpsys media.drm` is not proof (another app's session is never
  attributed). `DRM-enabled` on a third-party app needs an active session
  naming the target package or the hook; otherwise UNVERIFIABLE.
- A `Check: Showing => NoSuchPiece` is a FAIL (authoritative absence), while a
  present surface with missing media facts is UNVERIFIABLE, not FAIL.

The Android TV reference run `20261027-040844-6a42` passed all 105 stub smoke
steps, including the Media stage (`Video`, `Poster`, `Missing` and
`Player => Video Playing`; `media-playing.png`). The deeper `Showing`/
`DRM-disabled`/paused/position matrix and Android USB remain unverified
(HD-5/HD-8).

---

## 3. Apple — platforms `ios`, `tvos`

Three layers, with different reachability:

* the **SwiftPM package** (`agents/apple`) builds and unit-tests on any Swift
  host, including Linux — verified here (WSL Ubuntu 22.04, Swift 6.4.1, §3.1);
* the **Xcode targets** are compiled unsigned by the `Apple Xcode` GitHub
  Actions job. They are not executed in CI: the user rejected simulator tests
  on 2026-10-25 and the simulator workflow/scripts were removed (§3.2);
* a **physical Apple TV** has run the agent and the full 105-step stub smoke
  from Windows: RemotePairing/RSD install, DDI mount, direct-RSD XCUITest
  startup, then Home, Basic, Text, Scroll, Selection, Flags, Keys, Info,
  LongPress and Media (including `Player => Video Playing`) with back
  navigation (`run 20261027-063121-00d6`, §3.3, I-150). iOS on hardware is
  still unverified. GitHub-hosted runners cannot reach the LAN, so the device
  run is local. The run used the temporary legacy tvOS bundle ids; the
  short-id product line (now `master`) still needs fresh tvOS profiles before a
  hardware install.

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

### 3.2 Simulator execution is out of scope

The user rejected Apple simulator testing on 2026-10-25; simulator execution
stays out of scope and physical tvOS is the active P0 path. The `Apple
Simulator Tests` workflow, its helper scripts, the `ci` sample workspace and
`SimulatorSmokeTests` were removed. Do not reintroduce a simulator gate or cite
simulator execution as evidence.

The Xcode targets are still compiled unsigned by the `Apple Xcode` CI job. That
job cannot reach the LAN Apple TV, and compilation is not execution. The
product `AppleTransport` now **rejects simulator UDIDs** with an explicit
`DEVICE_UNAVAILABLE`; the pure-Rust path cannot drive a simulator and D-19
forbids the host tools that could.

### 3.3 Physical device (pure-Rust product path; tvOS hardware proven)

**Proven on 2026-10-26 (I-150; productized in I-152).** The Apple TV
`RAMA (4)` at `10.102.93.222` (`AppleTV5,3`, tvOS 26.1 build `23J582`, UDID
`a4d6ff71e98403878ba98fe85cb77e8d98d622e5`) ran the whole path without a Mac:
the unsigned GitHub artifact from `Apple Xcode` run `40286509471` at `89d35e3`
was signed locally with the user's free-team profiles and installed through the
pure-Rust RemotePairing -> RSD -> AFC -> InstallationProxy pipeline; the
personalized tvOS Developer Disk Image was mounted; the UI test started through
a direct outer-RSD `testmanagerd`/`dtservicehub` connection.
`AgentRunnerTests.testServe()` (the pre-rename name) passed, Impresario Agent
accepted a real gRPC `Handshake` on `50062` (session
`47123c1c-df6e-44b8-8efd-0043cdb3d001`, capabilities `2,3,5,7,8,9,10,11`) and
completed `Shutdown`. The same install/start/handshake path was then repeated
through the product `AppleTransport` with `impresario device check`.

**2026-10-27 — full stub smoke (P0).** The unsigned GitHub artifact from
`Apple Xcode` run `40460826743` at `d3b60c8` was imported into product set
`p0test-fix`; `impresario apple doctor` passed signing (team `GR2NDRW99B`,
profiles expire `2026-11-02`) and the device/DDI checks. The physical play
`20261027-063121-00d6` passed all **105 steps** in `6m37s` on `RAMA (4)`
(`AppleTV5,3`, tvOS 26.1 build `23J582`, UDID
`a4d6ff71e98403878ba98fe85cb77e8d98d622e5`), covering Home, Basic, Text
(replace/append/clear), Scroll (`scrollto` Row29 and Row0), Selection
(dialog/choice/back), Flags, Keys, Info, LongPress and Media. The media step
passed `Player => Video Playing`; `media-playing.png` shows the local
colour-bar video, poster and missing-piece slot. All 13 screenshots are in
`reports/20261027-063121-00d6/screenshots/rama/` in the run workspace. This
run used the temporary legacy tvOS ids (`com.impresario.agent.AgentHost-tvOS`,
`...AgentUITests-tvOS`, `...AgentUITests-tvOS.xctrunner`,
`...AgentTargetStub-tvOS`); it is the P0 behaviour/visual evidence, not proof
that the short tvOS ids are signed.

The `impresario-apple-device` crate now owns pairing verification, tunnels,
DDI, signing, product classification, AFC + InstallationProxy install and the
direct-RSD XCUITest start. Development provisioning (device registration,
App IDs, RSA CSR/certificate, PKCS#12, profiles) is also native; it authenticates
with `impresario account login` or an App Store Connect Team API key and never
with Python, `openssl` or an app-specific password (I-153). `AppleTransport`
spawns no host tool: `xcodebuild`,
`iproxy`, `simctl`, `devicectl` and `ideviceinstaller` are gone from the Apple
path (D-19). Unit tests cover the signing store, product import, IPA
inspection, DDI state, pairing normalization and the agent-port launch
environment; the physical `device check` above is the integration evidence.
On 2026-10-27 a long-id signing set with only host/runner/nested profiles
signed the product set without a stub (`signing verify` exit 0), while
`--with-stub` failed with the expected missing-profile error. The main
`doctor --plain` probe reported `RAMA (4)` `READY` and listed the missing
optional stub profile as a device detail. These checks still used imported
long-id profiles; live Apple ID login/2FA, live API-key provisioning and
`updateDevice` have not yet run against Apple's live services. The App Store
Connect client and API-key provisioning path are covered by offline tests
(ES256 JWT, platform/profile-type mapping, certificate-limit detection).

**Bundle ids.** iOS already uses the agreed short ids
(`com.impresario.agent`, `com.impresario.agent.tests`,
`com.impresario.stub`). The 2026-10-26/27 tvOS hardware evidence used temporary
legacy explicit-App-ID bundles because the existing free-team tvOS profiles
are tied to them; the P0 evidence build (tag `p0-tvos-evidence`) kept those
ids. The product line (now `master`) uses the short ids for tvOS as well.
Pure-Rust development provisioning now exists (I-153), so fresh short-id
profiles can be created without Xcode/openssl; those profiles and the short-id
bundles have **not** yet run on the physical Apple TV. The old profiles expire
`2026-11-02`. Use separate signing-set names for iOS and tvOS if the profiles
are platform-specific; both can map the same bundle ids.

Machine-global setup (the project holds no pairing secrets or certificates):

```bash
# One global Apple ID session (encrypted; supports device or SMS 2FA):
impresario account login
impresario account status --check

impresario apple status
impresario apple pair --host <apple-tv-host>
# or: impresario apple import-pair <pair.plist> --save <udid>

# Pure-Rust development provisioning (host + runner + nested; no stub):
impresario apple signing provision --name default --platform tvos
# Add the fixture stub profile only when stub plays are wanted:
impresario apple signing provision --name default --platform tvos --with-stub

# Unattended alternative: an App Store Connect Team API key (no 2FA at run time).
# Create the Team Key with the Admin role once in the browser, keep the .p8 out
# of the repository, and pass the issuer id, key id and your team id:
impresario apple signing provision --name default --platform tvos `
  --team GR2NDRW99B `
  --api-key "$env:USERPROFILE\.impresario\apple\AuthKey_<KEYID>.p8" `
  --issuer-id "<issuer-id>" --key-id "<key-id>"
# The same values can come from IMPRESARIO_APPLE_API_KEY,
# IMPRESARIO_APPLE_API_ISSUER_ID, IMPRESARIO_APPLE_API_KEY_ID and
# IMPRESARIO_APPLE_TEAM_ID. App-specific passwords do not authenticate the
# developer portal and are not supported.

impresario apple products import ./build-tvos/Build/Products/Debug-appletvos --name current
impresario apple ddi import ./ddi-source --build <build>
impresario apple doctor --host <apple-tv-host> --signing default --products current
```

Provisioning registers the device under the name **`impresario`** (not the
computer name, Apple TV name or `pyatv`) and renames an existing developer
portal record to match. `--device-name` overrides this deliberately.

The older import path is still supported: `identity.p12` at the root or beside
any profile, and one directory per bundle containing
`profile.mobileprovision` plus `entitlements.plist`, then
`impresario apple signing import ./signing-source --name default`. The PKCS#12
password is prompted for and stored in the OS credential store;
`IMPRESARIO_APPLE_P12_PASSWORD` is for headless runs only.
`impresario apple signing verify` signs a product set offline.
See `docs/SIGNING.md` and `agents/apple/README.md` for the full commands.

Building the payload on a Mac (optional; CI produces the same unsigned apps):

```bash
brew install xcodegen
cd agents/apple
xcodegen generate                                  # ImpresarioAgent.xcodeproj

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

Use the `-iOS` schemes for iPhone/iPad. Physical builds need a development
team (set it in Xcode or a local xcconfig; the repo pins none). A free Apple ID
personal team works but its profile expires after seven days. Sign in
interactively, or use an App Store Connect Team API key for unattended
provisioning; app-specific passwords do not authenticate the developer portal.
Impresario never stores or passes a plaintext Apple ID password, and it never
writes the API private key into a signing set. The unsigned GitHub artifact
cannot be installed directly.

The attached Apple TV is `AppleTV5,3` (Apple TV HD), name `RAMA (4)`, AirPlay
device ID `B6:FD:1D:0C:99:3C`, build `23J582`; its LAN address is dynamic (the
2026-10-27 P0 run used `192.168.93.19`; an earlier run used
`10.102.93.222`). Identify it by UDID in committed setup; do not pin the
address. Developer Mode is enabled and the device trusted the host.
The first start asks for local-network access; tvOS has no XCTest `tap`, so
grant that one prompt with the remote. The permission is remembered after that.

Master side:

```yaml
devices:
  - name: rama
    platform: tvos
    connection:
      transport: network
      udid: <udid>
      endpoint: "<apple-tv-host>:50062"

apps:
  - name: stub
    platforms:
      tvos:
        id: com.impresario.stub
    options:
      clear_data: true

  - name: actual
    platforms:
      tvos:
        id: com.example.release
        package: /abs/path/Release-tvos/Example.app
    options:
      clear_data: true
```

```bash
impresario device check rama --plain
impresario doctor
impresario run --play plays/login.play --device rama --app stub
impresario run --play plays/login.play --device rama --app actual
```

The stub entry deliberately has **no `package`**: the transport signs and
installs the product stub only when the selected app id is the product stub and
no package is supplied. A normal/release app (`actual` above) installs its
package and never requires or installs the stub profile. Supplying a package
for the stub id also wins over the product stub, so an explicitly packaged
build is never silently replaced.

`impresario doctor` probes every configured `ios`/`tvos` device by its
configured name: pairing, RSD/UDID, DDI state/assets, signing identity and the
host/runner/nested profiles. It never mounts a DDI or installs anything. Use
`impresario apple doctor --host <host>` for the Apple-specific report.

Endpoint resolution: `connection.udid` is required. The host comes from
`connection.host`, else the host part of `connection.endpoint`; the port comes
from `options.agent_port`, else the endpoint port, else `50062`. The transport
passes that port to the UI test as `IMPRESARIO_AGENT_PORT`, so both sides always
agree. The pure-Rust path reaches devices over the network (RemotePairing on
`_remotepairing._tcp:49152`); a USB-only iPhone is not supported yet. iOS/tvOS
**simulators are rejected** because the Rust path cannot drive them and D-19
forbids the host tools that could. Physical iOS over WLAN would use the same
path but is not hardware-verified.

Set a non-default signing/product/DDI with:

| Variable | Default |
| --- | --- |
| `IMPRESARIO_APPLE_SIGNING` | `default` |
| `IMPRESARIO_APPLE_PRODUCTS` | `current` |
| `IMPRESARIO_APPLE_DDI` | detected OS build |
| `IMPRESARIO_APPLE_START_TIMEOUT_SECS` | `120` |

Clean state per test is a pure-Rust uninstall + reinstall over the live
RemotePairing control tunnel (`clear_data` + the app platform's signed
`package`/`id`, I-84). It needs no `simctl`, `devicectl` or
`ideviceinstaller`. If the package is not configured, the agent cold-starts the
app and logs that the clean reinstall was unavailable.

Session cleanup uninstalls the XCUITest runner and, when it was installed, the
fixture stub; the agent host stays installed. The runner is visible in the
Apple TV drawer while a play runs. tvOS has no supported API for hiding an
installed app, so Impresario removes transient apps at teardown rather than
claiming a hidden runner. An actual app installed from its package is left in
place.

First physical run after an install: iOS/tvOS ask for local-network permission.
The host app and test bundle declare the usage description and Bonjour service.
On iOS the test bundle tries an interruption monitor + SpringBoard poll; on
tvOS those calls are compiled out and one human press on the remote grants it
for good (I-88). Uninstall + reinstall does **not** reset that permission.

Apple TV specifics: no USB path, so WLAN is the only way; Impresario Agent
presses remote buttons through `XCUIRemote` (D-pad/Select/Menu/PlayPause;
`home` = `XCUIDevice.press(.home)`), and the stub app's focused row is visible
in its `stub_key_log`/`stub_status` labels while a play runs.

### Playback probes and the Apple hook

The Xcode layer is **not compiled on this host** (no Xcode); the macOS GitHub
Actions job compiles both the tvOS and iOS targets unsigned (run
`40460826743` at `d3b60c8` built the P0 artifact). The physical 105-step run
`20261027-063121-00d6` executed the full `XCUIDriver` action matrix (focus,
click/Select, key, scroll, text/keyboard, long-press, media checks, back) and
the media `Player => Video Playing` probe on the Apple TV. `ProbeCollector`,
`HookReceiver` and the deeper decoder/DRM/position assertions were not
independently exercised by that play. In this host's WSL only `swift test`
runs; `XCUIDriver`/`HookReceiver` depend on XCTest and need a real device to
execute.

XCUITest cannot read another app's `AVPlayer`, decoder or FairPlay session.
The stub `MediaViewController` therefore opens a local H.264/AAC video, a
decoded PNG, an empty image slot and Apple's public BipBop HLS stream; the app
links `ImpresarioHook` and reports player/showing state every 250 ms to
`http://127.0.0.1:47831/hook`, where `HookReceiver` keeps the newest JSON
snapshot. Without that listener `start()` is a no-op and the probe correctly
stays UNVERIFIABLE. Run the media scenario from the stub home screen and
exercise `Showing`, `Player` (playing/paused/position/resolution) and
DRM-disabled; record whether the local-network permission allowed loopback.
The stub media screen and the UI `Player => Video Playing` check are
hardware-verified by `20261027-063121-00d6`; the hook/decoder/DRM/position
probes remain HD-1 work.

---

## 4. Hardware acceptance backlog scheduled with the Phase 6/7 window

Phase 6 closes on fixture/replay evidence. The TV runbooks are §1.3/§1.4
above; the remaining device/release checks below stay in the same deferred
window (and are tracked as HD-5, HD-6 and HD-8 in `docs/STATUS.md`). None of
them is a prerequisite for the Phase 6 code gates.

### 4.1 Android USB data (HD-5)

The native USB implementation exists but this host only had TCP reachability.

1. On Windows, bind the phone/TV ADB interface to WinUSB (the device must
   expose an ADB USB interface; a charge-only cable is not enough).
2. Define the device with `connection.transport: usb` and `serial: <usb
   serial>` (no host/port).
3. Run `impresario device check`, then the full phone/TV smoke play with the
   USB device and record `doctor --json`.
4. Launch the stub media scenario and run playback probes:
   `Check: Showing => stub_media_video with coverage>=50%, decoded`,
   `Check: Player => stub_media_video Playing`,
   `Check: Player => stub_media_video DRM-disabled`, plus the paused/position
   assertions. With the `:impresario-hook` SDK linked, verify the
   `IMPRESARIO_HOOK` logcat line is visible to the agent and that removing the
   hook produces `unverifiable`, never a guessed PASS. Record whether
   `dumpsys media.drm` exists on this vendor image.
5. Verify the USB-specific behaviours: no host logcat while the single USB
   session is owned by the agent, and an explicit refusal of the forwarding
   fallback while instrumentation is running.
6. Record the USB serial, driver, run id, probe report and any
   `DEVICE_UNAVAILABLE` output.

### 4.2 Managed Chromium outside Windows x64 (HD-6)

1. On each of Linux x64, Linux arm64, macOS arm64 and macOS x64, install from
   the pinned archive (consent/`--yes`) and verify its SHA-256.
2. Run `doctor --json` and a headless web smoke play without system Chrome on
   `PATH`; record whether sandbox/AppContainer setup is needed per OS.
3. Run the media fixture play against `fixtures/web/media.html` and the hook
   fixture in `fixtures/web/hook.html`: `Showing` with coverage/decoded/source,
   `Player` playing/position/resolution/muted, DRM-disabled and the intentional
   missing piece. Record the report evidence and confirm a missing piece is a
   FAIL, not UNVERIFIABLE.
4. Confirm `tools remove chromium` leaves no managed payload and that a run
   never downloads without consent.

### 4.3 Post-Phase-7 device matrix regression (HD-8)

After the Apple/Phase 7 hardware work, repeat the detached four-device matrix
(web, Android phone, Android TV, TV connector) with the then-current binaries;
attach after detach, read `report.json`, and record the run id, duration and
step counts. Include one media act per platform (stub video/image or the web
fixture) covering `Showing`, `Player`, DRM-disabled and `--unverifiable warn`;
record the evidence sources and confirm no third-party player claims PASS
without an in-app hook. This is a regression check, not a replacement for the
recorded Phase 4 acceptance run `20261021-080459-ec51`.
