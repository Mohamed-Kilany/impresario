# Platform parity

What every action, check and key does on every platform — reconciled with
observed behavior, not only the spec table. The matrices below are generated
from the planner's own support tables (`impresario-core::support`); the
`parity_doc` test fails if this file drifts from them. `docs/STATUS.md`
records what was verified on real hardware versus unit-tested only.

<!-- BEGIN GENERATED MATRIX -->
Generated from the planner's own tables (`impresario-core::support`) — the same
functions that raise `IMP-DSL-013` at plan time. Kept in sync by
`cargo test -p impresario-master --test parity_doc`.

Legend: ✅ full · ◐ limited (see the list below) · ✖ unsupported — the planner
rejects ✖ before any device is touched.

**Actions** (spec 6.1)

| Action | web | webos | tizen | android | androidtv | ios | tvos |
|---|---|---|---|---|---|---|---|
| `click` | ✅ | ◐ | ◐ | ✅ | ◐ | ✅ | ◐ |
| `type` / `append` / `clear` | ✅ | ◐ | ◐ | ✅ | ✅ | ✅ | ◐ |
| `longpress` | ◐ | ◐ | ◐ | ✅ | ◐ | ✅ | ✅ |
| `focus` | ✅ | ✅ | ✅ | ✖ | ◐ | ✖ | ◐ |
| `select` | ✅ | ✅ | ✅ | ◐ | ◐ | ◐ | ✖ |
| `scroll` | ✅ | ✅ | ✅ | ✅ | ◐ | ✅ | ◐ |
| `scrollto` | ✅ | ✅ | ✅ | ✅ | ◐ | ✅ | ◐ |
| `swipe` | ✖ | ✖ | ✖ | ✅ | ✖ | ✅ | ✖ |
| `back` | ◐ | ◐ | ◐ | ✅ | ✅ | ◐ | ◐ |
| `key <k>` | 6.2 | 6.2 | 6.2 | 6.2 | 6.2 | 6.2 | 6.2 |
| `screenshot` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `app launch` | ◐ | ◐ | ◐ | ✅ | ✅ | ✅ | ✅ |
| `app terminate` | ✖ | ✖ | ✖ | ✅ | ✅ | ✅ | ✅ |
| `app restart` | ◐ | ◐ | ◐ | ✅ | ✅ | ✅ | ✅ |
| `app background` | ✖ | ✖ | ✖ | ✅ | ✅ | ✅ | ✅ |
| `wait` | — | — | — | — | — | — | — |

`wait` is master-side (decision D-10) and never crosses the wire; `key <k>` is
resolved per key in table 6.2 below.

**Checks** (spec 6.1)

| Check | web | webos | tizen | android | androidtv | ios | tvos |
|---|---|---|---|---|---|---|---|
| `visible` `hidden` `enabled` `disabled` `text` `textContains` `count` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `focused` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✅ |

`Check: P` with no condition means `is visible`.

**Remote/hardware keys** (spec 6.2)

| Key | web | webos | tizen | android | androidtv | ios | tvos |
|---|---|---|---|---|---|---|---|
| `ok` | ✅ | ✅ | ✅ | ✖ | ✅ | ✖ | ✅ |
| `enter` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `back` | ✖ | ✅ | ✅ | ✅ | ✅ | ✖ | ✅ |
| `up` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✅ |
| `down` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✅ |
| `left` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✅ |
| `right` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✅ |
| `home` | ✅ | ✖ | ✖ | ✅ | ✅ | ✅ | ✅ |
| `menu` | ✅ | ✖ | ✖ | ✅ | ✅ | ✖ | ✖ |
| `play` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `pause` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `playpause` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✅ |
| `rewind` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `fastforward` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `red` | ✅ | ✅ | ✅ | ✖ | ✅ | ✖ | ✖ |
| `green` | ✅ | ✅ | ✅ | ✖ | ✅ | ✖ | ✖ |
| `yellow` | ✅ | ✅ | ✅ | ✖ | ✅ | ✖ | ✖ |
| `blue` | ✅ | ✅ | ✅ | ✖ | ✅ | ✖ | ✖ |
| `backspace` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num0` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num1` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num2` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num3` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num4` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num5` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num6` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num7` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num8` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `num9` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `volumeup` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `volumedown` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `mute` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `channelup` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `channeldown` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `stop` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `record` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `info` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `guide` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `exit` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `subtitle` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `audio` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `search` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |
| `settings` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✖ |

Dispatch codes/constants per platform are in spec 6.2 and in each executor's
key table (`KeyTable.kt`, `keys.rs`); the planner enforces exactly this ✅/✖ split.

**Limitations behind the ◐ cells**

- **focus+OK** — `click` on `webos`, `tizen`, `androidtv`
- **focus+select** — `click` on `tvos`
- **programmatic** — `type`, `append`, `clear` on `webos`, `tizen`
- **requires focus + keyboard** — `type`, `append`, `clear` on `tvos`
- **mouse hold** — `longpress` on `web`
- **OK held** — `longpress` on `webos`, `tizen`
- **injected OK hold** — `longpress` on `androidtv`
- **D-pad only** — `focus`, `select` on `androidtv`, `tvos`, `android`, `ios`
- **D-pad** — `scroll` on `androidtv`, `tvos`
- **same as focus** — `scrollto` on `androidtv`, `tvos`
- **history** — `back` on `web`
- **back key** — `back` on `webos`, `tizen`
- **nav-bar back button, else edge swipe** — `back` on `ios`
- **menu** — `back` on `tvos`
- **navigate** — `app launch` on `web`, `webos`, `tizen`
- **reload/navigate** — `app restart` on `web`, `webos`, `tizen`
<!-- END GENERATED MATRIX -->

## How unsupported combinations fail

Every ✖ combination fails loudly; nothing is a silent no-op:

- **Plan time (statically knowable).** `impresario validate` and the planner
  reject an action, check or key the target platform does not support with
  `IMP-DSL-013` (the error names the platform and suggests the closest
  supported spelling). No device is touched, no command is sent.
- **Runtime (defense in depth).** Executors still answer
  `UNSUPPORTED_ACTION` if such a command somehow reaches them, so a stale
  planner can never produce a silent no-op:
  - the Android agent guards `swipe` on the TV profile ("no touch"),
    `focus` on phones, `key <k>` per platform profile, `app background`
    when the driver has no support for it, and non-ASCII text entry when
    the `input text` fallback is the only path;
  - the Apple agent guards `focus` on `ios`, `swipe` and `select` on `tvos`,
    `key <k>` per platform profile, and a selector strategy the platform does
    not implement (`classChain`, D-15, and the Android/web strategies);
  - the connector guards what CDP cannot do on web runtimes;
  - the master surfaces the wire error verbatim in the report.

## Notes on the ◐ cells and wording differences from spec 6.1

- `androidtv` `longpress` is "injected OK hold" — the spec's own words. The
  agent focuses the element and injects a real OK key hold through
  `UiAutomation.injectInputEvent` (decision I-66: down, a repeat flagged
  `FLAG_LONG_PRESS`, up, holding ≥ 600 ms), and raises
  `UNSUPPORTED_ACTION` when no injection channel is available.
- `select` on `android`/`androidtv`/`ios`: the spec says "◐" without a note;
  the implementation is D-pad traversal to the element, then OK.
- `app launch` on `web`/`webos`/`tizen` is "navigate"; `app restart` is
  "reload/navigate". The spec prints ✅ with those words inline; the
  implementation classifies both as ◐ limited with the same words, because
  a browser runtime cannot be truly launched/restarted from outside — the
  master navigates/reloads instead. Both classifications allow the action
  (`IMP-DSL-013` is unaffected); the ◐ merely documents the mechanism.
- `focus` is ✖ on touch Android/iOS because out-of-process automation
  cannot move programmatic focus; `swipe` is ✖ on every TV platform (no
  touch). Both also fail at runtime (see above).
- `key <k>` cells: the spec 6.2 dispatch table (keyCodes/constants) is
  mirrored by per-executor key tables with unit tests; the planner enforces
  the same ✅/✖ split shown here.

Apple (`ios`, `tvos`) specifics:

- `select` on `ios` is "D-pad only" like `android`/`androidtv`: the executor
  takes the value from a `pickerWheel` (`adjust(toPickerWheelValue:)`) when the
  located element is one, otherwise it taps the element and taps the match for
  the value's label among the newly visible options.
- `click` on `tvos` traverses focus and presses Select; if the traversal ends
  `FOCUS_UNREACHABLE` but XCTest still reports the element hittable (system
  alerts, focus traps), the executor taps it directly — the honest focus error
  is kept only when the element is not hittable (I-86).
- Text entry reads the field before touching it and verifies the value after
  typing; `append` is implemented as read → delete-all → retype, because
  XCUITest has no clear/append primitive (I-87). A non-text element answers
  `NOT_INTERACTABLE` before the keyboard is involved.
- `scroll` on `tvos` presses D-pad directions (the executor never calls the
  driver's drag path there); on `ios` one step is a drag across 80% of the
  container's visible extent, and `scrollto` splits its budget down-then-up.
- `classChain` (selector strategy, spec 5.3) is **not implemented** for Apple
  (D-15) and answers `UNSUPPORTED_ACTION`/`INVALID_SELECTOR` at runtime; the
  Android strategies (`android_id`, `android_desc`, `android_class`) and the
  web strategies (`css`, `xpath`, `text`, `text_contains`) are rejected with
  `INVALID_SELECTOR` naming the Apple-supported set.
- `clear_data` on `ios`/`tvos` is performed by the **host** (uninstall +
  reinstall the app under test, the app platform's `package`) because no app can delete
  another app's container; without a usable tool the option degrades to a cold
  start plus one WARN line (I-84). On `android`/`androidtv` the agent wipes
  in place.

## Playback probes

`Check: Showing` and `Check: Player` are planned from the same support matrix
(`impresario-core::probe::support`), not by executor-specific guessing. The
table below is a human summary; `docs/PROBES.md` lists the exact sources,
confidence rules and SDK transport.

| Platform | Showing | Player | DRM | Authoritative player source | Known limits |
|---|---|---|---|---|---|
| `web` | ✅ DOM + screenshot coverage + hook | ✅ | ✅ | `HTMLMediaElement`, EME hook, `app_hook` | protected pixels may be black; source is web-only |
| `webos` | ✅ DOM + screenshot coverage + hook | ✅ | ✅ | `HTMLMediaElement`, `media.mediaKeys`, `app_hook` | native pipeline via Luna is not read yet |
| `tizen` | ✅ DOM + screenshot coverage + hook | ✅ | ✅ | `HTMLMediaElement`/AVPlay hook, `mediaKeys`, `app_hook` | AVPlay fields limited to state/time/duration/rate |
| `android` | ✅ a11y + screenshot + hook | ✅ | ✅ | MediaSession, `dumpsys media.drm`, `app_hook` | no decoded/codec/framerate without hook |
| `androidtv` | ✅ a11y + screenshot + hook | ✅ | ✅ | MediaSession, `dumpsys media.drm`, `app_hook` | same as Android |
| `ios` | ✅ a11y + screenshot + hook | ◐ | ◐ | `app_hook` only for player/DRM facts | XCUITest cannot read another app's AVPlayer/FairPlay |
| `tvos` | ✅ a11y + screenshot + hook | ◐ | ◐ | `app_hook` only for player/DRM facts | same as iOS |

◐ means the command is planned and reachable, but a third-party app without
the debug hook yields `unverifiable` rather than a guessed verdict. A missing
piece remains authoritative evidence that no media exists there. There is no
`▶` fallback: unsupported combinations are rejected at plan time
(`IMP-DSL-013`).

## Verification status (what was actually exercised)

| Platform | Matrix verification | Real hardware |
|---|---|---|
| `web` | live E2E `impresario run` → connector → headless Chrome (8/8 steps, exit 0) plus the connector fixture suite covering click/type/append/clear/longpress/select/focus/key/back/scroll/scrollto/checks/screenshots/logs/TV D-pad | desktop Chrome/Chromium (headless), not a TV browser |
| `webos` | unit and loopback-mock tests plus the 105-step stub play through SSH-forwarded CDP | ✅ LG webOS TV, install/update/launch/CDP (I-157) |
| `tizen` | unit and loopback-mock tests plus the 105-step stub play through SDB-forwarded CDP and the player probe | ✅ Samsung Tizen 9.0 TV, native install/launch/signing (I-157) |
| `android` | live E2E on a real phone (`run 20261018-103702-ee29`: 86 steps, 0 failed) incl. touch `scroll`/`swipe`, text entry, keys, app lifecycle | ✅ Galaxy A51, Android 13 (Wi-Fi) |
| `androidtv` | live E2E on a real TV (`run 20261018-104405-4f10`: 89 steps, 0 failed, D-pad `scrollto`, key hold, app lifecycle) + the on-device driver/wire test classes (26/26, 2/2) | ✅ TCL Android TV, Android 11 (Wi-Fi) |
| `ios` | `AgentCore`/`AgentService` are exercised by `swift test` on Linux (executor + dispatcher + focus navigator against fakes; the full wire protocol over loopback gRPC); the XCUITest layer (`XCUIDriver`, `ImpresarioAgentTests`) is compiled unsigned by `Apple Xcode` but its actions have not run on iOS hardware | none: no iOS hardware evidence |
| `tvos` | executor/session behavior under `swift test`; the short-id 105-step stub play passed on the physical Apple TV (`20261028-110828-b477`), and the actual-agent no-stub path passed (`20261028-112028-80fd`); deeper media probes remain limited by the optional app hook | ✅ Apple TV, 105-step stub play and actual-agent run (I-156) |

Hardware details, run ids and case-by-case evidence: `docs/DEVICES.md` and
`docs/STATUS.md`.
