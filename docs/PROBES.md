# Probes: showing, player and DRM truth

This document is the operational reference for `Check: Showing`, `Check: Player` and the
probe evidence model. The algorithm, vocabularies and support matrix live in
`impresario-core::probe`; executors collect evidence and the **master** recomputes every
verdict. No executor and no SDK decides PASS/FAIL (D-27, D-30).

The normative specification is `TASK.md` sections 3–7. This page records how the
implementation behaves, which source is reachable on each platform, and what remains
unverified.

## Status vocabulary

`Check: Showing` answers *is the piece really painted, loaded and not occluded?*
`Check: Player` answers *what is the media pipeline doing?* Verdicts are tri-state:

| Verdict | Meaning | Run effect |
|---|---|---|
| `pass` | Expected side reached the required confidence and nothing contradicts it. | Step passes. |
| `fail` | The opposite side reached the required confidence, or a missing piece authoritatively means no media exists. | Step fails with `CHECK_FAILED` / `IMP-PROBE-*`. |
| `unverifiable` | Neither side reached the threshold, evidence conflicts, or the platform cannot expose the fact. | Default policy fails the step (`--unverifiable fail`); `warn` records `unverifiable` and keeps the run green. |

`unverifiable` is a first-class report status. JSON reports keep it; JUnit renders it as
`skipped` with the reason; console and HTML show it explicitly. It is never silently
converted to PASS.

The default required confidence is `corroborated`. `authoritative` requires one
authoritative source; `inferred` accepts any agreeing evidence. The policy is set by
`unverifiable_policy` in config or `--unverifiable fail|warn` on `impresario run`.
MCP/daemon jobs inherit the config value.

## DSL

```text
Check: Showing => PlayerRegion
Check: not Showing => Poster
Check: Showing => PlayerRegion with coverage>=50%, decoded, source~".m3u8"
Check: Showing => Poster with coverage>=80% timeout 10s

Check: Player => VideoContainer Playing
Check: Player => VideoContainer not Playing
Check: Player => VideoContainer Paused DRM-enabled FairPlay
Check: Player => VideoContainer Playing with position>30s, resolution>=1080p, not muted
Check: Player => VideoContainer with decoder=hardware, not droppedFrames>50 timeout 20s
Check: Player => VideoContainer DRM-enabled with license=active, securityLevel=L1, not keyStatus=expired
```

`Anything` after `with` is an assertion on a player property. A bare property (`muted`,
`secure`) means truthy. `not` negates exactly one term. `Showing` supports the three
options `coverage>=N%`, `decoded`, and `source~"substring"`; `source` is web-only.
Unsupported status/property/provider combinations are rejected at plan time with
`IMP-DSL-013` and a pointer to `docs/PARITY.md`.

Keywords are case-sensitive. `Check: P` without a condition still means `is visible`;
it is not a probe.

## Evidence sources and classes

| Class | Meaning | Examples |
|---|---|---|
| `authoritative` | A direct runtime/API answer, or the app's own report. | `HTMLMediaElement`, EME `keyStatuses`, `MediaController`/MediaSession state, in-app hook, decoded image, captured-region coverage. |
| `system` | A system signal that strongly implies the fact. | secure codec held, system audio, syslog/dlog transition, a foreign session that was not attributed. |
| `heuristic` | UI or visual inference that needs corroboration. | Accessibility labels, player-control text, black protected region alone. |

A missing piece is authoritative evidence that no media exists there. A present piece
with no observable media is **not** a failure; it stays unverifiable unless another
source supplies enough evidence.

### Verdict rules (implemented once in core)

1. Compute confidence for the expected and opposite sides separately.
2. If the expected side wins at the required confidence and no strong contradiction
   exists -> `pass`.
3. If the opposite side wins at the required confidence and nothing strong supports the
   expectation -> `fail`.
4. Otherwise -> `unverifiable` (insufficient or conflicting evidence).
5. The worst claim determines the step status; `UNVERIFIABLE` is converted according to
   `unverifiable_policy`.
6. Executors return `ok=true` with per-claim verdicts for reachable probes. A transport,
   selector or protocol failure is still a normal command error. The master canonicalizes
   the result: it recomputes confidence, verdicts and the reason from evidence and ignores
   executor-supplied verdicts.

## Platform matrix

`reachable` describes what the collector can expose; a third-party app can still leave a
claim unverifiable. Hardware rows marked *unverified* have fixture coverage only.

| Capability | Web / webOS / Tizen | Android / Android TV | iOS / tvOS |
|---|---|---|---|
| Showing geometry/visibility | DOM geometry + hit test | accessibility tree | XCUIElement frame/hittable |
| Showing occlusion | `elementFromPoint` | accessibility windows | XCUITest hit test |
| Showing pixels/coverage | CDP screenshot region | `UiAutomation` screenshot region | `XCUIScreen` screenshot region |
| Image decoded | `complete`, `naturalWidth`, `decode()` | optional hook only; pixels cover occupancy | optional hook only; pixels cover occupancy |
| Video decoded/frame | `readyState`, `getVideoPlaybackQuality`, `requestVideoFrameCallback` (fixtures use quality counters) | optional hook `decoded`; pixels cover presence; no frame counters | optional in-app hook; XCUITest cannot read AVPlayer |
| Player status | HTMLMediaElement/AVPlay (authoritative) | MediaSession or optional hook, filtered to the target package | in-app hook only for third-party apps; no invented state |
| Player properties | HTMLMediaElement/AVPlay fields and Web APIs | MediaSession fields; optional hook supplies the full set | in-app hook fields |
| DRM enabled/provider | EME hook + `media.mediaKeys` | `dumpsys media.drm` only when an active session names the target package; hook overrides | in-app hook only |
| DRM license/key status | EME session wrapper | optional hook only | in-app hook only |
| Source URL | `currentSrc`, CSS `url(...)`, media element | MediaSession metadata or optional hook; showing.source is web-only | optional hook |
| Network metadata | not implemented (`net_flow` unavailable) | not implemented | not implemented (pcapd/DVT are host-assisted and unshipped) |
| In-app hook | `window.__impresarioHook` | `:impresario-hook` + `IMPRESARIO_HOOK` logcat marker | `ImpresarioHook` Swift package over loopback |

Full per-property and source support is defined in `impresario-core::probe::support()` and
summarized in `docs/PARITY.md`. `docs/SPIKES.md` records the fallback rung for each
capability per TASK-3 §6.0.

### Web / webOS / Tizen

The connector installs a versioned, idempotent hook before page scripts and samples the
selected piece for a bounded window. It reads:

- DOM geometry, visibility, hit-testing/occlusion and computed background;
- `HTMLMediaElement`: state, position, duration, buffered, rate, volume, muted,
  dimensions, audio/subtitle tracks, source, error, looping, fullscreen, playback quality
  (total/dropped frames) and `mediaKeys`;
- images: `complete`, `naturalWidth/Height`, `currentSrc`;
- EME: requested key system, provider mapping, robustness -> security level, license
  state and key statuses;
- Tizen AVPlay: state, current time, duration and rate;
- `Page.captureScreenshot` region coverage, computed by the connector.

The probe first waits for the piece to appear (bounded by the step timeout) and, when
`decoded` is required, for decode/visibility; it adds one paint turn before capturing.
This prevents a navigation-timing race from becoming a false FAIL. A screenshot region
that is uniform reports low coverage; a protected/overlay-black region is not treated as
proof of absence.

### Android / Android TV

`ProbeCollector` lives in `agent-core` and parses structured driver snapshots; the driver
implementation is in `driver-uiautomator`.

- **Showing**: accessibility bounds/visibility + topmost window, screenshot-region
  occupancy, and optional hook values for decoded/source. The hook's dedicated `showing`
  object is authoritative; pixels alone prove occupancy, not decode.
- **Player**: `dumpsys media_session` playback state (the block for the target package; no
  fallback to another app's session). `state=` is authoritative only when that session
  reports it; otherwise it stays unverifiable. When the optional app hook reports a media
  snapshot, it takes precedence and evidence is labelled `app_hook`; MediaSession evidence
  stays `media_session`. AudioManager is deliberately not used because it is system-wide,
  not app-scoped.
- **DRM**: `dumpsys media.drm` evidence only when the dump names the target package and
  lists an active session. Provider is derived conservatively. The app hook overrides the
  dump and is labelled `app_hook`. `DRM-disabled` is not inferred from an empty dump;
  license/key status and security level generally need the hook.
- **Not available**: reliable decoded flag, codec, framerate or dropped frames for arbitrary
  players without the hook; no SurfaceFlinger parser or network collector is shipped yet.
- **Hook**: debug-only SDK requires `ImpresarioHook.start(context)` once, then writes an
  `IMPRESARIO_HOOK` JSON line including its package; the driver reads the latest line from
  logcat, rejects a foreign package and treats the snapshot as authoritative. The
  `:impresario-hook` module is built and linked by the stub app; no hardware run has read it
  yet.

The Android media stub is `agent-target-stub/.../MediaActivity.kt`: local H.264/AAC MP4,
decoded PNG, an empty image slot, a real MediaSession and D-pad controls. It asserts that
pixels and session state are actually present; no scenario reports success without drawing
or publishing state.

### Apple iOS / tvOS

XCUITest can inspect another app's accessibility tree, frames, hit testing and screenshots.
It **cannot** read that app's `AVPlayer`, decoder or FairPlay session through public APIs.
The Apple agent therefore does not invent player or DRM state:

- Showing: element presence/visibility/hittability, screenshot-region coverage, and an
  optional hook for decoded/source.
- Player/DRM: evidence comes from the optional in-app hook. Without it, the claims are
  `unverifiable`; a missing selected element still authoritatively contradicts state.
- The Apple stub ships local H.264/AAC media, a still image, an intentionally empty image
  slot and Apple's public BipBop HLS stream. It reports through the Swift hook when the
  runner is listening.
- Host-assisted DVT/syslog/pcapd collectors described in TASK.md are not implemented and
  remain unverified. No iOS/tvOS hardware is available in this environment.

### Own apps: in-app hook

The hook is debug-only and opt-in. It changes achievable confidence, never whether a play
is valid. SDKs live at `sdk/web/impresario-hook.js`, `agents/android/impresario-hook`, and
`agents/apple/Sources/ImpresarioHook`; each exposes the same JSON snapshot shape.

```json
{
  "media": {
    "state": "Playing",
    "positionSeconds": 12.5,
    "durationSeconds": 120.0,
    "rate": 1.0,
    "volume": 0.8,
    "muted": false,
    "widthPx": 1920,
    "heightPx": 1080,
    "audioTrack": "en",
    "subtitleTrack": "",
    "error": "",
    "decoded": true,
    "source": "https://media.example/movie.m3u8"
  },
  "drm": {
    "enabled": true,
    "provider": "Widevine",
    "license": "active",
    "keyStatus": "usable",
    "securityLevel": "L1",
    "hdcp": "HDCP-2.2"
  },
  "showing": {
    "decoded": true,
    "source": "https://media.example/movie.m3u8",
    "piece": "player"
  }
}
```

All fields are optional. Missing means "not observed", never false. The master maps
`source: app_hook` to `evidence_class: authoritative`.

Transport per stack:

- **Web**: include `sdk/web/impresario-hook.js`, call `ImpresarioHook.player/drm/showing`.
  The connector reads `window.__impresarioHook.snapshot`.
- **Swift**: add the `ImpresarioHook` product, call `start()` once in debug, then report.
  It POSTs the snapshot to `http://127.0.0.1:47831/hook`; the runner keeps the newest
  snapshot. Missing listener is silently ignored.
- **Android**: depend on `:impresario-hook`, call `ImpresarioHook.start(context)` once (the
   stub does this in `MediaActivity`), then report. The library logs
   `IMPRESARIO_HOOK <json>` under the `ImpresarioHook` tag, including the package name; the
   driver reads the latest line with shell identity and rejects another package's snapshot.
   Logcat must not be disabled for debug builds.

```js
// Web
ImpresarioHook.player({ state: "Playing", positionSeconds: 12.5 });
ImpresarioHook.drm({ enabled: true, provider: "Widevine", keyStatus: "usable" });
ImpresarioHook.showing({ decoded: true, source: "https://cdn.example/movie.m3u8" });
```

```swift
// Swift (debug only)
ImpresarioHook.shared.start()
ImpresarioHook.shared.report(
    media: .init(state: "Playing", positionSeconds: 12.5),
    drm: .init(enabled: true, provider: "FairPlay", keyStatus: "usable"),
    showing: .init(decoded: true, source: "https://cdn.example/movie.m3u8"))
```

```kotlin
// Android (debugImplementation)
ImpresarioHook.start(context)
ImpresarioHook.player(ImpresarioHook.Media(state = "Playing", positionSeconds = 12.5))
ImpresarioHook.drm(ImpresarioHook.Drm(enabled = true, provider = "Widevine"))
ImpresarioHook.showing(
    ImpresarioHook.Showing(decoded = true, source = "https://cdn.example/movie.m3u8"))
```

Do not report paused as playing, synthetic progress, or a guessed DRM state. The probe
rules will treat conflicting hook and platform evidence as unverifiable, which is the
correct result.

## Running probes

Today probes run as planned steps:

```text
impresario run --dry-run            # check the plan and IMP-DSL-013 diagnostics
impresario run --device web1        # execute
impresario run --unverifiable warn  # keep unverifiable claims non-fatal
```

`impresario probe ...` live inspection and the TUI probe screen are specified but not
implemented; use `--dry-run` plus the report evidence table until then. MCP runs inherit
the same planner, probe commands and policy.

## Fixtures

| Fixture | Covers |
|---|---|
| `fixtures/web/media.html` + `sample.webm` + `sample.png` | Web Showing coverage/decoded/source, HTMLMedia Player state/properties for video and audio-only elements, image decode, missing piece; local and offline. |
| `fixtures/web/hook.html` + `sdk/web/impresario-hook.js` | Web in-app hook shape and connector `app_hook` evidence. |
| `fixtures/web/tizen-avplay.html` | Synthetic Tizen `webapis.avplay` API for the connector parser and Tizen-only source health; not a hardware playback result. |
| `agents/android/agent-target-stub` `MediaActivity` | Android MediaSession state, local video, decoded and empty images, screenshot coverage, D-pad controls, `:impresario-hook` reporting. |
| `agents/apple/Xcode/AgentTargetStub/MediaViewController.swift` | Apple local video/audio/image, empty image slot, public HLS, Swift hook reports. |
| Public streams | Apple BipBop HLS; no private account, FairPlay asset or license proxy is used. |

The web fixture is exercised by
`crates/impresario-connector/tests/fixtures_suite.rs::media_probes_collect_showing_player_and_drm_evidence`;
the optional hook contract is exercised by
`web_app_hook_supplies_authoritative_player_evidence`. The Android and Apple fixture apps
compile in their own toolchains but hardware playback acceptance is deferred (`HD-1`,
`HD-5`, `HD-8`). Web probes on other managed Chromium archives are part of `HD-6`; real-TV
CDP is `HD-7`.

## Unverified work

- No webOS TV, Tizen TV, iOS or tvOS hardware is available. Transport/probe paths are
  verified with fakes, fixtures and replay only.
- Apple host-side DVT/syslog/pcapd evidence and the in-app hook over loopback are not
  hardware-tested.
- Android exact decoder/codec/framerate properties and `dumpsys media.drm` semantics are
  vendor/API-level dependent; only conservative parsers are shipped.
- Public stream URLs can change; local fixtures are authoritative for CI.
- Third-party DRM streams (Widevine/FairPlay) are not used in CI.

See `docs/STATUS.md` for the current ledger and `docs/DECISIONS.md` D-26…D-43 for the
binding decisions.
