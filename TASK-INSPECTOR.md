# TASK-INSPECTOR.md — Impresario Element Inspector

> Audience: the coding agent implementing this feature.
> Companion documents: `TASK.md`, `TASK-2.md`, `TASK-3.md`. This document **extends** them and must not contradict them.
> Status: spec v1. Anything marked **[ASSUMPTION]** must be verified against the real repository in Phase 0 and corrected here before building on it.

---

## 0. Ground rules for the agent (read first)

1. **Never assume. Ask.** If the repository, the DSL, or an existing spec contradicts this document, stop that task, write the contradiction into `INSPECTOR-NOTES.md` (see §0.4), and ask the owner. Do not silently pick a side.
2. **Additive changes only.** Nothing that exists today (CLI commands, gRPC RPCs, proto fields, DSL syntax, scene/theatre/play files, config files, agent behavior, MCP tools) may change behavior or break. Proto changes are append-only (new fields, new RPCs, new messages). Enforce with `buf breaking` (or equivalent) in CI.
3. **One resolver.** The locator engine used by the inspector must be the *same code* the test runner uses at run time. A locator the inspector writes into a scene must resolve identically when the scene is executed. No second implementation.
4. **Working log.** Keep `INSPECTOR-NOTES.md` at repo root: decisions, spike results, benchmark numbers, open questions, deviations from this spec. Update it as you go.
5. **Legitimate flows only.** No jailbreak/root requirement. Developer mode, debug ports, developer-signed runners, accessibility APIs and in-app opt-in packages only.
6. **No external tool dependencies** (adb, Xcode tools, ares, sdb, tizen CLI), consistent with TASK-2. Protocol clients are implemented in Rust.
7. **Performance is a feature with budgets** (§5.6). A task is not done if its budget is not measured and met, or the miss is documented in `INSPECTOR-NOTES.md` with the owner's acknowledgement.
8. **Every task ends with tests.** No task is complete without the tests named in its acceptance criteria, passing in CI.

### 0.1 Terminology

- **master**: **[ASSUMPTION]** the main long-lived impresario host process that owns device connections, the scheduler/resident runner, and the gRPC server. Confirm the real name and boundaries in Phase 0.
- **agents**: **[ASSUMPTION]** the per-platform driver components that talk to a device/app (host-side workers and on-device helper apps such as the Android helper APK, the Apple XCTest runner, the injected web script). Confirm in Phase 0 and use the repo's real names.
- **actor / platform**: per TASK-3, "Platforms" were replaced by "Actors". The scene table columns are keyed per the *current* DSL spec. Use the real DSL term; this document says **column** for "the thing a locator cell is filed under".
- **view / viewport**: what is currently visible on screen to the user (§5.1).
- **snapshot**: a normalized, viewport-only element tree at a revision `rev`.
- **locator**: a Playwright-style selector expression (§7) stored in a scene cell.

### 0.2 Scope

In scope: Web UI (dedicated port), MCP frontend, a shared inspector core, per-platform acquisition for web / webOS / Tizen / Android / Android TV / iOS / tvOS, Flutter support, locator engine and synthesis, scene authoring from the inspector, master/agent changes, tests and benchmarks.

Out of scope: a TUI inspector (explicitly not wanted), recording/replaying flows, visual regression, replacing Appium for general use, anything requiring jailbreak/root.

### 0.3 Requirements traceability

| # | Requirement | Where |
|---|---|---|
| 1 | Web UI (dedicated port) + MCP only; no TUI | §3, §10, §11 |
| 2 | Bidirectional connection, instant feedback | §4.2, §6 |
| 3 | Current view only | §5 |
| 4 | Auto-update on scroll/navigation/any change | §6 |
| 5 | Select → add to `.scene` table, auto-pick column | §9 |
| 6 | Much faster than Appium Inspector (esp. Apple) | §5, §15 |
| 7 | Graceful selection + tree parsing | §13 |
| 8 | Flutter | §8 |
| 9 | Playwright-style locators | §7 |
| 10 | Master changes | §12 |
| 11 | Master-embedded vs dedicated, gRPC | §3 |
| 12 | Agents aligned, nothing broken | §12 |

### 0.4 Deliverables checklist

- `crates/impresario-inspector-core` (or the repo's naming convention), `impresario-locator` (shared with the runner), `impresario-inspector-web` (server + embedded SPA), `impresario-inspector-mcp`, proto additions, per-agent changes, Flutter Dart package, docs, tests, benchmarks, `INSPECTOR-NOTES.md`.

---

## 1. Why Appium Inspector is slow (and what we do differently)

Understand this before designing anything; every item below maps to a design rule.

1. **Whole-tree snapshots.** It requests `/source` for the entire app, including off-screen and invisible nodes, then ships a giant XML/JSON blob and re-parses it. → We acquire **viewport-pruned** trees only (§5).
2. **XCUITest cost model.** On Apple platforms a snapshot walks the whole accessibility hierarchy over IPC with a wide attribute set and large `maxDepth`. Cost grows with tree size, and "is visible"/"is accessible" are often queried per element. → We use a **custom runner** with a minimal attribute set, bounded depth, **lazy subtree expansion limited to on-screen children**, and parallel fetch (§5.4).
3. **Polling and request/response.** Refresh is a manual action or timed polling. → We use **push events** and a **bidirectional stream** (§6).
4. **XML + XPath.** Locators generated from XPath are slow and brittle. → Role/name/text/id locators (§7), no XPath generation.
5. **Full redraw.** Each refresh replaces the whole tree. → **Delta updates** with stable node ids (§4.4).
6. **Coupled screenshot and tree.** → Independent streams; overlay is drawn client-side from coordinates, never baked into pixels.
7. **Idle cost.** → Zero acquisition work when no subscriber is attached.

---

## 2. Design principles

- **Viewport-first, event-driven, delta-based, lazily-expanded.**
- **One normalized model, raw data preserved.** Normalize for locators/UI; keep the original platform attributes untouched in `raw`.
- **Degrade, never crash.** Partial trees are valid output; every parser is tolerant (§13).
- **Server owns truth.** Frontends are thin; all logic lives in the core so Web UI and MCP behave identically.
- **Shared device sessions.** The inspector must not open a second competing connection to a device that the master already holds.

---

## 3. Architecture decision (requirement 11)

### 3.1 Decision

**The inspector core lives inside the master process (same executable), exposes a gRPC service, and the frontends are thin clients of that service.**

- `impresario-inspector-core`: a library crate linked into the master. Owns subscriptions, snapshot pipeline, locator synthesis calls, scene editing, leases.
- gRPC service `impresario.inspector.v1.Inspector` served by the master (tonic) on the master's existing gRPC endpoint (or an additional listener if the existing one is unsuitable; document the choice).
- **Web UI**: HTTP + WebSocket server on a **dedicated port**, started by the master when enabled (`impresario inspector` opens it / `inspector.enabled = true` in config). Serves the embedded SPA and a WebSocket gateway that carries the same protobuf messages as the gRPC bidi stream. (Browsers cannot do gRPC bidi streams directly; do not use grpc-web streaming.)
- **MCP**: `impresario mcp` (existing MCP entry point if one exists — extend it) acts as a **gRPC client** of the resident master. It never talks to devices directly.

### 3.2 Rationale (record in `INSPECTOR-NOTES.md`, challenge it if the repo disagrees)

- The master already owns device connections, signing, and scheduling. A separate inspector process would need its own device sessions, causing contention with runs and double work on USB/ADB/RSD connections.
- Frontends being gRPC clients keeps a single API contract and allows out-of-process frontends (MCP stdio shim, future tools) without touching the core.
- Single executable keeps the "no external dependencies" property.

If Phase 0 reveals that the master cannot host this (for example, a hard process-isolation requirement), the fallback is a dedicated `impresario-inspector` binary that connects to the master over gRPC and obtains **device session leases** from it (never opening its own device connection). Document and ask before taking that path.

### 3.3 Components

```
 Web UI (browser) ──WS(protobuf)──┐
                                  ▼
 MCP client ──stdio/HTTP──▶ impresario mcp ──gRPC──▶  MASTER
                                                      ├─ inspector-core
                                                      │    ├─ subscription manager
                                                      │    ├─ snapshot pipeline (normalize, prune, diff)
                                                      │    ├─ locator engine (shared with runner)
                                                      │    ├─ scene editor (via DSL crate)
                                                      │    └─ lease/priority manager
                                                      ├─ device session manager (existing)
                                                      ├─ scheduler / resident runner (existing)
                                                      └─ agent channel(s) (existing, extended)
                                                            └─ agents: web/CDP, android, apple, flutter-bridge
```

---

## 4. Contracts

### 4.1 Proto additions (append-only; adapt names to repo conventions)

```proto
syntax = "proto3";
package impresario.inspector.v1;

service Inspector {
  // Primary: long-lived bidirectional stream. One per frontend connection.
  rpc Session(stream ClientEvent) returns (stream ServerEvent);

  // Unary conveniences (used by MCP one-shots and tests).
  rpc ListDevices(ListDevicesRequest) returns (ListDevicesResponse);
  rpc GetSnapshot(GetSnapshotRequest) returns (Snapshot);
  rpc Resolve(ResolveRequest) returns (ResolveResponse);
  rpc SuggestLocators(SuggestRequest) returns (SuggestResponse);
  rpc ListScenes(ListScenesRequest) returns (ListScenesResponse);
  rpc ApplySceneEdit(SceneEditRequest) returns (SceneEditResponse);
}

message ClientEvent {
  uint64 client_seq = 1;
  oneof kind {
    Attach attach = 10;              // device + options, acquires a lease
    Detach detach = 11;
    SetOptions set_options = 12;     // include_system_ui, min_visible_fraction, fps cap, screenshot on/off/scale
    Freeze freeze = 13;              // pause live updates; keep last snapshot+frame
    Unfreeze unfreeze = 14;
    Refresh refresh = 15;            // force full re-acquire
    ResyncRequest resync = 16;       // client detected rev gap
    Input input = 20;                // tap/swipe/scroll/key/dpad/text (gated by lease mode)
    Highlight highlight = 21;        // ask device-side highlight where supported
    Select select = 30;              // select node ids; server tracks selection
    ResolveLocator resolve = 31;
    SuggestForSelection suggest = 32;
    SceneEdit scene_edit = 40;
    Ping ping = 90;
  }
}

message ServerEvent {
  uint64 server_seq = 1;
  oneof kind {
    Attached attached = 10;          // capabilities, screen info, lease mode
    Snapshot snapshot = 11;          // full
    SnapshotDelta delta = 12;        // incremental
    Frame frame = 13;                // screenshot frame (jpeg/webp), screen-relative
    SelectionState selection = 20;   // alive / stale / offscreen + re-resolved node
    ResolveResult resolve_result = 21;
    Suggestions suggestions = 22;
    SceneEditResult scene_result = 30;
    SceneChanged scene_changed = 31; // file changed on disk
    LeaseEvent lease = 40;           // preempted by run, read-only, restored
    Degraded degraded = 41;          // partial tree, timeouts, slow path
    Error error = 90;
    Pong pong = 91;
  }
}
```

Required messages (define fully): `Snapshot{rev, device_id, screen: ScreenInfo, nodes: Node[] (flat, parent_id refs), degraded[], stats}`, `SnapshotDelta{base_rev, rev, ops: Op[] (Add/Remove/Update/Move)}`, `ScreenInfo{logical_w, logical_h, pixel_w, pixel_h, scale, rotation, safe_insets, focus_model: POINTER|DPAD|BOTH}`, `Frame{rev_hint, w, h, mime, bytes}`.

### 4.2 Bidirectional behavior (requirement 2)

- All traffic for a connected frontend flows over one stream: server pushes snapshots/deltas/frames as soon as they are produced; client sends input/selection/edits at any time and receives the result on the same stream.
- Every server event carries `server_seq`; deltas carry `base_rev`. If the client sees a gap or mismatch it sends `resync` and the server replies with a full `Snapshot`.
- **Backpressure / coalescing**: if a client is slow, the server drops intermediate snapshots/frames and sends the latest (always keep deltas consistent by sending a full snapshot after drops). Never queue unboundedly.
- **Keepalive**: ping/pong every 10 s; detach and release lease on timeout.
- **Input echo**: after an `Input`, trigger an immediate high-priority re-acquire (do not wait for the next event/poll tick).

### 4.3 Normalized `Node`

```
id            u64   stable across snapshots for the same underlying element (§4.4)
parent_id     u64   0 for root
role          enum  normalized role (§7.2); `generic` if unknown
name          str   accessible name (§7.3)
text          str   visible text (truncated; `text_truncated` flag)
value         str
description   str
test_id       str   explicit automation id (testid / resource-id / accessibilityIdentifier / Semantics.identifier / Key)
bounds        rect  logical units, screen space (§4.5)
visible_fraction f32 0..1 of the node's area inside the viewport after clipping
state         { enabled, focusable, focused, selected, checked(tri), expanded(tri), pressed(tri), disabled, readonly, required, hidden_by_a11y }
scroll        { can_scroll_forward, can_scroll_backward, axis }   // only on scroll containers
context       enum  NATIVE | WEBVIEW | FLUTTER | WEB
source_path   str   platform path, informational only
raw           map<str,str>  original attributes (size-capped)
children_hidden_count u32   offscreen children pruned beneath this node (hint, see §5.2)
```

### 4.4 Stable ids

- Web: injected script keeps a `WeakMap<Element,id>`; ids are never reused within a session.
- Android: hash(window id, `viewIdResourceName`, class, sibling-index path, bounds-bucketed) with collision handling; prefer `AccessibilityNodeInfo` identity where the helper keeps nodes alive.
- iOS/tvOS: hash(element type, identifier, label, parent id, sibling ordinal among same type/label).
- Flutter: `SemanticsNode.id`.
- Rule: if an element persists across a scroll or minor update, its id must not change, so that selection survives (§13.3). Provide a property test for id stability under pure scroll/translation.

### 4.5 Coordinate spaces

Maintain explicitly (do not mix): **logical** (points / CSS px / dp), **device pixels**, **screenshot pixels**. `ScreenInfo.scale` converts. All `Node.bounds` are logical, screen-relative, rotation-applied. Frames carry their pixel size so the UI maps boxes correctly. Handle: Android density, iOS points vs pixels, web `devicePixelRatio` and page zoom, webOS/Tizen apps rendered at 1280×720 or 1920×1080 CSS space and scaled, rotation, split-screen/multi-window.

---

## 5. Viewport-only acquisition engine (requirements 3 and 6)

### 5.1 Definition of "in view"

A node is **in view** if its bounds, **clipped by every ancestor clip region** (overflow hidden/scroll containers, window bounds, parent clips), intersect the viewport with area ≥ `min_visible_fraction` (default: any non-zero intersection; configurable, expose in options).

Always include regardless of visibility fraction:
- the **focused** element (TV/DPAD platforms and keyboard focus) and its ancestor chain,
- ancestors of any included node (needed to keep the tree connected; mark them `visible_fraction` as computed).

Exclude by default (option-controlled): opacity 0, `visibility:hidden`, `display:none`, a11y-hidden (`aria-hidden`, `importantForAccessibility=no-hide-descendants`, iOS `accessibilityElementsHidden`), zero-size nodes, nodes occluded by a fully covering modal layer (web `inert`, Android other window on top, iOS modal presentation). System UI (status bar, nav bar, keyboard) excluded unless `include_system_ui`.

### 5.2 Hints for what is not shown

Pruned content must not be invisible to the user or agent: scroll containers carry `scroll.can_scroll_*` and `children_hidden_count`, so the UI shows "more content below (N items)" and MCP can `scroll` then re-read. This resolves the tension between "current view only" and authoring elements that need scrolling.

### 5.3 Pruning rule (all platforms)

Prune **at acquisition time inside the agent/runner**, not after transport. Never ship off-screen nodes over the wire. During traversal, if a node's clipped bounds do not intersect the viewport, skip its entire subtree (increment parent's `children_hidden_count`), except where the platform lets descendants escape the parent clip (web `position:fixed`, transforms, overlays; Android `clipChildren=false`; Flutter overlays). Handle those cases explicitly and test them.

### 5.4 Per-platform acquisition

**Web, webOS, Tizen (CDP)**
- Primary: an **injected script** (installed via `Page.addScriptToEvaluateOnNewDocument` and evaluated in the current document) walking the DOM with pruning: `getBoundingClientRect` + ancestor clip intersection, skipping subtrees fully outside, handling `position:fixed/sticky`, transforms, shadow DOM (open roots), same-origin iframes, and `<video>`/`<canvas>` as opaque nodes. Compute role/name per §7.3 in-script to avoid round trips.
- Do **not** use `DOM.getDocument` full-tree or `Accessibility.getFullAXTree` as the default path (full-tree cost). Optionally use `Accessibility.queryAXTree`/partial AX for name computation verification in tests.
- Old webOS engines (WebKit / early Chromium) may lack CDP domains: script must be ES5-safe and the driver must feature-detect and fall back to polling (§6.3). Record the minimum supported engine per target in notes.
- Tizen/webOS access goes through the repo's native SSH/SDB implementations (TASK-2). Dev-mode/inspectable build required; if the app is not inspectable, return a `Degraded{reason: NOT_INSPECTABLE}` and offer the visual fallback (§13.5).
- Screenshots: `Page.startScreencast` (event-driven frames) where available; else `Page.captureScreenshot` on change.

**Android / Android TV**
- On-device **helper** (instrumentation APK, launched without adb binaries via the native ADB client in Rust). Uses `UiAutomation` / `AccessibilityService`-class APIs to read `AccessibilityNodeInfo`.
- Traverse from the active window root(s) with pruning on `getBoundsInScreen` ∩ screen and `isVisibleToUser()`; skip subtrees early. Configure prefetch flags to reduce IPC; avoid `refresh()` per node. Request only needed fields.
- Windows: foreground app window by default; optional dialogs/popups of the same app; system windows only when `include_system_ui`.
- **Never** use `uiautomator dump` in the fast path (full dump, slow, flaky). It may exist only as an emergency fallback.
- Events: `TYPE_WINDOW_CONTENT_CHANGED`, `TYPE_WINDOW_STATE_CHANGED`, `TYPE_VIEW_SCROLLED`, `TYPE_VIEW_FOCUSED`, `TYPE_VIEW_ACCESSIBILITY_FOCUSED`, `TYPE_WINDOWS_CHANGED` (§6.2).
- Android TV: focus (`isFocused`, input focus) is first-class; always include the focused node and its chain; expose D-pad navigation helpers (§10.5).
- Compose: respect `testTagsAsResourceId`; document the one-line app setup in the docs. Map semantics roles.
- WebViews: attach CDP via the `webview_devtools_remote_<pid>` socket (forwarded by the native ADB client) and merge the DOM subtree under the WebView node with `context=WEBVIEW`. Require debuggable WebViews; degrade gracefully otherwise.
- Screenshots: `UiAutomation.takeScreenshot` encoded on-device (JPEG, scaled) or a low-latency stream; make the transport pluggable and benchmark both.

**iOS / tvOS (the hard one)**
- Custom Apple runner (XCTest-based, derived conceptually from WebDriverAgent but **not** the stock WDA `/source` path), signed with `apple-codesign`, installed/launched through `idevice` (developer image/RSD tunnel as required per iOS version). Do not require macOS/Xcode.
- Speed strategy (validate in Spike S1, §17):
  1. Request snapshots through the lowest-level available XCTest accessibility client API with an **explicit minimal attribute set** (type, identifier, label, value, frame, enabled, focused/selected, hittable-equivalent) and a **small `maxDepth`**.
  2. **Lazy expansion**: snapshot the root at depth 1–2, then fetch subtrees **only for children whose frames intersect the viewport**, in parallel. Skip off-screen subtrees entirely.
  3. Cache stable subtrees by identity and invalidate only on change signals.
  4. Avoid per-element property queries (`isHittable`, `isVisible`, etc.); derive visibility from frames.
  5. Keep the runner process warm; never restart per request; reuse one XCTest session.
- Alternatives to evaluate in the spike and document: the accessibility audit service (`axAuditDaemon` remote server) for hierarchy without XCTest, and a hybrid (audit service for change detection, runner for details).
- tvOS: same runner; focus via the focused-element query as the **fast path** (focused element + siblings/parents first, then fill the visible remainder). Remote key input via the runner's remote-press API.
- Change signals: see §6.4 (Apple has no first-class push event stream; use cheap fingerprinting).
- WKWebView content: Web Inspector protocol via webinspectord, merged like Android WebViews.
- Screenshots: runner-side MJPEG-style stream or `XCUIScreen` capture, scaled; adaptive rate.
- If the spike shows the target budget is unreachable with a given iOS version, document the achieved numbers and ask the owner before shipping a slower mode.

**Flutter**: §8.

### 5.5 Combining native + embedded contexts

A single snapshot may contain `NATIVE`, `WEBVIEW`, `FLUTTER` subtrees. The context boundary node carries a `context` marker; locators can target a context scope (§7.5).

### 5.6 Performance budgets (hard targets, measured by the benchmark harness §15.3)

Measured on a mid-range device of each class, typical app screen (≈500–2,000 total nodes, 50–200 in view):

| Metric | Web/webOS/Tizen | Android/Android TV | iOS | tvOS |
|---|---|---|---|---|
| Change event → UI updated (p50) | ≤ 60 ms | ≤ 120 ms | ≤ 250 ms | ≤ 250 ms |
| Change event → UI updated (p95) | ≤ 150 ms | ≤ 300 ms | ≤ 500 ms | ≤ 500 ms |
| Cold attach → first snapshot | ≤ 1.0 s | ≤ 1.5 s | ≤ 3 s (warm runner), runner install excluded | ≤ 3 s |
| Full viewport snapshot (agent-side) | ≤ 30 ms | ≤ 80 ms | ≤ 200 ms | ≤ 200 ms |
| Delta payload for a scroll tick | < 20 KB typical | < 20 KB | < 20 KB | < 20 KB |
| Idle CPU with a subscriber and static UI | < 2% | < 2% | < 3% | < 3% |
| Idle cost with no subscriber | zero acquisition | zero | zero | zero |

Also record the equivalent numbers for Appium Inspector on the same screens in `INSPECTOR-NOTES.md` for comparison; the goal is a documented large multiple (target ≥ 10× on Apple, ≥ 5× elsewhere).

---

## 6. Live updates (requirement 4)

### 6.1 Principle

Acquisition runs only while ≥ 1 subscriber is attached (lease held). Each device has one acquisition loop shared by all subscribers of that device.

### 6.2 Event sources per platform

- **Web/webOS/Tizen**: in-page `MutationObserver` (childList, attributes, characterData, subtree), `scroll` (capture, passive, all scrollers), `resize`, `focusin/focusout`, `transitionend/animationend`, `popstate/hashchange`, patched `history.pushState/replaceState`, `visibilitychange`, `fullscreenchange`, `<video>` events. Debounce with `requestAnimationFrame`, then notify through `Runtime.addBinding` → `Runtime.bindingCalled` (instant push). Maintain a cheap **change counter**; the host re-acquires on counter change.
- **Android**: accessibility events (§5.4) throttled (≈ 16–50 ms trailing debounce, configurable); the helper pushes a small "dirty" message and the host (or helper) re-acquires. Scroll events must trigger re-acquisition so newly visible items appear.
- **Flutter**: `SemanticsOwner` change notifications (§8).
- **Apple**: see §6.4.

### 6.3 Polling fallback (only when no push source)

Adaptive: fast (≈ 100–150 ms) for ~1 s after any input/user interaction or detected change, decaying to slow (≈ 500–1000 ms) when static. Poll a **cheap fingerprint**, not the tree (e.g., hash of viewport-root children frames + focused element + scroll offsets). Re-acquire only on fingerprint change.

### 6.4 Apple change detection (no push)

Combine: (a) the runner's own cheap fingerprint (frames of top-level visible children, focused element, scroll container content offsets), polled adaptively; (b) low-resolution screenshot hash as a secondary signal (when frames are streamed anyway it is free); (c) immediate refresh after any `Input` issued by the inspector; (d) navigation hints from app-state changes (active app, key window). Document the achieved latency in notes. Investigate in Spike S1 whether accessibility notifications are obtainable without private entitlements; use them if they are legitimate and stable.

### 6.5 Delta generation

Server diffs the previous and new viewport snapshots using stable ids and emits `SnapshotDelta` ops. If the delta would exceed 60% of the full snapshot size, send a full snapshot instead. Always monotonic `rev`.

### 6.6 Settle detection

During animations/transitions, avoid flicker: coalesce updates within a short window (default 50 ms, adaptive), but never delay the **first** update after a quiet period by more than one frame. Expose `settled: bool` on snapshots (true when no changes for ≥ N ms) so scene authoring can prefer settled snapshots.

---

## 7. Locator engine (requirement 9)

Implemented once in `impresario-locator`, used by the test runner and the inspector. Pure function over a snapshot-like tree (the runner may resolve against live acquisition; both go through the same matching code).

### 7.1 Locator kinds (Playwright-style)

| Kind | Meaning | Example (canonical text form) |
|---|---|---|
| `role` | normalized role + optional accessible name and states | `role=button[name="Sign in"][disabled=false]` |
| `text` | visible text (substring default; `exact`, regex) | `text="Welcome"`, `text=/^Hi, .+/` |
| `label` | associated label / accessibility label | `label="Email"` |
| `placeholder` | input placeholder / hint | `placeholder="Search"` |
| `alt` | image alt / content description | `alt="Logo"` |
| `title` | title / tooltip | `title="Close"` |
| `testid` | explicit automation id | `testid=login.submit` |
| `id` | platform native id (resource-id, accessibilityIdentifier, DOM id) | `id=com.app:id/ok` |
| `css` | web only | `css=.btn.primary` |
| `xpath` | web only; never generated by synthesis | accepted only if hand-written |
| `focused` | currently focused element | `focused` |
| `region` | visual fallback (§13.5), last resort | `region=…` |

Modifiers/composition: `>>` chaining (descendant scope), `within(<locator>)` / `has(<locator>)` / `has-not`, `nth=N`, `first`, `last`, `visible` (default on), `context=NATIVE|WEBVIEW|FLUTTER`. Name matching options: `exact`, `i` (case-insensitive), regex. Whitespace-normalized text matching.

**[ASSUMPTION]** The scene DSL already has a locator cell grammar (TASK-2: "scene (tabular locators)"). In Phase 0, read the real grammar and:
- if it already supports these kinds, reuse it verbatim;
- if not, **propose** the minimal additive extension as a DSL change (see §12.3) and get approval before writing any new syntax into scene files. Do not invent a competing syntax.

### 7.2 Role normalization

Single table in code (data-driven, unit-tested), mapping platform types to roles (superset of ARIA roles): `button, link, textbox, searchbox, checkbox, radio, switch, slider, spinbutton, combobox, listbox, option, menu, menuitem, tab, tablist, tabpanel, list, listitem, grid, row, cell, image, heading, text, dialog, alert, progressbar, scrollbar, toolbar, navigation, video, generic`.

- Web: explicit `role` attribute, else implicit ARIA role from tag/type (follow the HTML-AAM mapping).
- Android: `className` + `AccessibilityNodeInfo` role description / collection info (`Button`, `EditText`, `CheckBox`, `Switch`, `SeekBar`, `RecyclerView`→`list`, `ImageView`→`image`, `TextView`→`text`, …).
- iOS/tvOS: `XCUIElementType*` (`Button`, `TextField`, `SecureTextField`, `Switch`, `Slider`, `Table`, `Cell`, `Image`, `StaticText`, `NavigationBar`, `TabBar`, …) plus accessibility traits (header, button, link, selected).
- Flutter: `SemanticsFlag`/action-derived role (§8.4).
- Unknown → `generic`; never throw.

### 7.3 Accessible name computation

Per platform, documented order, implemented in the agent (web) or normalizer (others): label/`aria-labelledby`/`aria-label` → text content → `title`/`alt`/`placeholder` (web); `contentDescription` → `text` → `hint` (Android); `label` → `identifier` is **not** a name (iOS); semantics `label` → `value` (Flutter). Trim, collapse whitespace, cap length (default 200).

### 7.4 Resolution semantics

- Result: ordered list of matches with `match_count`. **Strict mode** for scene execution (exactly one or error); inspector reports counts and highlights all matches.
- Matching is against **in-view** nodes by default. A `scroll_into_view` execution option is a runner concern; the inspector only reports "N matches in view, M possibly off-screen (container hints)".
- Deterministic ordering (document order / spatial order for ties).

### 7.5 Locator synthesis (inspector feature)

Given a selected node and the current snapshot, produce ranked candidates:

1. `testid` / `id` (stable explicit ids) — highest.
2. `role` + `name` (+ states only when needed for uniqueness).
3. `label` / `placeholder` / `alt` / `title`.
4. `text` (reject if it looks dynamic: contains digits patterns like time/price/counters, or is very long).
5. Scoped chains: `within(<stable ancestor>) >> role=…`.
6. `nth` only as a last structural resort, flagged **fragile**.
7. `region` (visual) only when nothing else exists, flagged **fragile**.

For each candidate compute and return: `unique_in_view: bool`, `match_count`, `stability_score 0..100`, `fragile: bool`, `reasons[]`, and **cross-actor availability** (does an equivalent resolve on other attached devices; §9.3). Algorithm: generate → verify against the current snapshot via the real resolver → rank by (unique, stability, brevity). Never emit XPath. Heuristics for dynamic text: numbers/timestamps/UUIDs/`…` truncation, locale-dependent strings (suggest the id-based alternative and mention localization risk, TASK-2 supports EN/AR RTL apps).

Stability scoring is pure and fully unit-tested with golden fixtures.

### 7.6 Resolver parity tests

For every fixture snapshot and every synthesized locator, assert the runner's resolution path returns the same node(s) as the inspector. Block merges on mismatch.

---

## 8. Flutter strategy (requirement 8)

Flutter paints its own pixels; native accessibility trees are empty unless semantics are enabled, and widgets have no native ids. Use a **layered strategy**, auto-detected per app:

### 8.1 Layer A — Semantics tree (preferred; works in release builds)

- Flutter exposes its Semantics tree to the platform accessibility layer (Android `AccessibilityNodeInfo`, iOS accessibility elements, web `flt-semantics*` DOM). The normal acquisition paths therefore see it **if semantics are enabled**.
- Provide an **opt-in Dart package** `impresario_flutter` (tiny, no heavy deps):
  - `Impresario.ensureSemantics()` → calls `SemanticsBinding.instance.ensureSemantics()` (keeps semantics on without a screen reader). Gated by build flag/flavor so apps can include it in QA builds only.
  - Helper `ImpresarioTag(id: 'login.submit', child: …)` wrapping `Semantics(identifier: …, container: …)`. The `identifier` maps to Android `resource-id`, iOS `accessibilityIdentifier` and web `flt-semantic-identifier` on supported Flutter versions. Document the minimum Flutter version; add a fallback for older versions (label-based).
  - Lint/doc guidance: use `Semantics(identifier:)` for automation ids, `label` for user-facing names; do not rely on `Key` for native-side lookup.
- Web (Flutter web, CanvasKit/Skwasm): detect `flt-glass-pane`; auto-activate accessibility by triggering the `flt-semantics-placeholder` and verify `flt-semantics` nodes appear. If still empty, report `Degraded{reason: FLUTTER_SEMANTICS_OFF}` with the fix instructions.

### 8.2 Layer B — In-app semantics extension (fast, viewport-only, push-based)

The Dart package registers a VM-service/service-extension (`ext.impresario.snapshot`) **and** an optional lightweight socket bridge, which:
- walks `RendererBinding.instance.pipelineOwner.semanticsOwner!.rootSemanticsNode` with transforms applied to get screen-space rects, pruning nodes that are hidden or outside the viewport (mirror §5.3);
- returns normalized nodes directly (id=`SemanticsNode.id`, `identifier`, `label`, `value`, `hint`, flags, actions, rect, focus state);
- pushes change notifications from the `SemanticsOwner` listener (it is a `ChangeNotifier`) → instant updates, no polling;
- works in debug/profile/release **when the package is included**, without needing the Dart VM service in release.

This is the recommended path for Flutter: faster and richer than reading it back through the platform accessibility layer.

### 8.3 Layer C — Widget inspector (debug/profile only, authoring aid)

When a VM service is reachable (debug/profile), query `ext.flutter.inspector.*` (widget summary tree, selected widget, creation location). Use it to enrich a selected semantics node with: widget type, `Key`/`ValueKey`, **source location (file:line)**. This lets the owner/AI jump to the code and add a missing `Semantics(identifier:)`. Optional; never required for locators.

### 8.4 Role mapping for Flutter

Derive role from semantics flags/actions: `isButton`→button, `isTextField`→textbox, `isLink`→link, `isHeader`→heading, `isImage`→image, `isSlider`→slider, `hasCheckedState`+`isInMutuallyExclusiveGroup`→radio, `hasToggledState`→switch, `hasCheckedState`→checkbox, `scrollsAcross`/scroll actions→scroll container, else `generic`/`text`.

### 8.5 When nothing is addressable

If a Flutter app has neither semantics nor the package:
1. Show a clear in-UI explanation with the exact fix (add the package or enable semantics) — "Flutter semantics off" banner.
2. Offer the **visual fallback**: screenshot region selection producing a `region` locator (template/OCR-assisted, normalized coordinates relative to screen *and* an optional nearby-text anchor), flagged fragile (§13.5).
3. MCP returns the same structured diagnosis so the agent can propose code changes.

### 8.6 Flutter TV/desktop-like focus

Honor `FocusNode` state via semantics `isFocused`; always include the focused node (§5.1). Verify on Android TV and tvOS where Flutter is used (the SolidTV port may add Flutter-adjacent surfaces; confirm in Phase 0).

---

## 9. Scene authoring from the inspector (requirement 5)

### 9.1 Principle

All scene reading/writing goes through the **existing DSL crate** (parser + lossless formatter/CST). Never hand-edit text. Preserve comments, ordering, alignment and untouched rows byte-for-byte. If the DSL crate lacks a lossless round-trip API, adding it is a prerequisite task (INS-9.0) — **ask first**, because it touches the DSL.

### 9.2 Operations (exposed via gRPC, Web UI, MCP)

- `ListScenes(workspace)`; `OpenScene(path)`; `CreateScene(path, name)` (validate path stays inside the workspace; refuse overwrite).
- `AddElement(scene, row_name?, locator, device)` — new row.
- `FillCell(scene, row, column, locator)` — complete an existing row's empty cell.
- `ReplaceCell(scene, row, column, locator, confirm)` — overwrite with explicit confirmation and returned diff.
- `RenameRow`, `RemoveRow` (UI only; MCP optional, gated).
- `ValidateScene(scene)` — run the DSL checks / LSP diagnostics logic.
- File watching: emit `SceneChanged` when the file changes on disk (editor, git, user). Edits use **optimistic concurrency**: client sends the file revision hash it last saw; mismatch → conflict result with the fresh content, no partial write. Writes are atomic (temp + rename).

### 9.3 Automatic column selection

The target column is derived from the **attached device's identity**, not typed by the user:

1. Resolve `device → platform/actor` using the device definition (TASK-2 device vs app definitions; TASK-3 actors).
2. Map to the scene table's column key as defined by the current DSL.
3. If exactly one column matches → fill it automatically, show it as pre-selected (still visible and editable in the UI).
4. If multiple columns match (e.g., two actors of the same platform) or none → **do not guess**. UI: selectable column picker with a sensible default. MCP: return a structured `AMBIGUOUS_COLUMN` error with candidates; the agent must choose.
5. If the scene has no column for this device, offer to add the column (DSL operation) with confirmation.

**Row matching assistance**: when adding an element, search existing rows for likely counterparts (same name, similar label/role in another column, test id equality) and propose "fill existing row X" vs "new row". Name suggestions derive from `test_id` → `name` → role (e.g. `LoginButton`), made unique and conforming to the DSL naming rules. Show the proposed name editable before commit.

**Cross-actor intelligence**: if multiple devices are attached to the same scene authoring session, a locator candidate can be evaluated live on each (§7.5 "cross-actor availability"), and the UI can fill several columns in one action, each with the best locator **for that platform**.

### 9.4 UI selection flow ("proper selectable")

1. Pick mode on the screenshot/tree → select node(s).
2. Side panel shows ranked locator candidates (from §7.5) with unique/fragile badges and live match highlighting.
3. Choose target scene (existing dropdown with search, or "New scene…").
4. Choose/confirm row (new or existing) and column (auto-filled).
5. Preview the exact diff of the `.scene` file; Commit applies it; the result is verified by re-resolving the written locator against the live snapshot and confirming `match_count == 1` (or showing a warning).
6. Undo last scene edit (kept as an in-memory inverse patch plus file revision check; not a git operation).

MCP mirrors steps 1–5 as tools (§11) with the same validations and error semantics.

---

## 10. Web UI (dedicated port)

### 10.1 Server

- Dedicated port (config `inspector.port`, default proposal `7311`; fall back to next free port and print the URL). Bind `127.0.0.1` by default; `--listen` for LAN requires an auth token (§14).
- Serves the embedded SPA (single binary: embed built assets) + `/ws` WebSocket (protobuf binary, same messages as gRPC `Session`) + `/healthz`.
- The WS gateway is a thin translator: it opens a gRPC `Session` to the in-process core (or the master's endpoint) and forwards messages both directions. No business logic in the gateway.

### 10.2 SPA

TypeScript + a lightweight reactive framework (Preact or Solid) + Vite; no heavy UI kits. Must stay responsive with 2,000 in-view nodes.

Layout:
- **Left**: device picker (status, platform badge, lease mode), view options (include system UI, min visible fraction, screenshot scale/fps, freeze).
- **Center**: live screenshot with an **overlay canvas** for bounding boxes: hover highlight, selected highlight, match highlights, focus ring, scroll-container badges. Click-through selection; Alt/Cmd+click cycles overlapping candidates (z-order list). Mapping uses `ScreenInfo` and frame size (§4.5).
- **Right top**: virtualized tree (viewport nodes only), search box accepting locator syntax (live resolve and count), collapse/expand, role/state icons; selecting syncs with the canvas.
- **Right middle**: selected node details (normalized + `raw`), locator candidates with badges, copy buttons.
- **Right bottom**: scene panel (target scene, row, column, diff preview, commit, undo).
- **Footer**: connection state, rev, latency (event→render), acquisition time per platform, degraded/lease banners.

### 10.3 Interaction (optional but valuable; gated by lease mode)

Click-to-tap, drag-to-swipe, wheel-to-scroll, type text, hardware keys. For TV platforms, an on-screen **remote/D-pad** with keyboard bindings (arrows/Enter/Back). Input is forwarded as `Input` events with immediate re-acquire. Disabled when the device lease is read-only (a run is active).

### 10.4 Freeze and stale handling

- **Freeze**: stops applying live updates (server keeps tracking); the user can inspect a transient state (toast, loading spinner) indefinitely. Un-freeze catches up with one full snapshot.
- Selected node disappearing → state `stale` (greyed, last-known candidates still usable) or `offscreen` (still exists but scrolled out; show "scroll to reveal" action). Never silently clear a selection.

### 10.5 TV specifics

D-pad navigation graph preview: from the focused node, probe/predict next focus for each arrow (only via real key presses when allowed; never in read-only mode), show focus path history, and highlight the focused node distinctly.

### 10.6 Quality

Keyboard accessible, dark/light, RTL-safe (the products under test use AR RTL; the tool UI itself needs LTR/RTL-correct rendering of Arabic labels in tree and details). Frontend tests with Playwright against a mock server (fixtures replaying snapshot/delta streams).

---

## 11. MCP frontend

Entry: `impresario mcp` (extend the existing MCP surface if there is one; do not rename existing tools). Transport: stdio, plus streamable HTTP if the repo already supports it. Implemented as a gRPC client of the master.

### 11.1 Tools (names are proposals; keep the repo's naming style)

| Tool | Purpose |
|---|---|
| `inspector_list_devices` | Devices, platform, attach state, lease mode |
| `inspector_attach` / `inspector_detach` | Subscribe/unsubscribe (lease) |
| `inspector_get_view` | Current viewport snapshot as compact text (below) |
| `inspector_wait_for_change` | Block until the view changes (or timeout) and return the delta summary |
| `inspector_get_screenshot` | Current frame (scaled) |
| `inspector_find` | Resolve a locator → matches with refs |
| `inspector_describe` | Full details of a ref (incl. `raw`) |
| `inspector_suggest_locators` | Ranked candidates for a ref |
| `inspector_verify_locator` | Count + uniqueness on one or all attached devices |
| `inspector_scroll`, `inspector_tap`, `inspector_key`, `inspector_type` | Interaction (gated, lease permitting) |
| `inspector_scene_list`, `inspector_scene_create`, `inspector_scene_add_element`, `inspector_scene_fill_cell`, `inspector_scene_replace_cell`, `inspector_scene_validate` | Scene authoring with the same rules as §9 |

### 11.2 Compact snapshot format (token-efficient, Playwright-aria-snapshot-like)

```
device: tv-lg-01 (webos)  rev: 481  settled: true  screen: 1920x1080
- navigation "Main" [ref=e3]
  - link "Home" [ref=e4] [focused]
  - link "Movies" [ref=e5]
- list [ref=e9] [scroll: more below (14)]
  - listitem [ref=e10]
    - button "Play Dune" [ref=e11] [testid=card.play]
```
Rules: viewport-only, role + quoted name + `[ref=eN]` (ref = stable node id for the lifetime of the attachment), states in brackets, depth-limited and `max_nodes` parameter with truncation notice, optional `format=json`. No raw attributes unless asked via `inspector_describe`.

### 11.3 Instant feedback over MCP

MCP is request/response, so provide: (a) `inspector_wait_for_change` (long-poll over the gRPC stream; returns immediately when a change occurs); (b) MCP **resource** `impresario://inspector/{device}/view` with `notifications/resources/updated` for clients that support subscriptions; (c) every interaction tool returns the **post-action settled view delta** so the agent needs no extra call.

### 11.4 Safety

- Scene writes only inside the configured workspace; reject path traversal.
- Destructive/overwriting operations require an explicit `confirm=true` and return the diff.
- Input tools disabled when a run holds the device (structured `DEVICE_BUSY_READONLY` error with retry guidance).
- Errors are structured (`code`, `message`, `candidates`/`hint`) so agents can recover (`AMBIGUOUS_COLUMN`, `STALE_REF`, `NOT_INSPECTABLE`, `FLUTTER_SEMANTICS_OFF`, `CONFLICT`).

---

## 12. Master changes, agent alignment, and not breaking anything (requirements 10, 12)

### 12.1 Master changes

1. **Device session sharing + leases**: new `InspectorLease{device, mode: READ_ONLY|INTERACTIVE, owner, priority}` managed by the master. The inspector and test runs share the same underlying device/agent connection (multiplexed), never a second one.
2. **Run-vs-inspector policy** (ties into TASK-3's resident runner/scheduler): scheduled or background runs have priority by default. When a run starts on a device with an INTERACTIVE inspector lease: downgrade the lease to READ_ONLY (inspection continues, input disabled) and emit `LeaseEvent` to frontends; if the run needs exclusive access (configurable per device), queue/notify per scheduler policy. Provide config `inspector.run_conflict = readonly | exclusive | block_run` with default `readonly`. **Ask the owner to confirm this default** (§18).
3. **Capability negotiation**: extend the agent handshake with an `inspector` capability descriptor `{version, features: [viewport_prune, push_events, delta, screenshots, flutter_semantics, webview, focus_graph, ...]}`. The master uses it to enable/disable features per device and to explain degradations to the UI/MCP.
4. **Shared locator resolver**: refactor the runner's locator resolution to call `impresario-locator` (behavior-preserving, covered by the existing test suite *plus* new parity tests) before adding any new kind.
5. **Scene editing API**: expose DSL read/format/edit as a library API (if not already) and route inspector writes through it.
6. **Config**: new additive section (e.g. `[inspector]`: `enabled`, `port`, `listen`, `token`, `include_system_ui`, `min_visible_fraction`, `fps`, `screenshot_scale`, `run_conflict`). CLI-editable like other configs (TASK-2: configs editable from the CLI). New commands: `impresario inspector` (start/open UI), `impresario inspector status`. Existing commands untouched.
7. **Resident runner/background** (TASK-3): the master's per-user background process hosts the inspector when running; `impresario inspector` starts the master if it is not up, consistent with how other commands do it.
8. **Observability**: structured logs/metrics for acquisition time, delta size, event→render latency, dropped frames, lease transitions.
9. **Docs**: user docs for the inspector, locator reference, Flutter integration guide, per-platform prerequisites (dev mode, signing), troubleshooting for each `Degraded` reason.

### 12.2 Agents (all platforms) — alignment checklist

For each agent, produce a short design note and implementation, then tick the checklist in `INSPECTOR-NOTES.md`:

- [ ] **Web/CDP agent** (web, webOS, Tizen): injected script, binding push, screencast, old-engine fallback, webview/iframe handling.
- [ ] **Android agent / helper APK** (Android, Android TV): viewport-pruned traversal, events, focus, WebView bridge, Compose test tags, screenshot path.
- [ ] **Apple runner** (iOS, tvOS): minimal-attribute lazy-expansion snapshots, change fingerprint, focus fast path, screenshot stream, WKWebView bridge.
- [ ] **Flutter bridge** (Dart package + host adapter): semantics extension, push, optional widget inspector enrichment.
- [ ] **Existing automation paths** (tap/swipe/key/text/launch, TASK-3 on-device checks like Showing/Player/DRM): unchanged API and behavior; verify by running the existing end-to-end suites unchanged.

Rules for every agent change:
- New messages/fields are **optional**; an old agent that does not advertise `inspector` simply yields "not supported" in the UI/MCP and the rest of the system works as before.
- The runner must remain compatible with an **older on-device helper** (version skew): capability negotiation + semantic versioning; the master refuses nothing it previously accepted.
- The on-device helpers add inspector code **without** altering the code paths used by test runs (separate classes/modules, no shared mutable state, no performance regression on the run path — add a regression benchmark for run-path step latency before/after).
- Helper/runner installation/versioning follows the existing mechanism (TASK-2 signing, auto-install); bump helper versions and handle upgrade-in-place.

### 12.3 DSL / LSP / editor extension

- Locator grammar extension (if needed, §7.1): propose in `INSPECTOR-NOTES.md`, implement in the DSL crate with parser, formatter, LSP diagnostics/completions/hover, and the TypeScript editor extension's standalone grammar (it must work without the LSP). Existing scenes must parse and format identically (golden tests over all existing scene files in the repo and examples).
- Optional later phase: LSP completions that use a live inspector snapshot ("suggest locators from the attached device"). Not required for v1; design the API so it is possible.

### 12.4 Anti-breakage guardrails (CI gates)

1. `buf breaking` / proto compatibility check.
2. Full existing test suite and example/e2e suites must pass unchanged.
3. Golden parse/format tests for all existing `.scene`, theatre and play files.
4. Run-path latency benchmark must not regress by more than a small, documented tolerance (propose 3%).
5. Feature flag: `inspector.enabled=false` (default **[ASSUMPTION]**: enabled only when the user starts it) must result in no new background work, ports, or acquisition.
6. Old-agent/new-master and new-agent/old-master compatibility tests (handshake matrix).

---

## 13. Robustness: graceful selection and parsing (requirement 7)

### 13.1 Parsing

- Parsers are **total functions**: unknown/missing attributes → defaults; unknown roles → `generic`; malformed nodes are skipped and recorded in `Snapshot.degraded[]` with a node path and reason; the rest of the tree is delivered.
- Defensive limits: max depth (default 80), max nodes in view (default 5,000, with truncation marker and `degraded`), max text length per field, max `raw` size per node, cycle detection by id, invalid UTF-8 replaced, control characters stripped, RTL/bidi text preserved.
- Time budgets: per-acquisition timeout (platform-specific); on timeout return the last good snapshot + `Degraded{reason: TIMEOUT, partial: true}` and keep retrying with backoff. Never block the stream.
- Protocol errors from agents (disconnect, runner crash, CDP detach, app backgrounded/killed): emit `Degraded`/`LeaseEvent`, attempt automatic re-attach with backoff, restore selection when possible. App not running → friendly state with "launch app" action.
- Fuzz tests for every parser (web script output, Android node payload, Apple snapshot payload, Flutter payload) and property tests (id stability, delta apply == full snapshot).

### 13.2 Mixed/edge cases to handle explicitly

Overlays/modals/popups/dialogs, keyboards, split screen, multiple windows, rotation mid-stream, iframes/cross-origin frames (mark opaque), shadow DOM, `<canvas>`/`<video>` opaque nodes, scroll containers with virtualization (recycled items), carousel/TV rails, lazy-loaded content, text scaling, RTL layouts, nodes with identical names (disambiguation by scope/nth with fragility flag).

### 13.3 Selection model

- Selection = `{node_id, fingerprint: {role, name, test_id, path-hash, bounds}, last_candidates}`.
- On every snapshot/delta the server **re-resolves** selection: same id → `alive`; id gone but fingerprint uniquely matches → remap (`alive`, `remapped=true`); otherwise `stale`/`offscreen`.
- Multi-select supported (for adding several rows at once).
- Selection state is part of the server stream so Web UI and MCP agree.

### 13.4 Concurrency

Single-writer per device acquisition loop; immutable snapshots shared via `Arc`; deltas computed once and fanned out; scene edits serialized per file.

### 13.5 Visual fallback

When no tree exists (non-inspectable production TV apps, DRM surfaces, canvas/game UIs, Flutter without semantics): enable screenshot-region selection, producing `region` locators (normalized rect + optional template hash and nearby-text anchor via OCR if the repo already ships/permits one). Mark **fragile**, explain why, and never default to it when any tree-based option exists. DRM/secure surfaces produce black frames: show a "protected content" note and point to TASK-3 player/DRM checks.

---

## 14. Security

- Default bind `127.0.0.1`; non-local requires `inspector.token` (random generated, shown once, required via header/WS subprotocol/query on first load then cookie).
- Check `Origin`/`Host` on WS upgrade (DNS-rebinding protection); strict CSP for the SPA; no remote assets.
- MCP/Web scene writes sandboxed to the workspace; no path traversal; symlink-safe.
- No device secrets (signing identities, SSH keys) ever appear in snapshots, logs, or the UI.
- Snapshots can contain sensitive on-screen data (passwords fields: mask `value` for secure text fields on all platforms; redact by default and make `reveal` an explicit local-only option).
- Rate-limit inputs; audit log of inspector-originated device input and scene writes.

---

## 15. Testing and benchmarks

### 15.1 Unit/property

Locator matching, role map, name computation, synthesis ranking, stability scoring, id stability, delta correctness (`apply(delta, prev) == next`), pruning geometry (clip intersection, transforms), parsers (fuzz), selection re-resolution, scene edit round-trip (byte-preserving for untouched content), column resolution.

### 15.2 Integration

- **Fixture-based**: recorded real snapshots from each platform (web, webOS, Tizen, Android, Android TV, iOS, tvOS, Flutter×{Android,iOS,web}) stored as golden fixtures; run the whole pipeline (normalize → prune → synthesize → resolve → scene edit).
- **Mock agents** that replay event streams (scroll, navigation, modal open/close) for the bidirectional protocol and for the Web UI.
- **Real-device smoke** (manual/CI-optional): one device per platform, scripted scenario: attach → scroll → navigate → select → add to scene → run the scene and confirm the locator resolves.
- **Sample apps** (small, in-repo): native + Flutter + web test app with lists, modals, carousels, RTL text, virtualization, hidden elements, overlays.

### 15.3 Benchmark harness

Reproducible CLI (`impresario inspector bench --device X --scenario scroll|navigate|modal`) producing the §5.6 table as JSON/Markdown: acquisition time, nodes in view vs total nodes, payload size, event→render latency, CPU. Include an Appium Inspector baseline procedure in docs. CI runs mock-based perf tests; device perf runs are manual with results committed to `INSPECTOR-NOTES.md`.

### 15.4 Web UI tests

Playwright tests against the mock server: selection, overlay alignment under scale/rotation, tree virtualization, stale/offscreen states, freeze, scene commit flow, auth token flow, RTL rendering.

---

## 16. Phased tasks

Each task: **Goal → Steps → Acceptance**. Do phases in order; tasks inside a phase may be parallelized where noted. Check the box and note results in `INSPECTOR-NOTES.md`.

### Phase 0 — Recon and alignment (no feature code)

- [ ] **INS-0.1 Repository audit.** Map crates, the master process, gRPC server/proto locations, agent channel, existing MCP server, DSL crate (parser/formatter/CST), locator resolution code in the runner, device/app definition model, scheduler/resident runner, config system, helper/runner installation mechanism. Confirm or correct every **[ASSUMPTION]** here. *Acceptance:* `INSPECTOR-NOTES.md` section "Repo map" + a corrected-assumptions list.
- [ ] **INS-0.2 DSL audit.** Read the real scene grammar (tabular locators, actor/column keys, naming rules) and the TS extension. Decide reuse vs extension (§7.1). *Acceptance:* written proposal + owner approval for any grammar change.
- [ ] **INS-0.3 Baseline measurements.** Run Appium Inspector (or document how) on 2–3 representative screens per platform and record timings; record current run-path step latencies (for §12.4 gate 4). *Acceptance:* numbers in notes.
- [ ] **INS-0.4 Open questions.** Send the owner the unresolved items from §18 that Phase 0 did not answer. *Acceptance:* answers recorded.

### Phase 1 — Foundations (core, contracts, shared locator)

- [ ] **INS-1.1 Proto + generated code** (§4). *Acceptance:* `buf breaking` clean; round-trip serialization tests.
- [ ] **INS-1.2 `impresario-locator` crate**: model, parser/printer (per §7.1/0.2 outcome), matcher, role map, name rules; refactor the runner to use it behavior-preserving. *Acceptance:* all existing runner tests pass unchanged; new unit tests; parity harness skeleton.
- [ ] **INS-1.3 `inspector-core` skeleton**: subscription manager, lease manager, snapshot store (`Arc`), diff engine with property tests, backpressure/coalescing, mock-agent harness. *Acceptance:* delta property tests; stream stress test with slow client.
- [ ] **INS-1.4 Capability negotiation + feature flag + config + CLI** (`impresario inspector`, status). *Acceptance:* with the flag off nothing new runs; handshake matrix tests pass.

### Phase 2 — Web family first (web, webOS, Tizen)

- [ ] **INS-2.1 Injected viewport script** (pruning, roles, names, ids, shadow DOM/iframes, ES5-safe build). *Acceptance:* fixture tests in headless Chromium; pruning correctness suite (fixed/sticky/transform/overflow cases).
- [ ] **INS-2.2 Event push** (`Runtime.addBinding`), screencast frames, polling fallback, delta wiring. *Acceptance:* §5.6 web budgets met on desktop web; scroll/navigation/modal scenarios update the UI without manual refresh.
- [ ] **INS-2.3 webOS and Tizen attach** via native SSH/SDB (TASK-2 code), inspectable-build detection, `NOT_INSPECTABLE` degradation, engine feature detection. *Acceptance:* real device smoke on each (or recorded fixtures + documented device gap).
- [ ] **INS-2.4 Web UI MVP** (device list, screenshot+overlay, tree, selection, locator candidates, details) — can start in parallel with 2.1 using mock agents. *Acceptance:* Playwright UI tests.

### Phase 3 — Locator synthesis + scene authoring

- [ ] **INS-3.1 Synthesis + stability scoring + verification** (§7.5) with golden fixtures. *Acceptance:* parity tests (§7.6) green.
- [ ] **INS-3.2 DSL lossless edit API** (if missing; ask first, §9.1). *Acceptance:* byte-preserving round-trip on all repo scenes.
- [ ] **INS-3.3 Scene operations + column resolution + conflict handling + file watching** (§9). *Acceptance:* all §9.3 cases (auto, ambiguous, missing column, overwrite confirm, concurrent edit conflict) covered by tests.
- [ ] **INS-3.4 Web UI scene flow** (§9.4) incl. diff preview, verify-after-commit, undo.
- [ ] **INS-3.5 MCP frontend v1** (§11) with compact snapshot, wait-for-change, scene tools, structured errors, resource subscription. *Acceptance:* MCP conformance tests + an end-to-end script where an AI client attaches, finds an element, and fills a scene cell.

### Phase 4 — Android / Android TV

- [ ] **INS-4.1 Helper viewport traversal + events** (§5.4, §6.2) and native ADB transport hookup. *Acceptance:* §5.6 Android budgets; scroll in a RecyclerView/LazyColumn updates within budget.
- [ ] **INS-4.2 Focus model + TV remote UI/D-pad** (§10.5). *Acceptance:* focus ring correct across a TV rail sample.
- [ ] **INS-4.3 WebView bridge** via devtools socket. *Acceptance:* merged native+web tree on the sample app.
- [ ] **INS-4.4 Compose/testTag + resource-id handling and docs.**
- [ ] **INS-4.5 Run-path regression check** for the helper (§12.2).

### Phase 5 — Apple (iOS, tvOS)

- [ ] **INS-5.0 Spike S1** (§17): measure the candidate approaches (full snapshot baseline, minimal attributes + depth, lazy viewport expansion, audit service) on iOS and tvOS across at least two OS versions; commit results. *Acceptance:* decision recorded; budgets feasibility stated.
- [ ] **INS-5.1 Runner implementation** per the spike decision, with warm session reuse and signing/install via existing mechanisms.
- [ ] **INS-5.2 Change fingerprinting + adaptive polling + immediate post-input refresh** (§6.4).
- [ ] **INS-5.3 tvOS focus fast path + remote input.**
- [ ] **INS-5.4 WKWebView bridge** (optional if time-boxed; document if deferred).
- [ ] **INS-5.5 Acceptance:** §5.6 Apple budgets and the documented multiple over the Appium baseline.

### Phase 6 — Flutter

- [ ] **INS-6.1 Dart package** (`ensureSemantics`, `ImpresarioTag`, service extension, `SemanticsOwner` push) with example app and docs.
- [ ] **INS-6.2 Host adapter + normalizer** for Layer B on Android/iOS/web; Layer A auto-detection and `FLUTTER_SEMANTICS_OFF` diagnosis; Flutter web placeholder activation.
- [ ] **INS-6.3 Widget inspector enrichment** (debug/profile): widget type, key, source location.
- [ ] **INS-6.4 Visual fallback** (§13.5) end-to-end.
- [ ] *Acceptance:* in the sample Flutter app, every interactive widget tagged with `ImpresarioTag` is selectable and produces a unique `testid` locator on Android, iOS and web; untagged widgets with labels produce `role`/`label` locators; a deliberately untagged, unlabeled widget triggers the diagnostic and visual fallback.

### Phase 7 — Hardening and release

- [ ] **INS-7.1 Lease/scheduler integration tests** (§12.1.2): a scheduled run starts during an interactive inspector session on the same device.
- [ ] **INS-7.2 Security review items** (§14) implemented and tested.
- [ ] **INS-7.3 Full benchmark run + report**, compare with Phase 0 baselines.
- [ ] **INS-7.4 Docs, troubleshooting, migration notes**; verify all §12.4 CI gates are enforced.
- [ ] **INS-7.5 Final checklist** (§19).

---

## 17. Risks and spikes

| ID | Risk | Mitigation |
|---|---|---|
| S1 | Apple snapshot speed with no Mac and no stock WDA assumptions | Spike INS-5.0; lazy expansion; audit-service hybrid; document achieved numbers and ask before shipping a slower mode |
| S2 | Apple has no push events | Fingerprint polling + post-input refresh; measure CPU/latency; look for legitimate AX notifications |
| S3 | Old webOS/Tizen engines lack CDP domains or modern JS | ES5 script, feature detection, polling fallback, per-engine support matrix |
| S4 | Non-inspectable production TV apps | Clear `NOT_INSPECTABLE` diagnosis, dev-build requirement documented, visual fallback |
| S5 | Android helper traversal cost on huge/virtualized lists | Early pruning, prefetch flags, benchmark on long lists |
| S6 | DSL lacks lossless editing or Playwright-style locators | Ask first; additive change with golden tests |
| S7 | Run/inspector device contention | Lease model + scheduler integration tests; owner confirms default policy |
| S8 | Flutter semantics off or old Flutter versions | Dart package, version matrix, fallback labels, diagnosis banner |
| S9 | Coordinate mismatches (scale, rotation, TV scaling) | Explicit `ScreenInfo`, overlay alignment tests per platform |
| S10 | Sensitive data exposure | Secure-field masking, local-only default, token auth |

---

## 18. Open questions for the owner (answer before or during Phase 0)

1. Confirm the meaning of **master** and **agents** in the current codebase (§0.1) and their real names.
2. Does the scene DSL already support role/text/label/testid-style locators? If not, approve adding the extension (§7.1, §12.3).
3. What exactly are the scene table **column keys** today (platforms vs actors per TASK-3) and how does a device map to them? How should two devices of the same platform behave (§9.3.4)?
4. Run-vs-inspector conflict default (§12.1.2): `readonly` (recommended), `exclusive`, or `block_run`?
5. Should the Web UI port be started automatically with the master, or only on `impresario inspector`? (Spec assumes only when started.)
6. Is shipping a small Dart package in the repo acceptable, and is requiring it (or semantics enabled) in Flutter QA builds acceptable?
7. Minimum supported OS/engine versions per platform (iOS/tvOS versions, Android API level, webOS/Tizen versions) for the support matrix.
8. Is OCR already available/permitted for the visual fallback, or should the fallback be template-only?
9. Should the inspector (or MCP) be allowed to perform **input actions** on devices, or read-only inspection plus scene authoring only, by default?
10. How does the `watchdog` project consume impresario's agents/MCP today? Confirm that none of its flows are affected (audit its integration points in Phase 0 and add them to the compatibility tests).

---

## 19. Definition of done

- [ ] Web UI on a dedicated port and MCP both work against all seven platform families (web, webOS, Tizen, Android, Android TV, iOS, tvOS) plus Flutter apps, within §5.6 budgets or with documented owner-approved exceptions.
- [ ] Only in-view elements are acquired, transported, and shown; scroll hints exist for off-view content.
- [ ] Scroll, navigation, modal, and focus changes update both frontends automatically over the bidirectional stream.
- [ ] A user or AI agent can select an element, get ranked Playwright-style locators, pick or create a `.scene`, and add it with the correct column auto-filled (or an explicit ambiguity resolution); the written locator is verified live and resolves identically in a real run.
- [ ] Parsing/selection failures degrade gracefully; fuzz/property suites are green.
- [ ] Flutter apps are addressable via semantics identifiers/labels, with the diagnosis and visual fallback when not.
- [ ] Master changes, leases, capability negotiation, config and CLI are in place; all agents implement or explicitly declare lack of the `inspector` capability.
- [ ] All §12.4 anti-breakage gates are green; existing suites, scenes, DSL golden tests, and run-path benchmarks show no regression.
- [ ] Docs, troubleshooting, and `INSPECTOR-NOTES.md` (decisions, benchmarks, deviations) are complete and reviewed by the owner.
