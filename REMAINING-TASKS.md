# Remaining tasks

Audit date: 2026-11-16. Sources: `TASK.md`, `TASK-1.md`, `TASK-2.md`,
`TASK-3.md`, `TASK-INSPECTOR.md`, `INSPECTOR-NOTES.md`, `docs/STATUS.md`.

This file tracks work that is **not** complete. The current session's iPhone/tvOS
inspector, Android phone/TV, iPhone and tvOS stub suites, pairing, runner icon
and DDI validation are done and recorded in `docs/STATUS.md`. The standalone
local agent stack in `ai/` is complete offline; its device evidence is deferred
(section 6).

Status legend: `todo`, `in progress`, `blocked`, `deferred` (needs hardware,
credentials or an owner decision), `done`.

Device inventory available for this pass:

| Name | Platform | Address | State |
|---|---|---|---|
| `android-phone` | Android phone (SM-A515F, Android 13) | `192.168.93.76` | ready; 10-act suite complete 2026-11-16 |
| `android-tv` | Android TV (G03_4K_GB, Android 11) | `192.168.93.84` | ready; 10-act suite complete 2026-11-16 |
| `iphone` | iPhone13,2, iOS 18.7.8 (22H352) | `192.168.93.14` | ready over Wi-Fi; 10-act suite complete 2026-11-16 |
| `appletv` | AppleTV5,3, tvOS 26.1 (23J582) | `192.168.93.19` | ready over Wi-Fi; inspector and 10-act suite complete 2026-11-16 |
| `android-flutter` | Android 15 (ALI-NX1) with `com.dooticketseg.app.staging` | `192.168.93.112` | ready; `device check --refresh` passed; inspector keyboard-hide acceptance complete 2026-11-16 |

## 1. Element Inspector (TASK-INSPECTOR)

| ID | Remaining item | Source | Device-feasible now | Status |
|---|---|---|---|---|
| INS-R1 | Flutter acquisition: Dart package, `ensureSemantics`, `ImpresarioTag`, semantics extension, host adapter, widget-inspector enrichment, visual fallback | TASK-INSPECTOR Phase 6 / STATUS component row | Yes: real staging app (`com.dooticketseg.app.staging`) on `android-flutter`; no Flutter SDK for the Dart package | in progress — Layer A semantics and keyboard-hide validated; detection/role/VM-service work remains |
| INS-R2 | Shared inspector sessions, leases and scheduler preemption; `inspector.run_conflict` was advisory | INS-7.1 / STATUS | Code; device regression possible | done 2026-11-24 for the file-lease layer (`633c780` + `c6ae5b0`): the inspector holds `jobs/leases/inspector.<device>.json` while runs keep `<device>.json`; `readonly` (default) keeps inspection live and rejects input with `LEASE_READONLY` (HTTP 409), `exclusive` detaches the inspector on its next tick, and `block_run` makes `lease::available` false and refuses `lease::acquire` (exit 3). Mode/owner appears in `/api/status`, `/healthz`, `/api/devices`, `inspector status` and the SPA banner. CI run `45338868848` at `c6ae5b0` passed all eight jobs. Shared device sessions and a resident scheduler remain out of scope. |
| INS-R3 | In-page push binding (`Runtime.addBinding` / `MutationObserver`) instead of polling at `inspector.fps` | INS-2.2 / STATUS | Code; web fixtures only | done 2026-11-24 (`371f117`; scoped to the inspector session in `ce00739`): the injected ES5 script installs a `MutationObserver` plus scroll/resize/focus/transition/popstate/hashchange/visibility/fullscreen listeners and patched `history.pushState`/`replaceState`; each calls a `Runtime.addBinding` callback, the connector keeps a monotonic counter and the master long-polls `Inspector.Watch` (5 s idle heartbeat), so an idle page costs no DOM traversal. Binding install is opt-in (`inspector_push=true`) so a runner sharing the page never clobbers it; engines without the binding fall back to `inspector.fps` polling. CI run `45336056795` at `ce00739` passed all eight jobs, including `push_watch_reports_page_changes` against real Chromium and `inspector_push_is_opt_in` |
| INS-R4 | Runner execution of canonical `LOCATOR` cells (native Android/Apple agents still reject them) | INS-1.2 deviation 7 / STATUS | Code + device regression | partial — web/webOS/Tizen done 2026-11-23 (`cabd15d`): the connector resolves locators against the inspector snapshot with the shared `impresario-locator` matcher and rewrites to a unique CSS handle; `locators.html` fixtures cover role/name, id, testid, shadow DOM, ambiguity, `index`, `Count`, `Hidden`, ordinary checks and probes on raw (CI-verified) with a feature-gated chromiumoxide entry. Native Android/Apple agents still reject `LOCATOR` (no snapshot-backed native resolver), so native execution remains open |
| INS-R5 | MCP inspector view resource/subscription (`notifications/resources/updated`) | INS-3.5 | Code | done 2026-11-23 (`7d5fba5`; watcher handoff fix `d8d27e3`; CI run `45229626731` passed all eight jobs): an attached inspector view is listed as `impresario://inspector/{device}/view` and readable with `resources/read`; `resources/subscribe` starts one per-URI watcher that long-polls `/api/wait` and emits `notifications/resources/updated`; `resources/unsubscribe` is idempotent; a racing resubscribe keeps exactly one watcher; static docs and other devices are refused |
| INS-R6 | Post-commit locator verification and scene file watching; stale revisions are refused but nothing watches files | INS-3.3/3.4 | Code | done 2026-11-23: commit re-resolves each written locator cell against the live snapshot (`verified`/`warning`/`invalid`/`unavailable`) and returns it in `verification`; open scenes are polled by SHA-256 and external changes arrive as SSE `update` events with `kind: "scene"` (idle scene auto-reloads, pending preview warns) |
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
| T3-R1 | Android hardware probe path: `Showing` coverage/decoded and `Player` position/duration/muted on the stub Media screen | TASK-3 Android collector / HD-5 | Yes (Android phone; TV after authorization) | done 2026-11-16 — phone `run 20261116-102116-0599` (video coverage 98.94%, decoded, position 3.38 s, duration 4.021 s, muted) and TV `run 20261116-102211-2f1f` (coverage 99.74%, position 1.637 s, same duration/muted), 8/8 steps each |
| T3-R2 | Paused playback evidence on Android and Apple (stub exposed only `playing`; no pause control in the fixture UI) | HD-1 / STATUS | Code + devices | done 2026-11-16 — Android phone `run 20261116-110113-0a92` (paused 1.714 s) and TV `run 20261116-110200-b506` (3.709 s); tvOS `run 20261116-115221-d1a6` (1.664 s) and iPhone `run 20261116-115340-3b80` (3.927 s). Each 14/14 steps with `rate=0` and identical positions 2 s apart, then resumed at `rate=1`; the Apple stub gained a `Pause` button |
| T3-R3 | One-time iOS local-network prompt | HD-1 | Yes, but needs a permission reset and an on-phone tap | deferred |
| T3-R4 | Fresh personalized tvOS DDI mount (all previous runs found it already mounted) | HD-1 | Yes (Apple TV restart + real DDI) | done 2026-11-16 — imported DDI into `23J582`, restarted the TV, and `run 20261116-095124-369b` mounted it without `IMPRESARIO_APPLE_DDI` |
| T3-R5 | FairPlay/DRM test asset and certificate | HD-1 / TASK-3 DRM | No (credentials/content) | deferred |
| T3-R6 | Netflix-class streaming journey with a user account | TASK-3 Definition of done 3 | No (account/content) | deferred |
| T3-R7 | HD-8: one four-device matrix with native transports and media probes | HD-8 | All devices ready, but Apple product sets are one-platform-per-process; runs must split Apple platforms | blocked (split runs possible) |
| T3-R8 | HD-5: Android USB data path on hardware | HD-5 | No USB ADB interface exposed | deferred |
| T3-R9 | HD-6: managed Chromium on Linux/macOS | HD-6 | No (Windows host only) | deferred |
| T3-R10 | HD-2: paid-team App Store Connect API-key provisioning | HD-2 | No (paid key) | deferred |

## 4. Apple operational tasks

| ID | Remaining item | Source | Device-feasible now | Status |
|---|---|---|---|---|
| A-R1 | Re-provision the tvOS `default` and iOS `ios-523d45d` profiles before 2026-11-22 | docs/STATUS.md | Yes (account) | done 2026-11-16 — both sets refreshed with `--with-stub`, expiry now 2026-11-23; iPhone Home rerun passed |
| A-R2 | Import the working personalized tvOS DDI (build 27A9269) into the `23J582` store so future runs do not need `IMPRESARIO_APPLE_DDI` | docs/DEVICES.md / I-202 | Yes (local files) | done 2026-11-16 — `apple ddi import` completed `23J582`; tvOS doctor and suite ran without the env override |
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
`flutter-kb` into the username field, and invoked `keyboard hide` through the
inspector. The IME screenshot fell from 173,401 to 120,161 bytes, the value stayed
`flutter-kb`, and the next snapshot still exposed the Login submit button. A second
round typed the password, checked the terms box, hid the keyboard again (121,363
bytes), tapped Login (ref 27) and reached the validation state (`Invalid format`,
144,390 bytes), proving submit was reachable after the hide. On 2026-11-16 the
device was refreshed first with `device check --refresh android-flutter` after an
initial check failed with `INSTALL_FAILED_UPDATE_INCOMPATIBLE`; the acceptance then
ran on `9ec8196` APKs. No success is claimed for Flutter detection/enrichment,
which remains INS-R1 work.

## 6. Standalone local agent stack (`ai/`)

A self-contained CPU-only agent operates the full 41-tool Impresario MCP from
`D:\projects\impresario\ai`. The folder is ignored by git (`/ai/`); only the
ignore rule is tracked. It bundles MiniCPM5-2B Q8_0, llama.cpp b12438, Python
3.13 and `impresario.exe` 1.0.0, sets `IMPRESARIO_HOME=ai/home`, and never calls
a system `adb`.

| ID | Item | Status |
|---|---|---|
| AI-R1 | Bundle model, llama.cpp, Python, MCP host and inspector scripts; relative paths and contained home | done 2026-11-16 — `ai.cmd doctor` PASS |
| AI-R2 | Five toolsets partition the live 41 tools exactly (authoring 12, automation 9, inspector-read 8, inspector-act 6, scenes 6) | done 2026-11-16 — doctor reports missing=[] extra=[] |
| AI-R3 | Compact catalog plus one validated `call_tool` dispatcher instead of full schemas | done — 128-token dispatcher; real MCP schemas enforced locally |
| AI-R4 | CPU profile: 131k context, q8_0 KV, 8 threads, `-fa on`, thinking off | done — rendered authoring prompt about 4,100 tokens; 5.08 GiB resident at full window |
| AI-R5 | Cache-aware prompt: constant tool schema, changing catalog at the tail | done — a toolset switch processed 683–2,231 tokens, not the full prompt |
| AI-R6 | Honest evidence gate | done — 18/18 cases; rejects overclaims, `not-run` with successful calls, wrong device/run/change/keyboard claims; normalizes tool-name misspellings |
| AI-R7 | One-command scripts: doctor, start/stop, chat, inspector start/stop, refresh, validate, fmt, status | done — all exercised; validate reports 9 scenes, 3 theatres, 2 plays, 5 devices, 3 apps and one expected `IMP-DSL-054` |
| AI-R8 | Offline toolset exercise | done — authoring list/validate; automation service/schedules/watchers; inspector-read, inspector-act and scenes report blocked honestly with no inspector |
| AI-R9 | Local inspector lifecycle | done — start, status and stop; no device attached, so no device evidence was claimed |
| AI-R10 | Apple CI after the acceptance merge | done — run `43871350553` green at `4b69a29` |

Device evidence is deferred to the device network:

| ID | Item | Status |
|---|---|---|
| AI-D1 | Android phone/TV `devices_check` and 10-act stub runs | deferred |
| AI-D2 | iPhone/tvOS separate runs and Apple device regression | deferred |
| AI-D3 | Wi-Fi acceptance on all four targets | deferred |
| AI-D4 | `.112` Flutter inspector acceptance (never the 10-act suite) | deferred |
| AI-D5 | Keyboard-hide scene run through `impresario_run` (MCP has no keyboard-hide input) | deferred |
| AI-D6 | `impresario_devices_check` / `inspector_attach` evidence from this host | deferred — the current network has no devices |

Local limits: a 2B model sometimes needs one or two ledger corrections; the
inspector toolsets require an operator-started inspector; `keyboard hide` is
only available as a `.scene` action, not an MCP call.

## 7. Session log

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
- 2026-11-16: `android-flutter` re-check initially failed with `INSTALL_FAILED_UPDATE_INCOMPATIBLE`; `device check --refresh android-flutter` then uninstalled all three packages and passed (`ALI-NX1 · Android 15 · 1200x2652 @3.25x · UiAutomator2`).
- 2026-11-16: Flutter keyboard-hide acceptance passed through the inspector on port 7321: typed `flutter-kb`, `keyboard` reduced the screenshot 173,401 -> 120,161 bytes with the value retained; typed the password, checked terms, hid the keyboard again (121,363 bytes), tapped Login and reached `Invalid format` (144,390 bytes), proving submit was reachable.
- 2026-11-16: imported the personalized tvOS DDI (`BuildManifest` reports 27A9269) into the `23J582` store with `impresario apple ddi import`; `apple ddi status` now reports `23J582: complete` alongside `22H352` and `27B52`.
- 2026-11-16: tvOS `doctor` without `IMPRESARIO_APPLE_DDI` reported `DDI assets: ...\ddi\23J582` and `Apple doctor: ready.` After `apple restart`, a post-reboot doctor showed `DDI: not mounted`; the suite then mounted it and passed `run 20261116-095124-369b` (10/10 acts, 111/111 steps, 230,404 ms, 12 screenshots). T3-R4, A-R2 and INS-R10 are complete.
- 2026-11-16: refreshed the tvOS `default` and iOS `ios-523d45d` signing sets with `--with-stub`; all profiles expire 2026-11-23T09:49-09:50Z and both identities were reused. The iPhone Home act first failed at `phase=prepare` with an AFC socket error; the immediate retry passed `run 20261116-095617-51bb` (11/11 steps, 36.1 s), validating the refreshed profiles. A-R1 is complete.
- 2026-11-16: Android hardware playback probes passed on the stub Media screen. Phone `run 20261116-102116-0599` and TV `run 20261116-102211-2f1f` each completed 8/8 steps: `Showing` video coverage 98.94% / 99.74% with hook `decoded=true`, and `Player` state `Playing` with `position` 3.38 s / 1.637 s, `duration` 4.021 s and `muted=true`; poster coverage was 99.17% / 99.40%. The video check needed a 2 s settle after `MediaStatus` because the hook reports an authoritative position below 1 s at playback start. T3-R1 is complete.
- 2026-11-16: paused playback passed on all four targets with 14/14 steps each. Android phone `run 20261116-110113-0a92` paused at 1.714 s and TV `run 20261116-110200-b506` at 3.709 s via the media-key path; tvOS `run 20261116-115221-d1a6` paused at 1.664 s and iPhone `run 20261116-115340-3b80` at 3.927 s via the new Apple `Pause` button. Every run reported `rate=0`, identical positions 2 s apart, then `rate=1` after resume. Apple Xcode run `43859138304` built the pause control and idle-timer changes at `ee1aa83`. T3-R2 is complete.
- 2026-11-16: the full Apple 10-act suite also passed against the `ee1aa83` products after the Pause control was added: tvOS `run 20261116-120735-0d83` (10/10 acts, 111/111 steps, 3m36s) and iPhone `run 20261116-121116-48f2` (10/10 acts, 107/107 steps, 3m23s). The shipped cross-platform suite does not reference the Apple-only `Pause` piece, so the embedded `IMP-DSL-054` lint stays clean; the paused probe uses its own workspace scene. Main CI `43862446601` at `c02cf0d` and the final docs CI `43864841289` at `68a9ba6` are both green across all eight jobs.
- 2026-11-16: the Apple agent host (`ImpresarioAgentAppDelegate`) and the UI-test runner (`testServe`) now set `UIApplication.shared.isIdleTimerDisabled` while serving, so a long play does not let the device sleep or lock. This is best-effort because the foreground app owns the final idle-timer state; no device verification was requested for this point.
- 2026-11-16: the standalone `ai/` stack verified its bundle and the exact 41-tool partition. The rendered authoring prompt fell from 6,205 to about 4,100 tokens after the compact catalog and constant `call_tool` dispatcher replaced the full schemas; the CPU profile is 131k context, q8_0 KV, 8 threads, `-fa on`, thinking off, 5.08 GiB at full window.
- 2026-11-16: offline toolset exercise passed. Authoring listed devices and validated the workspace; automation read service status, schedules and watchers; inspector-read, inspector-act and scenes each reported `blocked` with the exact no-inspector error. The evidence gate passed 18/18 cases, including overclaim, `not-run`, keyboard-hide, device-name and misspelling cases. The repeated multi-step workspace task that previously claimed a device check ended with `DEVICES: none contacted` and only `impresario_validate` evidence.
- 2026-11-16: inspector lifecycle start/status/stop passed locally with `device: (none)`; no device was attached and no device evidence was produced. Device acceptance remains deferred to the device network (AI-D1 to AI-D6).
