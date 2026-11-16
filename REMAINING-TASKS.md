# Remaining tasks

Audit date: 2026-11-16. Sources: `TASK.md`, `TASK-1.md`, `TASK-2.md`,
`TASK-3.md`, `TASK-INSPECTOR.md`, `INSPECTOR-NOTES.md`, `docs/STATUS.md`.

This file tracks work that is **not** complete. The current session's iPhone/tvOS
inspector, Android phone/TV, iPhone and tvOS stub suites, pairing, runner icon
and DDI validation are done and recorded in `docs/STATUS.md`.

Status legend: `todo`, `in progress`, `blocked`, `deferred` (needs hardware,
credentials or an owner decision), `done`.

Device inventory available for this pass:

| Name | Platform | Address | State |
|---|---|---|---|
| `android-phone` | Android phone (SM-A515F, Android 13) | `192.168.93.76` | ready; 10-act suite complete 2026-11-16 |
| `android-tv` | Android TV (G03_4K_GB, Android 11) | `192.168.93.84` | ready; 10-act suite complete 2026-11-16 |
| `iphone` | iPhone13,2, iOS 18.7.8 (22H352) | `192.168.93.14` | ready over Wi-Fi; 10-act suite complete 2026-11-16 |
| `appletv` | AppleTV5,3, tvOS 26.1 (23J582) | `192.168.93.19` | ready over Wi-Fi; inspector and 10-act suite complete 2026-11-16 |
| `android-flutter` | Android 15 (ALI-NX1) with `com.dooticketseg.app.staging` | `192.168.93.112` | blocked; screen off, agent installation times out |

## 1. Element Inspector (TASK-INSPECTOR)

| ID | Remaining item | Source | Device-feasible now | Status |
|---|---|---|---|---|
| INS-R1 | Flutter acquisition: Dart package, `ensureSemantics`, `ImpresarioTag`, semantics extension, host adapter, widget-inspector enrichment, visual fallback | TASK-INSPECTOR Phase 6 / STATUS component row | Yes: real staging app (`com.dooticketseg.app.staging`) on `android-flutter`; no Flutter SDK for the Dart package | in progress — Layer A semantics validated; bridge/diagnosis/role work remains |
| INS-R2 | Shared inspector sessions, leases and scheduler preemption; currently `inspector.run_conflict` is advisory and a concurrent run is not coordinated | INS-7.1 / STATUS | Code; device regression possible | todo |
| INS-R3 | In-page push binding (`Runtime.addBinding` / `MutationObserver`) instead of polling at `inspector.fps` | INS-2.2 / STATUS | Code; web fixtures only | todo |
| INS-R4 | Runner execution of canonical `LOCATOR` cells; the connector run path still rejects them | INS-1.2 deviation 7 / STATUS | Code + device regression | todo |
| INS-R5 | MCP inspector view resource/subscription (`notifications/resources/updated`) | INS-3.5 | Code | todo |
| INS-R6 | Post-commit locator verification and scene file watching; stale revisions are refused but nothing watches files | INS-3.3/3.4 | Code | todo |
| INS-R7 | Same-origin iframe traversal, Android WebView bridge and Apple WKWebView bridge | INS-2.1 / TASK-INSPECTOR 5.5 | Code + device fixtures | todo |
| INS-R8 | Android helper/Compose specifics: helper APK packaging, `testTag`, TV focus model and remote D-pad in the inspector UI | TASK-INSPECTOR Phase 4 | Partly (Android phone/TV) | todo |
| INS-R9 | Android inspector hardware acceptance (snapshot, screenshot, input, detach/reattach) on phone and TV | STATUS component row | Yes | done 2026-11-15 (phone and TV) |
| INS-R10 | tvOS inspector hardware acceptance (snapshot, screenshot, focus, input) | TASK-INSPECTOR Phase 5 | Yes (Apple TV) | done 2026-11-16 — snapshot, screenshot, focus/D-pad, scroll, Select and detach/reattach passed 2026-11-15; type and `keyboard hide` passed 2026-11-16 |
| INS-R11 | INS-0.3 Appium Inspector baseline and INS-7.3 full benchmark; only the mock core benchmark exists | INS-0.3 / INS-7.3 | Appium baseline no; device timings yes | deferred |
| INS-R12 | Anti-breakage gates: `buf breaking`, run-path latency gate, old/new handshake matrix, LSP/TS locator-cell completion and hover | INS-7.4 | CI/code | todo |
| INS-R13 | Owner review of `CLI.md`, `WEB.md`, `MCP.md` and `INSPECTOR-NOTES.md` | Definition-of-done table | Owner | deferred |

## 2. TASK-2 partials

| ID | Remaining item | Source | Device-feasible now | Status |
|---|---|---|---|---|
| T2-R1 | TUI editor/home: dedicated add/edit/remove forms for devices, apps and config (Enter opens `$EDITOR` today; Groups is the exception) | STATUS gap 19 | Code | todo |
| T2-R2 | TV setup wizard steps for Tizen profiles and webOS Developer Mode passphrase (currently explicit CLI work) | STATUS gap 20 | Code | todo |
| T2-R3 | `app check` installed state, code signature and re-signing (only id/package existence today) | STATUS gap 9 / TASK-2 Phase 1B | Code + devices | todo |
| T2-R4 | Clean-VM "one archive, no adb/Xcode/ares/sdb/tizen on PATH" acceptance | TASK-2 Definition of done 1 | No (needs clean VM) | deferred |

## 3. TASK-3 probes and hardware acceptance

| ID | Remaining item | Source | Device-feasible now | Status |
|---|---|---|---|---|
| T3-R1 | Android hardware probe path: `Showing` coverage/decoded and `Player` position/duration/muted on the stub Media screen | TASK-3 Android collector / HD-5 | Yes (Android phone; TV after authorization) | todo |
| T3-R2 | Paused playback evidence on Android and Apple (stub currently exposes only `playing`; no pause control in the fixture UI) | HD-1 / STATUS | Code + devices | todo |
| T3-R3 | One-time iOS local-network prompt | HD-1 | Yes, but needs a permission reset and an on-phone tap | deferred |
| T3-R4 | Fresh personalized tvOS DDI mount (all previous runs found it already mounted) | HD-1 | Yes (Apple TV restart + real DDI) | todo |
| T3-R5 | FairPlay/DRM test asset and certificate | HD-1 / TASK-3 DRM | No (credentials/content) | deferred |
| T3-R6 | Netflix-class streaming journey with a user account | TASK-3 Definition of done 3 | No (account/content) | deferred |
| T3-R7 | HD-8: one four-device matrix with native transports and media probes | HD-8 | All devices ready, but Apple product sets are one-platform-per-process; runs must split Apple platforms | blocked (split runs possible) |
| T3-R8 | HD-5: Android USB data path on hardware | HD-5 | No USB ADB interface exposed | deferred |
| T3-R9 | HD-6: managed Chromium on Linux/macOS | HD-6 | No (Windows host only) | deferred |
| T3-R10 | HD-2: paid-team App Store Connect API-key provisioning | HD-2 | No (paid key) | deferred |

## 4. Apple operational tasks

| ID | Remaining item | Source | Device-feasible now | Status |
|---|---|---|---|---|
| A-R1 | Re-provision the tvOS `default` and iOS `ios-523d45d` profiles before 2026-11-22 | docs/STATUS.md | Yes (account) | todo |
| A-R2 | Import the working personalized tvOS DDI (build 27A9269) into the `23J582` store so future runs do not need `IMPRESARIO_APPLE_DDI` | docs/DEVICES.md / I-202 | Yes (local files) | todo |
| A-R3 | tvOS inspector acceptance (also INS-R10) | TASK-INSPECTOR Phase 5 | Yes | done 2026-11-16 |

## 5. Flutter device findings (2026-11-16)

The Android 15 `ALI-NX1` at `192.168.93.112` runs a release Flutter build
(`com.dooticketseg.app.staging`, version `0.1.0`, no debuggable flag and no Dart
VM service). Impresario reaches it through its own native ADB on port `5555`;
the phone was paired with `impresario device pair`.

What worked with the existing Layer A path:

| Check | Result |
|---|---|
| Snapshot | Home screen: 29 nodes, including `New Events`, `Upcoming Events`, `Dining`, bottom nav `Home`/`Search`/`Tickets`/`Reservations`/`Profile` |
| Screenshot | `1200x2652` PNG, 100,594 bytes (home), 174,348 bytes (login with typed username) |
| Tap | `Login` on the welcome screen navigated to the login form; tapping the username field focused it |
| Type | `flutter-user` landed in the username textbox; `/api/node` reported `value: "flutter-user"`, `role: text_box`, `focused: true` |
| Scroll | The home content exposed `scroll: more below (3)` and horizontal carousels (`more right`) |
| Detach/reattach | Works; the form and typed value survived the reattach |

Observed gaps for INS-R1:

- Roles are only as good as the app's `Semantics`: bottom-nav labels are
  `generic`, not buttons; the app's image nodes have no labels. Flutter buttons
  that set `Semantics(button: true)` (Login/Signup/Explore) correctly arrive as
  `button`.
- No Flutter detection or `FLUTTER_SEMANTICS_OFF` diagnosis is produced; the app
  is treated as an ordinary Android app.
- No `testid` for untagged widgets (the app does not ship `ImpresarioTag` or
  `Semantics(identifier:)`), and the release build exposes no VM service for
  widget-inspector enrichment.
- On this Honor device the agent is **force-stopped by an OEM power manager**
  roughly every 60-90 seconds (`am_kill ... stop com.impresario.agent due to
  from pid ... by app`), so inspector commands must run in short bursts or the
  agent must be exempted in the phone's App launch management.
- Android's `uiautomator dump` is killed (`exit 137`) on this Flutter tree, so
  raw-attribute dumps through the platform tool are unavailable; Impresario's
  own capture still succeeds.

Requested acceptance performed: tapped `Login` from the welcome screen, typed
`flutter-user` into the username field, and tapped the `Login` submit button
(no navigation/error surfaced with an empty password and unchecked terms).
On 2026-11-16 the device was asleep and `device check` timed out after 300 s
installing the agent APK, so the keyboard-hide acceptance (login, type,
`POST /api/input {"kind":"keyboard"}`, submit) remains blocked pending a
physical wake/unlock and an OEM power-manager exemption. No success is claimed.

## 6. Session log

- 2026-11-15: `android-phone` ready (A51, Android 13); stale debug-signed agent/test/stub packages were uninstalled and the CI `d90344f` APKs installed.
- 2026-11-15: `android-tv` authorized on the TV; `device check` passes (`G03_4K_GB`, Android 11, 1920x1080 @2x).
- 2026-11-15: `iphone` and `appletv` pass `device check` over Wi-Fi with `ios-523d45d` / `tvos-523d45d`.
- 2026-11-15: Android phone inspector acceptance: 18 nodes at attach; 1080x2400 screenshot (95,148 bytes); tap `Text entry`, type `android-wifi-check` (`echo:android-wifi-check`), back Home; scroll moved rows 0-12 to 17-29 only when the scrollable node ref was supplied; detach/reattach preserved the form at `rev: 13`.
- 2026-11-15: Android TV inspector acceptance: 1920x1080 screenshot; tap `Scroller`, scroll right moved rows 0-6 to 13-20; D-pad `down` moved focus `Alpha` -> `Bravo` and logged `key=DPAD_DOWN`; `Select` logged `clicked=Alpha`; detach/reattach passed.
- 2026-11-15: Apple TV inspector: snapshot 30 nodes at 1920x1080; screenshot 72,022 bytes; D-pad focus and `Select` navigation passed; `scroll down` moved the list rows 0-8 to 4-15; detach/reattach passed. `type` failed with `NOT_INTERACTABLE` because `XCUIDriver.inspectorType` taps the field; the run path already uses focus + Select for tvOS (I-198), so the inspector needs the same branch.
- 2026-11-16: `android-flutter` (`ALI-NX1`, Android 15, 1200x2652 @3.25x) added at `192.168.93.112`; paired with `impresario device pair` and now reached through Impresario's own native ADB. The wireless-debugging port was switched to `5555` once with platform-tools `adb tcpip 5555` because the 1.0.0 device schema hardcodes ADB 5555; all inspection after that used Impresario only.
- 2026-11-16: Flutter staging app validated through the inspector: tapped `Login`, typed `flutter-user` into the username textbox (`value: "flutter-user"`, `role: text_box`), tapped `Login` submit; home and login screenshots captured. Layer A semantics work; Flutter detection, role enrichment, identifiers and VM-service enrichment remain (section 5).
- 2026-11-16: CI `43824162511` at `98a978f` green. The Android phone suite improved to 9/10 (`run 20261116-073401-f967`, 107 steps, 1m57s) after I-205, but the Text act's final `back` only dismissed the IME; the TV suite (7/10, `run 20261116-073537-b24c`, 111 steps) had the same Text failure, a cascading Scroll failure and a Media status node below the 1080p screen.
- 2026-11-16: I-206 (Android `back` dismisses an open IME first) and I-207 (compact 1080p TV media profile) landed; CI `43825869219` at `9ec8196` passed all eight jobs, including Android JVM tests and APKs.
- 2026-11-16: `device check --refresh` uninstalled and reinstalled `com.impresario.agent`, `com.impresario.agent.test` and `com.impresario.stub` from the `9ec8196` APKs over native ADB on both `android-phone` and `android-tv`; both checks passed.
- 2026-11-16: Android 10-act suites passed with `9ec8196`: phone `run 20261116-075901-c09c` (10/10 acts, 107/107 steps, 95,020 ms, 12 screenshots) and TV `run 20261116-075630-6923` (10/10 acts, 111/111 steps, 94,042 ms, 12 screenshots). The TV workspace uses `scroll_max_attempts: 30` per I-70; a Scroll-only rerun passed as `run 20261116-075550-68a3` (12/12 steps, 29.6 s).
- 2026-11-16: Android `keyboard hide` validated through the inspector with `98a978f`: phone typing `kb-check` left `echo:kb-check` and a 127,567-byte IME screenshot; `POST /api/input {"kind":"keyboard"}` returned `ok`, the screenshot dropped to 53,466 bytes with the echo intact, and `back` reached Home. TV `tv-kb-check` went 63,572 -> 28,258 bytes and `back` reached Home.
- 2026-11-16: tvOS inspector typing and `keyboard hide` passed on `192.168.93.19` (`tvos-1e08df4`, `default`, real DDI): typing `tvos keyboard hide` opened the keyboard (72 nodes), `keyboard` returned 15 nodes with the textbox focused and `echo:tvos keyboard hide`, and the screenshot fell from 1,052,382 to 59,372 bytes. INS-R10 and A-R3 are complete.
- 2026-11-16: tvOS 10-act suite passed `run 20261116-064843-e86f` (10/10 acts, 111/111 steps, 217,438 ms), including the Text back-from-keyboard check, Keys, Info and Media. The iPhone suite passed `run 20261116-073058-4eeb` with `ios-1e08df4`/`ios-523d45d` (10/10 acts, 107/107 steps, 203,993 ms); a first prepare attempt failed with an XCUITest `phase=prepare` transport error and the immediate retry passed without a reboot.
- 2026-11-16: `android-flutter` re-check failed: `device check` timed out after 300 s installing the agent APK and the screen remained off. The Flutter keyboard-hide acceptance stays blocked until a physical wake/unlock and an OEM power-manager exemption.
