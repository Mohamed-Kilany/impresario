# TASK-3.md — impresario, third generation: truth-checking probes, resident runner, actor-based DSL

> **Superseded.** Consolidated into [`TASK.md`](TASK.md), which is the single authoritative
> specification. This file is retained for provenance only; do not implement from it.

Status: normative delta spec. It builds on `TASK.md` (v1) and `TASK-2.md` (v2). Read `IMPRESARIO.md`,
`README.md`, `TASK.md`, `TASK-2.md`, `docs/DECISIONS.md` (D-01…D-25, I-01…I-90+), `docs/STATUS.md`
and `docs/SPIKES.md` **before touching anything**.

- Where TASK-2 is not yet implemented, implement the parts of it that this file depends on
  (global home, device/app split, theatre/scene/play DSL, daemon/jobs, lossless config) — never
  duplicate or fork them. Where this file conflicts with TASK/TASK-2, **this file wins** and the
  conflict is recorded as a numbered decision (continue at the next free `I-nn`; binding decisions of
  this spec are `D-26…`). Mark superseded decisions `Superseded by …`, never rewrite them.
- All working rules of TASK-2 §1 stay in force: spike before commit, no unverified claims, protocol
  facts verified from sources, licenses recorded, secrets only in the keychain, commit per work
  package with tests/docs/decisions/STATUS, fail loudly (`UNSUPPORTED_ACTION`), no silent no-ops,
  generated PARITY, honest STATUS, Rust gates green at every commit.
- **Never assume.** Every item in §13 ("Open items") must be put to the user as a question before the
  affected work package starts; record the answer as a decision.

---

## 0. Goal

Testing an app such as Netflix must need **no third party**: impresario alone must verify the app's
functionality *and* its streaming/business reality — "is the image really on screen", "is the video
really playing, on which decoder, at which quality", "is the DRM license really in use, with which
provider and level" — on every supported platform, from the outside, as a legitimate end-user journey.
In addition the master gains a resident runner (scheduler + always-on watcher) that keeps working
after the CLI/TUI is closed, and the DSL moves to an **actor / audience / interface-implementation**
model with two new truth-check forms.

## 1. Hard rules (legal, ethical, technical)

1. **Legitimate journeys only.** No jailbreak, no root requirement, no private entitlements that
   cannot be obtained by a normal developer/test setup, no DRM/HDCP circumvention, no capture of
   protected frames, no extraction, logging or storing of keys, license payloads, tokens or content.
   Probes read **system state and metadata** (states, counters, flags, hostnames, sizes, timings).
   License/handshake *payloads* are never persisted; only their existence, timing, size, status.
2. **Evidence, not guesses.** Every probe verdict is derived from recorded evidence items with a
   confidence class (§3). A probe that cannot reach the required confidence returns
   **UNVERIFIABLE**, which is neither pass nor fail and is reported as such (§3.4). Heuristics are
   labelled as heuristics in the report; they never masquerade as authoritative.
3. **Low level, wrapper-agnostic.** For players, the truth comes from the system layers underneath
   (codec, audio, compositor, DRM, network), not from the wrapper's UI — Bitmovin, MX Player, Shaka,
   ExoPlayer, AVPlayer wrappers all converge on the same system resources. Wrapper-published state
   (a MediaSession, an HTMLMediaElement) is *additional* evidence, never the only one when system
   evidence is available.
4. **Version-robust.** System interfaces differ per OS version and vendor. Prefer public/structured
   APIs (even under shell identity) over parsing `dumpsys` text; where text parsing is unavoidable,
   it is feature-detected, covered by per-OS-version golden fixtures, and a parse failure degrades to
   UNVERIFIABLE with the raw text attached to the report (redacted), never to a false PASS/FAIL.
5. **Cost-aware.** Probes sample for a bounded window (default 1 s, max configurable), never poll
   the device continuously unless a probe session is open, and never degrade the app under test
   noticeably (no heavy tracing left running; all collectors stop on command end or `Cancel`).

## 2. New binding decisions (copy into DECISIONS.md as D-26…)

| ID | Decision |
|---|---|
| D-26 | **Tri-state probe verdicts**: `PASS`, `FAIL`, `UNVERIFIABLE`. `UNVERIFIABLE` is a first-class report status (`report.json`, JUnit as `skipped`+message, console, HTML). Default policy `unverifiable_policy: fail` (configurable `fail|warn`, per run via `--unverifiable`). |
| D-27 | **Evidence model** lives in `impresario-core::probe` (single source of truth for verdict logic, thresholds, property/provider/status vocabularies and the per-platform probe support matrix). Executors and master-side collectors only *produce evidence*; the verdict is computed in one place so platforms cannot diverge. |
| D-28 | **Probe commands are wire commands** (new `Agent` commands + capabilities), run by the executor that owns the semantics: Android agent, Apple agent (+ host-assisted collectors), connector (web/webOS/Tizen). They follow D-09 (auto-wait until the condition holds or the timeout elapses). |
| D-29 | **`Platforms:` on plays/acts is removed** and replaced by **`Actors:`** = device names (and workspace-defined groups). Each actor resolves to a device, whose **platform** drives support checks, selector columns, fallback chain, and audience matching. No alias, no deprecation window (`migrate` converts). |
| D-30 | **`Perform` is an interface; `Stage` is an implementation.** Several stages may share one name provided their `Audience` platforms do not overlap: uniqueness key = `(stage name, audience platform)`. The interface is **implicit** (derived from the implementations); all implementations of one name must have identical parameter signatures. Supersedes TASK-2 §8.3 "stage names globally unique" and D-22's uniqueness clause only. |
| D-31 | **Audience matching is exact** (the device's platform), with **no fallback chain** (`webos` does not match `web`, `androidtv` does not match `android`). The selector fallback chain (D-07) is unaffected. |
| D-32 | **Resident runner = the per-user background process** (the TASK-2 §3.5 daemon extended), started at user logon through the OS's per-user mechanism, not a system service. It hosts jobs, schedules and watchers and survives closing the CLI/TUI. |
| D-33 | New checks: `Check: Showing => Piece` (actually rendered and seen) and `Check: Player => Piece …` (player/decoder/DRM truth). `is visible` keeps its meaning: **the element exists in the view/DOM tree and is displayed per its own attributes**; `Showing` means **pixels/frames of it are really being presented**. |

---

## 3. The evidence model (`impresario-core::probe`)

### 3.1 Types
```
Evidence {
  source:      enum   // e.g. a11y_tree, surfaceflinger, media_session, audio_playback, media_codec,
                      //      media_drm, logcat_tag, net_flow, screenshot_region, dom, html_media,
                      //      eme_hook, avplay_hook, luna_media, dvt_sysmon, dvt_graphics, dvt_network,
                      //      syslog, pcapd, now_playing_ui, audio_session, app_hook
  kind:        string // stable machine name, e.g. "frame_counter_advancing"
  observation: value  // typed (bool/number/string/enum/range) — never raw payloads
  at:          timestamp + window_ms
  class:       AUTHORITATIVE | SYSTEM | HEURISTIC
  supports:    [claim ids]   // which claim this evidence bears on (e.g. state=Playing, drm.provider=Widevine)
  contradicts: [claim ids]
}
```
- `AUTHORITATIVE`: a direct, typed system/runtime API states the fact (HTMLMediaElement/EME,
  `MediaController` PlaybackState, `MediaDrm` query, `AudioPlaybackConfiguration`, in-app hook).
- `SYSTEM`: a system-level signal that implies the fact (codec resource held, compositor layer buffers
  advancing, audio track started for the app's uid, secure-codec in use, network flow to a license host).
- `HEURISTIC`: UI/visual inference (accessibility labels, pixel analysis, black protected region).
- **Verdict per claim**: `PASS` if the required confidence is met and nothing contradicts;
  `FAIL` if sufficient evidence says the claim is false (or the timeout elapsed with evidence of the
  opposite); `UNVERIFIABLE` if the required confidence cannot be reached (platform cannot provide it,
  collector failed, evidence conflicts without resolution).
- **Required confidence** (config `probe.min_confidence`, default `corroborated`, overridable per
  run): `authoritative` = one AUTHORITATIVE item; `corroborated` = one AUTHORITATIVE **or** at least two
  independent SYSTEM/HEURISTIC items (distinct `source`) that agree and at least one is SYSTEM;
  `inferred` = any agreeing evidence. Defined once in core, unit-tested exhaustively.

### 3.2 Probe results on the wire and in reports
Each probe result carries: verdict per claim, the evidence list (trimmed, redacted), the sampling
window, the confidence reached, and **a "why unverifiable" reason code** when applicable
(`IMP-PROBE-*`, §10). The HTML report renders an evidence table per probe (source, class, observation,
time), thumbnails of the analysed region (only when the region is not protected), counter sparklines
(frames/bytes over the window) and the raw system text snippets that were parsed (redacted).
`report.json` schema version is bumped; `exit_code` semantics follow I-21/I-28 plus D-26.

### 3.3 Probe session / live inspection (full visibility)
`impresario probe <showing|player|drm> --device <d> [--app <a>] [--selector …|--piece Scene.Piece]
[--watch]` prints the raw live evidence stream for debugging and for writing plays; the TUI has a
"Probe" screen (live signal table, per-source health). MCP exposes the same (§9). This is also how the
Phase-0 signal matrices are produced.

### 3.4 Policy
`unverifiable_policy: fail` (default) turns UNVERIFIABLE into a failed step with code
`IMP-PROBE-UNVERIFIABLE` and the reason; `warn` marks the step `unverifiable` (passes the run, listed in
the summary and notifications). Never hidden.

---

## 4. Probe 1 — "Showing" (really rendered and seen)

**Meaning.** The target piece (element/container/image/video surface) is not merely in the tree: its
content is **painted, loaded and not occluded**. Examples it must catch: an `<img>`/`ImageView` whose
network image never arrived (placeholder or empty), a lazily loaded image still off-paint, a
`background-image` that 404'd, a video container showing no frames, an element covered by another
window/overlay/dialog, zero-alpha or clipped content.

### 4.1 Layered algorithm (core defines it; executors supply the layers)
1. **Geometry**: exists, non-empty bounds, inside the visible viewport/display, not clipped away.
2. **Occlusion**: topmost at its center and at sampled points (hit-testing / window z-order);
   alpha/opacity > threshold; not under another window (system dialogs, IME, overlays).
3. **Content loaded** (type-specific, authoritative where the platform allows): image decoded (not the
   placeholder, not an error image), background image loaded, video has presented a frame.
4. **Pixels** (heuristic, optional, region screenshot): region is not uniform/transparent, entropy/
   variance above `showing.pixel_variance_min`, differs from a configured placeholder fingerprint;
   for a **protected** surface (secure layer/`FLAG_SECURE`/HDCP-redacted black), the pixel layer is
   *not applicable* and the protected-surface evidence (§6) replaces it — never reported as "blank".
Verdict = per layer verdicts combined by core: all applicable layers agree → PASS; any applicable layer
contradicts → FAIL; required layers unavailable → UNVERIFIABLE.

### 4.2 Per platform (starting points to verify in Phase-0 spikes)
- **web / webOS / Tizen (connector, CDP)**: `img.complete && naturalWidth>0 && currentSrc` plus
  `HTMLImageElement.decode()`; `CSS background-image` via `Network`/`Page.getResourceTree`/resource
  timing; `getVideoPlaybackQuality().totalVideoFrames` and `requestVideoFrameCallback` for video;
  `elementFromPoint` occlusion; `IntersectionObserver` ratio; `Page.captureScreenshot` with `clip` for
  the pixel layer (on TVs with hardware video overlay the video region may be black — then protected/
  overlay logic applies and the verdict relies on the frame-presentation evidence).
- **Android / Android TV (agent)**: accessibility bounds/visibility + window z-order via accessibility
  windows (not `dumpsys` text when the API exists); `UiAutomation.takeScreenshot` region analysis;
  compositor evidence via SurfaceFlinger (layer present, visible region, buffer count advancing,
  `secure` flag) — through a structured source if one exists, otherwise a version-guarded parser with
  golden fixtures; for images: `ImageView`/`Drawable` state is app-internal and **not** assumed visible
  from outside, so pixels + non-placeholder fingerprint are the evidence, labelled HEURISTIC.
- **iOS / tvOS (Apple agent + host-assisted)**: `XCUIElement` frame/hittable/`isEnabled`; XCUITest screenshot
  region analysis (HEURISTIC); protected regions render black in XCTest screenshots — treat as
  protected-surface evidence only together with §6 signals. No private API.

### 4.3 DSL
`Check: Showing => Piece [with …] [timeout 10s]` and `Check: not Showing => Piece`. `with` options
(minimal set, confirm in §13): `coverage>=50%` (share of the piece's area judged painted), `decoded`
(image/video decoded flag where authoritative), `source~"substring"` (web only).

---

## 5. Probe 2 — Player truth (low-level, wrapper-agnostic)

### 5.1 Vocabulary (core, single source; extend only via decision)
- **Status** (mutually exclusive): `Idle, Loading, Buffering, Ready, Playing, Paused, Seeking, Ended, Error`.
- **Properties** usable in `with`/`not` (each has type, unit, per-platform support and evidence class in
  the generated PARITY probe matrix): `position`, `duration`, `buffered`, `rate`, `volume`, `muted`,
  `live`, `looping`, `fullscreen`, `width`, `height`, `resolution` (e.g. `1080p`), `bitrate`, `framerate`,
  `droppedFrames`, `videoCodec`, `audioCodec`, `decoder` (`hardware|software|<name>`), `secure`
  (secure decoding/output path in use), `audioTrack`, `subtitleTrack`, `error` (code/name),
  plus DRM properties in §6 (`license`, `keyStatus`, `securityLevel`, `keySystem`, `hdcp`).
- Operators: `= != > >= < <= ~` (substring/regex-lite for strings); values: numbers, durations (`30s`),
  percentages, enums, quoted strings, resolutions.
- Unsupported (status/property/provider × platform) → `IMP-DSL-013` at plan time with the platform and
  the closest supported spelling, exactly like actions and keys.

### 5.2 Android / Android TV (agent; the most complete platform)
All collected under shell identity from the instrumentation process (`UiAutomation.adoptShellPermissionIdentity`,
as already used for key injection, I-66; verify exactly which permissions each API needs per API level).
- **Session layer (AUTHORITATIVE when the app/wrapper publishes it)**: `MediaSessionManager.getActiveSessions`
  → `MediaController` `PlaybackState` (state, position, speed, buffered position, actions), `MediaMetadata`
  (duration, title). Covers ExoPlayer/Media3, MX-style players, most wrappers. If no session exists, this is
  simply absent — not a failure.
- **Audio layer (SYSTEM)**: `AudioManager.getActivePlaybackConfigurations()` (player state STARTED/PAUSED/
  STOPPED, uid, usage, content type, session id); AudioFlinger/audio policy tracks as corroboration.
- **Decoder layer — the "busy decoder" signal (SYSTEM)**: MediaCodec resources held per process via the
  media resource manager (client pid/uid, codec type, **secure** and **hardware** flags, count/busy state),
  `MediaCodecList` secure-decoder capability, per-process codec threads/CPU as last-resort corroboration;
  media metrics (`MediaMetricsManager`/playback metrics: dropped frames, rebuffer counts, resolution,
  bitrate, codec) where the API level provides them.
- **Compositor layer (SYSTEM)**: SurfaceFlinger layer for the app's video surface — present, visible,
  buffer queue advancing (frame counter delta over the window), `secure`/protected flag, size → gives
  `Playing` vs `Paused/Buffering` independent of the player UI, and `width/height/framerate`.
- **UI layer (HEURISTIC)**: accessibility labels/values of player controls (Play/Pause, seek bar value,
  time labels), usable for wrappers with standard controls.
- **Network layer (SYSTEM, optional)**: per-uid traffic counters and, if the user enables the
  `local_vpn_tap` option (explicit consent dialog), flow metadata (SNI host, bytes, timing; TLS is never
  broken) to confirm manifest/segment/license traffic. Off by default.
- **Own apps**: optional in-app test hook (§6.5) upgrades everything to AUTHORITATIVE.
- Verify per device class: Galaxy A51 (Android 13), TCL Android TV (Android 11, vendor freezer I-67/68 —
  collectors must stay inside the agent process budget), Realme RMX3997 (Android 16, restrictive ROM I-82).
  Record in `docs/PROBES.md` which signal exists on which API level/vendor.

### 5.3 iOS / tvOS (legitimate-only strategy — the hard platform)
A third-party app's AVPlayer cannot be inspected from another process through public APIs. The legitimate
route is **corroboration across several sanctioned channels**, each documented with its limits. Phase-0 must
produce a signal matrix (iOS 16/17/18, tvOS 17/18: availability, redaction, prerequisites — Developer Mode,
mounted DDI, trust) and prove each channel on real hardware or mark it unverified:
1. **Audio session (SYSTEM)**: in the runner process, `AVAudioSession.isOtherAudioPlaying` /
   `secondaryAudioShouldBeSilencedHint` — public API reporting whether another app is producing audio →
   playing vs not playing, and (with timing) paused-after-playing.
2. **Accessibility tree (HEURISTIC)**: player control labels/values (Play/Pause, slider value `"1:23 of 45:10"`,
   elapsed/remaining labels), traits (`Updates Frequently`, `Plays Sound`); authoritative-ish for standard
   `AVPlayerViewController` UI, app-defined for custom controls.
3. **System Now Playing UI (HEURISTIC/SYSTEM)**: drive Control Center / lock-screen media card through
   XCUITest on SpringBoard (and the Now Playing tile on tvOS) and read title, play/pause state, elapsed/
   duration — a UI the *system* renders from the app's Now Playing info. Verify feasibility per OS version;
   it must restore the app's foreground state and be rate-limited.
4. **Host-side Instruments channels via the device tooling stack (SYSTEM)**: the same developer services
   Xcode's Instruments uses, reached through `idevice` once Developer Mode and the DDI are in place (they are
   already required for XCTest in TASK-2 §6.2): process monitor (CPU/memory of the app and of media daemons —
   a hardware decoder at work shows up as media-daemon activity), graphics/FPS and GPU utilisation
   (Core Animation frame production → frames really being presented), per-process **network** counters
   (streaming traffic = segments flowing), energy/thermal as weak corroboration. Verify the exact service
   names and field sets (go-ios / pymobiledevice3 expose them; study for facts, license rule TASK-2 §1.4).
5. **Unified log relay (SYSTEM, redaction-aware)**: syslog/os_log relay filtered to media subsystems
   (CoreMedia, AVFoundation, VideoToolbox decoder session create/destroy, FairPlay/`mediaserverd` events);
   `<private>` redaction means we match **event names and state transitions**, never payloads. Treat as
   corroboration only until the spike proves stability per OS version.
6. **Network metadata (SYSTEM)**: the device packet-capture relay (`pcapd` service) for per-flow metadata
   only (destination host/SNI, bytes, timing) to confirm manifest/segment/**license-server** traffic against
   user-configured host patterns (`probe.net.license_hosts`, `probe.net.media_hosts`). No payload storage,
   no TLS interception.
7. **Protected-surface inference (HEURISTIC)**: XCTest screenshots black out protected video while the
   surrounding UI is intact; "region black + UI chrome intact + audio playing + decoder/network activity" is
   corroborated evidence of protected playback. Black alone is never a verdict.
8. **Own apps (AUTHORITATIVE)**: optional in-app test hook (§6.5).
- Achievable confidence for third-party apps on Apple is therefore **corroborated** in the best case and is
  reported as such; properties that cannot be reached (e.g. exact bitrate, DRM security level) are
  UNVERIFIABLE for black-box apps and the matrix says so. If a spike shows a channel is unusable, the
  decision records why — the product never fakes it. No jailbreak, no private entitlements.
- External capture (HDMI capture box) is explicitly **out of scope**: HDCP forbids it for protected content.

### 5.4 Web / webOS / Tizen (connector — no agent)
- **HTMLMediaElement (AUTHORITATIVE)**: `paused, ended, readyState, networkState, currentTime, duration,
  buffered, seeking, playbackRate, volume, muted, videoWidth/Height, error, audioTracks, textTracks,
  getVideoPlaybackQuality()` (dropped/total frames) and `requestVideoFrameCallback` (frames truly
  presented, timestamps). Shadow DOM and iframes traversed (I-43 helper).
- **MSE (AUTHORITATIVE)**: hooks on `MediaSource/SourceBuffer` (appended ranges, codecs, quota/errors).
- **Wrappers** (shaka, hls.js, dash.js, Bitmovin web, vendor SDKs) are irrelevant because the truth is at
  the media element/MSE/EME layer; known wrapper APIs may add AUTHORITATIVE properties (quality ladder,
  active variant) via optional adapters, off by default.
- **Hook injection** before page scripts: `Page.addScriptToEvaluateOnNewDocument` (re-injected on every
  navigation, I-42 style) wraps `HTMLMediaElement.prototype.play/pause/load`, MSE and EME entry points
  and (Tizen) `webapis.avplay` / `webapis.avplay.prepareAsync/play/pause/…` + its listener callbacks, so
  **Tizen AVPlay** state (`getState()`, `getCurrentTime()`, `getDuration()`, streaming properties) is
  readable even though it is not an HTML element. Hooks record state and metadata only.
- **webOS native pipeline**: for apps using the platform media service rather than a plain `<video>`,
  query the pipeline state through the TV's media service over the SSH channel (TASK-2 §6.3; verify the
  Luna method names and their output) as SYSTEM evidence.
- **Tizen platform logs**: dlog tags for avplay/DRM over the sdb channel (TASK-2 §6.4) as corroboration.
- Network: CDP `Network` domain for manifest/segment/license requests (metadata only).

### 5.5 DSL
```
Check: Player => VideoContainer Playing
Check: Player => VideoContainer not Playing
Check: Player => VideoContainer Paused DRM-enabled FairPlay
Check: Player => VideoContainer Playing with position>30s, resolution>=1080p, not muted
Check: Player => VideoContainer with decoder=hardware, not droppedFrames>50 timeout 20s
```
Grammar (EBNF delta; `piece` as before):
```
check-line   = "Check:" ( showing-check | player-check | old-check ) EOL ;
showing-check= [ "not" ] "Showing" "=>" piece [ with ] [ tmo ] ;
player-check = "Player" "=>" piece [ [ "not" ] status ] [ drm ] [ with ] [ tmo ] ;   (* at least one of status|drm|with *)
status       = "Idle"|"Loading"|"Buffering"|"Ready"|"Playing"|"Paused"|"Seeking"|"Ended"|"Error" ;
drm          = ( "DRM-enabled" | "DRM-disabled" ) [ [ "not" ] provider ] ;               (* see §6 *)
with         = "with" assertion { "," assertion } ;
assertion    = [ "not" ] property [ op value ] ;                                         (* bare property = truthy (e.g. secure, muted) *)
```
`not` is allowed on the status, the provider and every `with` assertion; it negates that term only.
Keywords are case-sensitive. `IMP-DSL-013` rejects unsupported combinations per platform at plan time.

---

## 6. Probe 3 — DRM license usage and status (low level)

### 6.1 Claims
`DRM-enabled | DRM-disabled` (is protected/DRM playback in effect for this player), optional **provider**
(`Widevine | PlayReady | FairPlay | ClearKey`), and `with` properties: `license` (`none|requested|active|
expired|released|error`), `keyStatus` (`usable|expired|outputRestricted|outputDownscaled|internalError|
statusPending`), `securityLevel` (`L1|L2|L3|SW|HW` as the platform names it), `keySystem` (string),
`hdcp` (level), `secure` (secure decode/output path).

### 6.2 Android / Android TV
- **DRM plug-in/session layer**: the DRM service/HAL view of active sessions per client (via the system
  service dump/HAL query under shell identity — verify the available structured source per API level),
  provider from plug-in UUID (Widevine `edef8ba9…`, PlayReady `9a04f079…`, ClearKey `e2719d58…`), and
  device-level properties through our own `MediaDrm` instance (`securityLevel`, `maxHdcpLevel`,
  `currentHdcpLevel`, vendor/version) — device facts, labelled as such (they describe the device, not the
  app's session).
- **Secure decode (SYSTEM)**: media resource manager shows a **secure** codec held by the app's pid;
  SurfaceFlinger layer has the `secure`/protected flag; screenshot of the region fails/blank (protected
  evidence). These two together are strong corroboration of protected playback even when the DRM session
  detail is unreadable.
- **License lifecycle (SYSTEM)**: DRM logging tags and OEMCrypto/Widevine log events for
  provisioning/license request/response/key update (names/timing only); optional flow metadata to the
  license host through the network tap (§5.2). Key status/expiry only from authoritative sources
  (own-app hook or `MediaDrm` events when accessible) — otherwise UNVERIFIABLE.
- Netflix-class apps use their own DRM wiring and a custom player: the above is deliberately
  wrapper-agnostic; the Phase-0 spike must run on a real Netflix-class app on the TCL TV and the phone and
  record exactly which claims reach which confidence.

### 6.3 iOS / tvOS
FairPlay state is inside the app process. Legitimate black-box evidence: protected-surface inference (§5.3 #7),
audio/decoder/network corroboration, FairPlay/CoreMedia log events (`FairPlay` subsystem names, key request
events — names/timing only), license-server flow metadata against configured hosts. Provider claim `FairPlay`
is reachable at **corroborated** at best (protected playback on Apple platforms ⇒ FairPlay, and a Widevine/
PlayReady claim on iOS is `IMP-DSL-013`); `securityLevel`, `keyStatus`, `license` states are
UNVERIFIABLE black-box and AUTHORITATIVE only with the in-app hook.

### 6.4 Web / webOS / Tizen (EME — authoritative)
Hook `navigator.requestMediaKeySystemAccess`, `MediaKeySystemAccess.createMediaKeys/getConfiguration`,
`MediaKeys.createSession/setServerCertificate`, `MediaKeySession` (`generateRequest`, `update`, `close`,
`message`, `keystatuseschange`, `expiration`, `closed`, `sessionId`, `keyStatuses` entries' *statuses*,
never key ids/payloads). Derive: key system ⇒ provider (`com.widevine.alpha`, `com.microsoft.playready*`,
`com.apple.fps*`/`com.apple.fairplay`, `org.w3.clearkey`), requested/granted robustness (`securityLevel`),
license request/response timing and sizes, `keyStatus`, expiry, session lifecycle; correlate with the media
element via `HTMLMediaElement.mediaKeys`. webOS/Tizen: same hooks inside their Chromium-based runtimes;
Tizen **AVPlay** DRM (`setDrm`) wrapped the same way; platform logs as corroboration. Verify what each
old TV Chromium build supports (D-03) and degrade per claim.

### 6.5 Optional in-app test hook (own apps)
A tiny, debug-only, opt-in library/contract an app owner can add (Android AAR, Swift package, JS snippet),
exposing `player state / properties / drm state` over the same evidence types as `source: app_hook,
class: AUTHORITATIVE`. Specified as a stable JSON contract in `docs/PROBES.md`; **plays never change**
whether the hook is present — only the achievable confidence does. Implement the contract and reference
fixtures; shipping SDKs for all stacks is out of scope unless the user asks (§13).

### 6.6 DSL
```
Check: Player => VideoContainer Playing DRM-enabled Widevine
Check: Player => VideoContainer Paused DRM-enabled FairPlay
Check: Player => VideoContainer DRM-disabled
Check: Player => VideoContainer DRM-enabled with license=active, securityLevel=L1, not keyStatus=expired
Check: Player => VideoContainer Playing DRM-enabled not Widevine
```

---

## 7. Wire protocol, capabilities, connector (D-28)

1. **proto** (`impresario.v1`): new commands `ProbeShowing`, `ProbePlayer`, `ProbeDrm` (one `Probe` command
   with a `kind` is acceptable — decide in the spike), each carrying selector(s), claims, `with` assertions,
   window/timeout, `min_confidence`; result = `ProbeResult { verdicts[], evidence[], confidence_reached,
   unverifiable_reason?, observed{}, window_ms }`. New capabilities: `PROBE_SHOWING`, `PROBE_PLAYER`,
   `PROBE_DRM`, `NET_METADATA`, plus per-source health in the handshake (`ProbeSourceInfo{source, available,
   reason}`) so the master knows *before* running whether a claim is reachable.
2. **Master-side collectors**: for Apple (and optionally Android network tap) some evidence originates on the
   host (DVT, syslog, pcapd, flow metadata). `impresario-clients` gets a `SignalSource` trait; the master's
   probe aggregator merges executor evidence and host-side evidence into one `ProbeResult` and applies the
   D-27 verdict logic. Host-side collectors respect leases and cancellation and are covered by recorded-trace
   tests.
3. **Planner**: probes are plan-time-checked against the generated probe support matrix
   (`IMP-DSL-013`) and, at handshake time, against the device's reported source health
   (`UNSUPPORTED_ACTION`/`UNVERIFIABLE` with the reason before the step runs, never a silent skip).
4. **`MockAgent`** learns scripted evidence so every verdict path (PASS/FAIL/UNVERIFIABLE, conflicting
   evidence, missing source) is testable without hardware.
5. **Connector**: hook bundle (versioned JS, idempotent, tested against fixtures and old-Chromium
   constraints), EME/MSE/AVPlay wrappers, probe commands, no new external dependency (D-19).
6. **Android agent**: new modules `:probe-core` (pure Kotlin/JVM: evidence types, parsers, golden tests),
   `:probe-android` (collectors), integrated into `CommandExecutor`; **Apple agent**: probe collectors in
   `AgentCore` behind fakes (`swift test` on Linux) + XCUITest/AVAudioSession adapters in the runner.
7. **Fixtures (own apps, no third party needed for CI-able tests)**: Android stub gains a Player scenario
   set (Media3/ExoPlayer, `MediaPlayer`, `VideoView`, a wrapper-style custom view; clear stream, Widevine
   test stream, ClearKey), an image scenario (loaded / 404 placeholder / lazy / covered by overlay / zero
   alpha); web fixtures for `<video>`, MSE, EME ClearKey (no server needed) and a Widevine test stream,
   images (good, broken, lazy, `background-image`), an AVPlay shim fixture for the Tizen hook logic;
   Apple stub gains AVPlayer scenarios (clear HLS; FairPlay only when the user supplies a test asset and
   certificate — §13). Use publicly available DRM test streams/license proxies and record exact URLs, with
   a note that they may change; fixtures must also work offline with locally served clear/ClearKey media.

---

## 8. Master work package W-RESIDENT — the resident runner (D-32)

### 8.1 Process model
- The TASK-2 daemon becomes the **resident runner**: `impresario service run` (foreground),
  `service install | uninstall | start | stop | restart | status | logs [-f] | pause | resume`.
  Closing the CLI/TUI never stops it; the TUI/CLI/MCP are clients over the local IPC channel.
- **Autostart at user logon, no admin rights**, per OS, with *no external tool shell-outs* (D-19):
  Windows — per-user Run registry key or Task Scheduler via its API (decide in a decision; Startup folder
  shortcut is the fallback); macOS — a `LaunchAgent` plist in `~/Library/LaunchAgents`; Linux — a
  `systemd --user` unit, XDG autostart as fallback. `service install` is idempotent and reversible;
  `service uninstall` leaves config/history untouched unless `--purge`.
- Optional **tray icon** with status (idle/running/failing), "open TUI", "pause", "run now" — implement if a
  small cross-platform crate makes it low-risk; otherwise record the decision. Not required.
- Single instance guard; crash-safe state (`~/.impresario/service/state.json`, append-only
  `events.jsonl`); watchdog heartbeat; survives sleep/wake and clock/DST changes; log rotation; version
  handshake between client and resident process (restart on upgrade).
- All device access, keychain secrets and signing identities work because it runs **as the user**.

### 8.2 Scheduler
Config (global or workspace; editable from CLI/TUI with the TASK-2 wizard UX; lossless edits):
```yaml
schedules:
  - name: nightly-smoke
    when: { kind: daily, at: "02:30" }                 # host machine local time
    # other kinds: once { at: "2026-11-01T09:00" } | weekly { days: [mon, thu], at: "08:00" }
    #              | interval { every: 6h } | cron { expr: "0 2 * * 1-5" }
    run: { plays: [login, plays/streaming/*.play], tags: [smoke], exclude_tags: [wip],
           actors: [tv1, phone2, "@tvs"], app: shop, retries: 1, profile: smoke }
    on_missed: skip            # skip | run_once   (host was off/asleep at the scheduled time)
    on_busy: queue             # queue | skip | fail   (device leased by another job)
    notify: { on: [failure, unverifiable, device_unreachable], channels: [toast, "webhook:ops"] }
    enabled: true
```
Commands: `schedule list|show|add|edit|rm|enable|disable|run-now|next` (prints the next N fire times).
`once` schedules disable themselves after firing (kept in history). Every fire creates a normal **job**
(visible in `jobs`, reports under `reports/`, tagged with the schedule name). Missed-run logic, DST
transitions (ambiguous/non-existent local times) and overlapping fires of the same schedule are specified,
decided and tested with a fake clock.

### 8.3 Always-on runner ("watchers")
```yaml
watchers:
  - name: netflix-soak
    run: { plays: [streaming_soak], actors: [tv1], app: netflix_like, retries: 0 }
    pacing: { mode: back_to_back }                     # or { mode: min_gap, gap: 30s }  (configurable)
    priority: low                                      # yields the device to manual/scheduled jobs
    on_failure:
      dump: [report, screenshot, logs, device_state, evidence]   # evidence = probe evidence bundle
      throttle: { same_failure_window: 15m }
      notify: [toast, "webhook:ops"]
    device_health: { treat_unreachable_as: infrastructure, backoff: [10s, 30s, 2m, 10m] }
    retention: { max_reports: 500, max_age: 14d, max_disk: 10GiB }
```
- A watcher loops its selection forever until paused/stopped. Each iteration is a job; the device lease is
  held **per iteration** and released between iterations, so manual/scheduled jobs interleave (default;
  `priority: normal|low`, see §13). `watch list|show|add|edit|rm|start|stop|pause|resume|status|dumps`.
- **Failure handling**: on a failed/unverifiable iteration write a dump bundle
  (`<workspace>/reports/watch/<name>/<timestamp>/`: `report.json`, JUnit, failing screenshots, log tails,
  device/agent state, probe evidence) — never raw DRM/license payloads (§1.1). Deduplicate identical
  failures (same act/stage/step/code signature) inside the throttle window (counter increments, one
  notification). **Infrastructure failures** (device unreachable, transport, agent frozen) are classified
  separately from **test failures** and use the backoff list; recovery is notified once.
- **Notifications**: OS-native toast (Windows toast, macOS notification, Linux freedesktop) with title,
  watcher/schedule, failure signature, and an action that opens the report; plus a generic **webhook**
  channel (JSON POST, secrets from keychain). Quiet hours and per-channel rate limits configurable.
  Clicking/`impresario notify open <id>` opens the report in the TUI/HTML.
- Resource hygiene: bounded memory over days, no handle/socket leaks across iterations (soak test with the
  mock agent for ≥ 24 h of simulated iterations in CI via fake time), retention enforcement, automatic
  agent re-establishment (I-19/D-12 semantics).

### 8.4 Clients
TUI "Resident" screen (schedules, watchers, next fires, live iteration, last dumps, health); CLI as above;
MCP tools (§9). `status --json` for scripting. Everything works with the TUI closed.

---

## 9. DSL and tooling changes

### 9.1 Actors (D-29)
```
Play: Streaming Journey
App: shop
Actors: tv1, phone2, chrome_dev, @tvs          // devices and groups; absent = the devices selected for the run

Act: Watch a title
Tags: smoke, streaming
Actors: tv1, phone2                              // optional act-level override of the play list
    Perform: Login(user = '${var.USER}', password = '${env.TEST_PASSWORD}')
    Perform For [tv1]: OpenTitleWithRemote(title = 'Demo')
    Perform Except [tv1]: OpenTitleWithTouch(title = 'Demo')
    Perform: PlaybackStarted
```
- `Actors:` entries are **device names** (global or workspace) or **groups** `@name` defined under
  `groups:` in config (editable from CLI/TUI; nested groups allowed, cycles rejected). Resolution:
  actor → device → platform; the platform drives support checks, selector columns/fallback (D-07),
  audience matching (D-31) and the probe matrix. `--device`/`--actor` on the CLI **narrows** the resolved
  set (must be a subset, else exit 2). Unknown actor/group: `IMP-DSL-04x` with did-you-mean from the
  configured devices.
- `Perform For [a, b]: Stage(args)` runs the step **only** for those actors (⊆ effective actors);
  `Perform Except [a, b]: Stage(args)` runs it for all effective actors **except** those. `For` and
  `Except` cannot be combined on one line. An actor named in `For`/`Except` that is not among the
  effective actors of the act is an error. Square brackets are the list delimiter; items are device names
  or `@groups`. Inside theatre stages, nested `Perform` supports the same forms (actor names there refer
  to the **running** actors of the calling act).
- Reports show, per actor, which Performs ran/were skipped by `For/Except` (`skipped` with the reason).

### 9.2 Audience and Perform-as-interface (D-30, D-31)
```
Theatre: Playback
Audience: android, androidtv, web, webos, tizen
Tags: playback

Stage: PlaybackStarted
    Scene: PlayerScreen
        Check: Showing => VideoContainer timeout 30s
        Check: Player => VideoContainer Playing with position>2s

Theatre: PlaybackApple
Audience: ios, tvos

Stage: PlaybackStarted
    Scene: PlayerScreen
        Check: Showing => VideoContainer timeout 30s
        Check: Player => VideoContainer Playing
```
- `Audience:` is required in every `.theatre` file (platform names; `cross-platform` = all seven).
  A stage may narrow it with its own `Audience:` line (subset of the file's). Matching is **exact on the
  device platform** (D-31).
- Uniqueness: `(stage name, audience platform)` — duplicates are reported on **every** occurrence with
  related locations (`IMP-DSL-040`). All implementations of a name must have the same parameter list
  (names, order, defaults presence) — mismatch `IMP-DSL-041`. Theatre names stay unique workspace-wide.
- **Coverage validation** (`validate`, planner, LSP, MCP): for every `Perform` (including nested ones),
  for every effective actor after `For/Except`, there must be exactly one implementation whose audience
  contains the actor's device platform. Missing → **error** `IMP-DSL-042` naming the stage, the actor, the
  device platform, the act and play location, and offering the two remedies in the message and as code
  actions: *implement the missing stage for audience X* or *remove the actor from this Perform
  (`Except [actor]` / `For [...]`)* or from the act's Actors. Zero implementations anywhere →
  `IMP-DSL-031` (unknown stage). Overlap at call time cannot occur (guaranteed by uniqueness).
- Scenes/pieces keep their own per-platform columns and `IMP-DSL-012/017` rules; stage `Platforms:` from
  TASK-2 §8.3 is removed (superseded by `Audience`).
- Planner expansion: play → acts → example rows → effective actors → for each actor, stage resolution by
  `(name, platform)` → per-device command list with the source-span chain (`in stage X (Playback.theatre:12)
  called from act Y at line N`).

### 9.3 Tags (collected across all files)
- Tags may be declared on **plays, acts, theatres and stages** (`Tags:` line directly under the header).
  Effective tags of an act = play tags ∪ act tags ∪ tags of every theatre/stage it performs (transitive).
  (Propagation of theatre/stage tags to acts — confirm, §13.)
- A single workspace-wide **tag index** feeds `--tag/--exclude-tag` (match on effective tags),
  `impresario tags [--json]` (tag → count, defining files, usages), TUI filters, schedule/watch selections,
  MCP, LSP completion and "find references", and duplicate/typo lints (`IMP-DSL-043` near-duplicate tag
  such as `smoke` vs `smoke-test`, warning). Tags are `[a-z0-9_-]+`; case-insensitive duplicates warn.

### 9.4 Run by file name
`impresario run login`, `impresario run login.play`, `impresario run plays/auth/login.play` all work
(also several, mixed with globs/dirs). A bare name resolves against the plays directory tree by file stem
(extension optional, case-sensitive on Linux/macOS, insensitive on Windows to match the FS); multiple
matches → exit 2 listing the paths; no match → exit 2 with did-you-mean. Optional `#` act filter:
`login#"Successful login"`. Same resolution in TUI, schedules/watchers (`plays:`), profiles and MCP.
Only `.play` files are runnable (`.scene/.theatre` give a clear error).

### 9.5 Grammar, core, planner, reports
Update `docs/DSL.md` (EBNF: `Actors`, `Audience`, `Perform [For|Except] [list]: Stage(args)`, `showing-check`,
`player-check`, `with`, `not`), scanner/parser/AST (all new nodes keep spans; list items and `with`
assertions carry their own spans), validation, planner, formatter (`fmt` normalises `Perform For [a, b]:`
spacing and `with` lists; idempotent), reports (actor → act → stage → step; probe evidence; `skipped by
For/Except`; `unverifiable` status), `docs/ERRORS.md` new families `IMP-DSL-040…`, `IMP-PROBE-*`,
`IMP-SVC-*`, `IMP-ACTOR-*`. Everything generated from tables (probe matrix, vocabularies) feeds PARITY,
the LSP, the extension and the docs from **one** core source.

### 9.6 `impresario migrate` (extends TASK-2 §8.9)
`Platforms:` on plays/acts → `Actors:` by expanding to every configured device of those platforms (listing
the expansion in the output; a `groups:` entry is generated per platform set when it reduces repetition);
theatre files get an `Audience:` computed from the scenes/steps they use (or ask interactively if
ambiguous); duplicate stage names across theatres that become valid interface implementations are left
as is; everything else per TASK-2. `--dry-run`, `.bak`, idempotent, golden tests.

---

## 10. LSP, standalone extension, MCP, TUI (all must be updated and stay in parity)

**LSP and standalone (same feature list, one corpus — TASK-2 §9):**
- Completion: `Actors:` (devices, groups, from config incl. global), `Audience:` (platforms), `Perform` /
  `Perform For [` / `Perform Except [` (actors of the act), stage names filtered to those **implemented for
  every effective actor** (with a marker for partially implemented stages), argument names/defaults,
  `Check:` forms (`Showing`, `Player`), statuses, DRM providers (only those supported on the effective
  platforms), `with` properties filtered by platform and their value kinds/units/enums, `not`, tag names
  (workspace-wide), file names for `run` in config/schedule files.
- Diagnostics: everything in §9 live, per effective actor; duplicates of anything that must be unique
  (stage+audience, theatre, scene, piece, act, param, actor entries, tags near-duplicates) on **every**
  occurrence with related information; per-platform probe support (`IMP-DSL-013`); coverage errors with
  quick-fixes (*create stage implementation for audience X* scaffolds the file/stage with the interface
  signature, *add `Except [actor]`*, *remove actor from act*).
- Hover: actor → device → platform and which stage implementation each actor resolves to; check support
  per platform with the **evidence class and expected confidence** (from the probe matrix); stage
  signature + all implementations + their audiences.
- Go-to-definition/references/rename across implementations (renaming a stage renames *all* its
  implementations), actor/group rename in config and files, semantic tokens, folding, symbols, code lens
  ("Run on tv1", "N implementations", "audience coverage: 4/5 platforms"), inlay hints (resolved actors
  per Perform), signature help, formatting/range/on-type.
- Standalone mode keeps the TASK-2 §9.2 architecture (preferably the same Rust core in WASM); the shared
  corpus `fixtures/lang-corpus/` grows by cases for every new rule and CI fails on divergence between
  Rust, LSP and extension results. The extension reads `config`/`devices`/`groups` through the VS Code
  file API in standalone mode.

**MCP** (`impresario mcp`, I-80): `workspace_validate`/`run`/`report_get` understand actors, audiences and
the new checks (diagnostics include coverage errors); new tools `probe_inspect` (live evidence for a piece
on a device, bounded), `tags_list`, `schedules_list|add|remove|run_now`, `watchers_list|start|stop|dumps`,
`service_status`, `jobs_*`; artifacts include evidence bundles and dump bundles; new resources
`impresario://docs/PROBES`, `…/DSL`, `…/SCHEDULING`; all mutating tools keep the process-wide lock
discipline and never expose secrets. `tests/mcp_stdio.rs` grows end-to-end cases (mock-driven).

**TUI**: Probe screen, Resident screen, actor/group editor, theatre coverage matrix view
(stages × platforms), run view shows probe evidence and `unverifiable` states.

---

## 11. Phasing (each phase ends green, documented, decisions recorded)

| Phase | Content | Exit criteria |
|---|---|---|
| **0** | **Spikes** → `docs/SPIKES.md`, `docs/PROBES.md` signal matrices: (a) Android signal inventory per API level on A51 / TCL TV / Realme with a clear stream, Widevine test stream and a real Netflix-class app (user supplies, §13); (b) Apple channels 1–7 of §5.3 per iOS/tvOS version (hardware needed; without it: protocol-level feasibility + recorded traces + verdict "unverified"); (c) EME/MSE/AVPlay hook feasibility on old TV Chromium builds (fixture + any available TV); (d) autostart + toast + IPC per OS (Windows first) | Written verdict per signal: available / redacted / needs consent / impossible; chosen confidence per claim; new decisions; plan adjusted |
| **1** | `impresario-core::probe` (evidence, verdicts, vocabularies, matrix), proto + capabilities + `MockAgent`, report/schema changes, policy `unverifiable` | Exhaustive verdict unit tests; PARITY probe matrix generated |
| **2** | DSL: Actors/groups, Audience, Perform interface model, For/Except, Showing/Player/`with`/`not`, tags index, run-by-name, formatter, validation, migrate | Docs + golden corpus green; mock-driven E2E |
| **3** | Connector probes (web first on headless Chrome with fixtures, then TV-quirk handling) | Fixture suite green on real Chrome; EME ClearKey + Widevine fixtures |
| **4** | Android agent probes (`:probe-core`, collectors, stub scenarios) | JVM tests + on-device suites on A51/TV; live E2E with Netflix-class app per §13 |
| **5** | Resident runner: service install/logon autostart (Windows verified live), scheduler, watchers, notifications, dumps, retention | Fake-clock suites; live: close TUI → schedule fires → toast + report; watcher failure dump |
| **6** | LSP + standalone extension + MCP + TUI updates | Parity gate in CI; MCP e2e |
| **7** | Apple probes (host-assisted collectors via `idevice`, runner adapters), webOS/Tizen platform-level collectors | Replay/fake-source tests green; STATUS honest about unverified hardware |
| **8** | Docs sweep (`DSL`, `PROBES`, `SCHEDULING`, `PROTOCOL`, `PARITY`, `ERRORS`, `DEVICES`, `STATUS`, `README`, `IMPRESARIO.md`), release packaging | Definition of done |

Phases 3/4 can run in parallel; 5 can start after 1. Reorder only with a recorded reason.

## 12. Definition of done
1. A play can assert, on Android/Android TV (live) and web (live, headless Chrome), that an image is really
   showing (and fails on a 404'd placeholder), that a video is `Playing`/`Paused` with properties, and that
   DRM is enabled with the right provider — with evidence in the report and correct tri-state verdicts.
2. On Apple, the same checks run with the best legitimately reachable confidence; every claim that cannot be
   verified black-box reports **UNVERIFIABLE** with a reason, and `docs/PROBES.md` states per OS version
   what is possible. No jailbreak/private API anywhere.
3. A Netflix-class app (user-provided account/device) passes a streaming journey end to end driven only by
   impresario (UI journey + player + DRM claims), or the report/STATUS explain exactly which claims were
   unverifiable on which platform and why.
4. The resident runner survives closing the TUI/CLI: a schedule (once, daily, weekly, interval, cron) fires,
   a watcher loops with configurable pacing, failures dump bundles and raise OS toasts/webhooks, verified on
   Windows live; macOS/Linux autostart verified by unit/replay tests and flagged unverified if not run.
5. Actors/Audience/Perform-interface/For-Except/tags/run-by-name behave exactly as §9 in CLI, TUI, MCP, LSP
   and the standalone extension (shared corpus green, duplicates detected live).
6. All gates green (Rust fmt/clippy/test, Android JVM + instrumented where hardware exists, `swift test`,
   extension tests, corpus parity, `parity_doc`), decisions/STATUS/ERRORS/THIRD_PARTY/SPIKES/PROBES updated.

## 13. Open items — the agent must ask the user (never assume), before the named work starts
1. **Scenes' `Platforms:` header** (Phase 2): the rename to `Actors` targets plays/acts. Scenes are
   keyed by platform columns; should the scene-level `Platforms:` header be removed (columns decide
   applicability), kept as platform names, or something else?
2. **Tag propagation** (Phase 2): confirm that tags on theatres/stages propagate to the acts that perform
   them (this spec's default), or only play→act inheritance + aggregation.
3. **Audience granularity** (Phase 2): file-level `Audience:` with optional per-stage narrowing (default),
   or file-level only.
4. **`Showing … with` options** (Phase 2): confirm the minimal set `coverage`, `decoded`, `source`.
5. **Unverifiable default** (Phase 1): `fail` (default here) vs `warn`.
6. **Watcher priority** (Phase 5): per-iteration lease with `priority: low` yielding to manual/scheduled
   jobs (default) vs exclusive hold of the device.
7. **Acceptance material** (Phase 0/4/7): which real streaming app(s) and test account(s) on which devices;
   whether DRM test content with a Widevine license proxy is acceptable; FairPlay test asset + certificate
   availability; availability of an iPhone/Apple TV + Mac for Phase 0(b)/7; confirmation that the optional
   Android VPN-based network tap (user consent dialog) is acceptable to ship (default off).
8. **In-app hook SDKs** (Phase 4/7): contract + reference fixtures only (default) or full SDKs for the
   user's own apps (Android/Swift/JS)? Includes whether to wire it into the user's own streaming app.
9. **Tray icon** (Phase 5): required, optional, or skipped.
