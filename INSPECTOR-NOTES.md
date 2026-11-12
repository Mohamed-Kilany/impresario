# INSPECTOR-NOTES.md — Impresario Element Inspector

Working log for `TASK-INSPECTOR.md` (spec v1). It records the Phase 0 audit,
every corrected assumption, the architecture chosen for v1, the locator
grammar proposal, benchmarks, open questions and deviations.

Status legend: **done**, **in progress**, **deferred** (documented, not
claimable without hardware or owner input).

---

## 1. Phase 0 — repository audit (INS-0.1)

### 1.1 Repo map

| Spec term | Real component | Notes |
|---|---|---|
| master | `impresario` CLI (`crates/impresario-master`, binary `impresario`) | **Not** a resident master with a gRPC endpoint. `impresario service` is the resident runner; it has no gRPC server. |
| agents | per-device executors: `impresario-connector` (web/webOS/Tizen over CDP), Android agent (native ADB), Apple runner (XCTest) | Each is a tonic **server** exposing `impresario.v1.Agent`. |
| agent channel | `impresario-clients::agent_client::AgentClient` / `AgentSession` | Handshake, one command stream, out-of-band screenshots, logs, shutdown. |
| gRPC/proto | `proto/impresario/v1/impresario.proto` → `impresario-proto` | One package, one generated module (`impresario_proto::v1`). |
| DSL crate | `impresario-core` (`ast`, `parser`, `fmt`, `selectors`, `workspace`) | Parser keeps spans; formatter is whole-file. No lossless CST. |
| locator resolution | web: `impresario-connector/src/exec.rs` + embedded ES5 script; Android/Apple: their agents | No shared Rust resolver. |
| device/app model | `impresario-core::config` (`DeviceConfig`, `DevicesConfig`), `apps.rs` (`AppConfig`) | Devices have platform/type; apps bind per platform. |
| scheduler/resident | `impresario-core::resident`, `impresario-master::resident` | File-based schedules/watchers; no device session broker. |
| config | `impresario-core::config` + `settings` schema | `ImpresarioConfig` is `deny_unknown_fields`; top-level keys are validated, then deserialized. |
| helper installation | `impresario-clients::transport` + platform-specific code | Connector is spawned per device on an ephemeral loopback port. |
| MCP | `impresario mcp` (stdio, JSON-RPC 2.0, dual-era) | Calls library functions directly; no gRPC client. |
| Web console | `impresario web` (hand-written HTTP/1.1, port 7810) | One request per connection, embedded SPA, no WebSocket. |
| CLI | `impresario-master::cli` (clap) | Commands: config, workspace, validate, parse, devices, run, probe, service, mcp, web, lsp, inspect, mock-agent. |
| TUI | `impresario-master::tui` | Explicitly **out of scope** for the inspector. |

### 1.2 Corrected assumptions

1. **"master" is the CLI process, not a resident gRPC master.** `TASK-INSPECTOR`
   §3.1 assumes a long-lived master that owns devices and serves gRPC. In this
   repo the CLI owns device connections only for the duration of a command;
   the resident runner (`impresario service`) has no gRPC listener. This is the
   central contradiction; the v1 resolution is in §2.
2. **The connector already owns the web device session** and exposes
   `Agent`; a new inspector service can reuse `SessionManager`/`Session`
   without opening a second browser connection.
3. **Scene columns are platform keys**, not actor names. The parser rejects a
   duplicate platform column (`column 'web' is declared twice`). Therefore
   "two actors of the same platform" cannot currently be represented in one
   scene table. The inspector returns `AMBIGUOUS_COLUMN`/`MISSING_COLUMN`
   rather than guessing; adding actor-qualified columns is a DSL change left
   to the owner (open question 3).
4. **Existing selector syntax is `prefix:value`** per platform:
   web `css:`/`xpath:`/`text:`/`textContains:` (bare = CSS); Android
   `id:`/`text:`/`textContains:`/`desc:`/`class:`; Apple
   `accessibilityId:`/`label:`/`labelContains:`/`predicate:`/`type:`.
   There is no Playwright-style `role=...`, `testid=...` or `>>` chaining.
5. **`impresario inspect` is the existing probe CLI**, not an element
   inspector. It must not be renamed or repurposed.
6. **`impresario web` has no WebSocket and closes after one request.**
   The inspector UI therefore uses JSON + Server-Sent Events (SSE) in v1.
7. **MCP is stdio and calls library functions directly.** It is not a gRPC
   client of a resident master. Inspector MCP tools call the inspector HTTP
   API over loopback (the local frontend boundary), not device APIs directly.
8. **`impresario-core` must stay async/gRPC-free** (D-16). The locator and
   inspector-core crates are pure/sync; the connector and master own async.
9. **No local Rust build/test is available.** CI is the only compile/test
   gate; local work is limited to `cargo fmt --all`. The notes never claim a
   hardware or local-test result that was not observed.

### 1.3 Baseline measurements (INS-0.3)

**Deferred.** Real devices and Appium Inspector are not available in this
environment. No numbers are invented. The hardware-free core pipeline
benchmark is implemented (`impresario inspector bench --scenario
scroll|navigate|modal`); it reports p50/p95/p99/mean for acquisition build,
prune, diff, apply, compact and serialize, view-vs-total nodes, payload bytes
and a mock single-threaded CPU budget. It does not measure CDP, a browser or a
TV; those runs and the Appium comparison remain manual and will be recorded
here only with observed results.

---

## 2. Architecture decision for v1 (spec §3 deviation)

**Decision:** the inspector is a dedicated `impresario inspector` process
that uses a shared pure `impresario-inspector-core` and `impresario-locator`,
plus an **additive** `Inspector` gRPC service on each executor. The Web UI is
served by the inspector process on port `7311` by default; MCP tools call its
loopback HTTP API.

Why this deviates from §3.1:

- There is no resident master gRPC endpoint to host the service. Extending
  `impresario service` into a device-session broker is a much larger change
  and would touch the scheduler's process model.
- A dedicated process can reuse the existing `ConnectorTransport` +
  `AgentClient` flow unchanged and hold one session per attached device.
  Runs still spawn their own connector, so v1 does **not** claim shared
  device sessions or lease preemption. That is recorded as a deviation, not
  silently ignored.
- The core is still single-sourced: the same `impresario-locator` matcher
  resolves inspector locators and (for snapshot-backed paths) runner
  verification. The connector implements the same canonical locator string
  for live web execution.

**v1 deviations (explicit):**

1. Dedicated inspector process, not a resident master service.
2. JSON + SSE, not protobuf-over-WebSocket. The HTTP gateway is still a thin
   translator; the SPA contains no acquisition logic.
3. Unary inspector RPCs (`GetSnapshot`, `Resolve`, `SuggestLocators`, `Input`,
   `Screenshot`) rather than the full bidi `Session` stream. The Web UI gets
   push-like updates via SSE polling while attached; the proto is append-only
   so the bidi stream can be added later without breaking clients.
4. Hand-written embedded SPA (same pattern as `impresario web`), not
   Preact/Vite. No external assets, CSP-safe, one binary.
5. No lease/scheduler preemption in v1. Input is disabled when a run owns the
   device only if the transport reports the device busy; otherwise the UI
   warns that a concurrent run may contend. This is an honest limitation.
6. Android/Apple/Flutter inspector acquisition is not claimed in v1. The
   connector implements web/webOS/Tizen snapshots. Other platforms return a
   structured `NOT_SUPPORTED` degradation; their design notes are below.
7. **Locator execution is not yet shared with the runner.** The inspector and
   `impresario-inspector-core` use `impresario-locator` for matching and
   synthesis, and the DSL/proto accept `Strategy::Locator`. The connector's
   run path still evaluates only the legacy `prefix:value` strategies, so a
   scene cell written as a canonical locator is rejected at run time with a
   structured invalid-selector error instead of being executed. Closing this
   needs an async resolve-then-act path in the connector (snapshot + Rust
   matcher + node-id handle), not a second JS matcher. It is the main
   unmet invariant (spec requirement 3) and is not claimed as done.

---

## 3. DSL audit and locator grammar (INS-0.2)

### 3.1 Existing grammar

- A `.scene` table is `| Piece | <platform>... |`.
- A cell is a platform selector value; `-`, `~`, `none`, `null` and empty are
  absent.
- Selector parsing is in `impresario-core/src/selectors.rs`; `Strategy` is a
  closed enum mirrored in the proto.
- Existing scenes/examples use only the legacy `prefix:value` forms.

### 3.2 Proposal (additive, existing scenes unchanged)

The inspector accepts a canonical locator expression in a scene cell **in
addition to** the legacy forms. Detection is unambiguous: a cell is a locator
expression when it starts with one of the locator keywords followed by `=`
or is exactly `focused`, or when it contains a top-level `>>` or starts with
`within(`. Anything else is passed to the existing selector parser exactly as
before. This means every existing scene parses and formats identically.

Kinds:

```
role=<role>[name="..."]...     text=...       label=...
placeholder=...                alt=...        title=...
testid=...                     id=...         css=...        xpath=...
focused                        region=x,y,w,h
```

Modifiers/composition:

```
>>                              descendant scope
within(<locator>)               scope the following locator
[has=(<locator>)]               descendant must match
[has-not=(<locator>)]           descendant must not match
[nth=N] [first] [last]          positional selection
[visible=false]                 include non-visible nodes (default: visible only)
[context=NATIVE|WEBVIEW|FLUTTER|WEB]
[name="..."][exact][i]          name matching options
```

Strings may be quoted (`"..."` or `'...'`) with backslash escapes; `/.../` is
a regular expression. Bare values are accepted for `testid`, `id`, `css`,
`xpath`, roles and region. Whitespace in text/name matching is normalized.

Examples:

```
role=button[name="Sign in"]
role=button[name="Sign in"][disabled=false]
text="Welcome"
text=/^Hi, .+/
testid=login.submit
id=com.app:id/ok
within(role=dialog[name="Delete"]) >> role=button[name="Delete"]
role=list[has=(role=listitem)] >> role=button[nth=0]
```

**Approval note:** `TASK-INSPECTOR` §7.1/§12.3 asks for owner approval before
adding grammar. The owner's instruction for this task was "check and
implement"; this proposal is additive, does not change any existing scene,
and is recorded here. The owner can veto or rename any form before release;
the implementation keeps the grammar in one module so a rename is local.

### 3.3 Scene editing format

- Existing cells keep their bytes. Only the target cell/row is replaced or
  appended.
- New rows are appended after the last table row, aligned to the header
  widths where practical; existing rows are never reformatted.
- Writes are atomic (temp file + rename) and guarded by a SHA-256 revision of
  the file the client last saw.
- Column selection uses the device's `Platform` and the scene header. One
  matching column is auto-selected. Zero or multiple matches return
  `MISSING_COLUMN`/`AMBIGUOUS_COLUMN` with candidates; nothing is guessed.

---

## 4. Open questions (INS-0.4)

| # | Question | v1 answer / status |
|---|---|---|
| 1 | Meaning of master/agents | Answered in §1.2. |
| 2 | Approve locator extension? | Additive proposal in §3.2; implemented under the "implement" directive; owner may veto. |
| 3 | Column keys / same-platform devices | Platform keys; duplicates rejected by the parser. Same-platform devices return `AMBIGUOUS_COLUMN`. Actor-qualified columns deferred to the owner. |
| 4 | Run-vs-inspector conflict default | `readonly` is the default in config, but v1 does not yet preempt; it is a documented limitation until the scheduler integration lands. |
| 5 | Start UI automatically? | Only on `impresario inspector`. `inspector.enabled` defaults to false and is currently reserved (the resident service does not auto-start the inspector). |
| 6 | Dart package acceptable? | Deferred with Flutter (Phase 6); not claimed in v1. |
| 7 | Minimum OS/engine versions | Web: Chromium/WebKit with ES5 + `MutationObserver` (webOS/Tizen dev builds). Android/Apple: deferred to their phases. |
| 8 | OCR for visual fallback | Template-only is the default; OCR is not implemented/claimed. `region` locators are always marked fragile. |
| 9 | Allow input actions? | Yes when the device session is interactive; the UI shows a read-only banner when input is refused. Scene writes are always explicit. |
| 10 | watchdog integration | No watchdog integration points exist in this repo; the additive tools/RPCs do not rename or remove existing MCP tools or agent RPCs. |
| 11 | When will runner execution use the shared locator? | **Open blocker.** Inspector-side matching is shared; the connector run path rejects canonical locator cells. Do not claim requirement 3 until resolve-then-act lands. |

---

## 5. Phase checklist

### Phase 0 — Recon and alignment

- [x] INS-0.1 Repo map and corrected assumptions (§1).
- [x] INS-0.2 DSL audit and additive locator proposal (§3); owner approval
      treated as the task's "implement" directive, recorded above.
- [ ] INS-0.3 Baseline measurements — **deferred** (no devices/Appium); no
      numbers claimed.
- [x] INS-0.4 Open questions recorded (§4).

### Phase 1 — Foundations

- [x] INS-1.1 Append-only proto: `Inspector` service/messages, `LOCATOR`
      selector strategy, `INSPECTOR` capability.
- [x] INS-1.2 `impresario-locator`: model, parser/printer, role map, matcher,
      synthesis, stability scoring, tests; the canonical `LOCATOR` strategy is
      accepted by the DSL/proto. **Not** yet executed by the connector run
      path (see §2 deviation 7).
- [x] INS-1.3 `impresario-inspector-core`: snapshot model, delta/diff/apply,
      pruning geometry, selection re-resolution, compact snapshot, scene
      column resolution, mock harness, tests.
- [x] INS-1.4 Capability negotiation, config section, `impresario inspector`
      and `inspector status`; the inspector is opt-in and starts no listener
      or acquisition unless the command runs. Resident auto-start is not
      implemented.

### Phase 2 — Web family first

- [x] INS-2.1 Injected viewport script (ES5): pruning, roles, names, stable
      ids, composed/shadow DOM, ancestor clip geometry, scroll hints.
      Same-origin iframe subtrees are **not** traversed.
- [x] INS-2.2 Polling refresh: the inspector process re-acquires at
      `inspector.fps` (1-30, default 4) and pushes full snapshots/deltas over
      SSE. An in-page `MutationObserver`/`Runtime.addBinding` push channel is
      a follow-up; screenshots use the existing out-of-band capture.
- [ ] INS-2.3 webOS/Tizen real-device attach — transport reuse implemented;
      **real-device smoke deferred** (no devices). `NOT_INSPECTABLE` is a
      structured degradation.
- [x] INS-2.4 Web UI MVP: device list, screenshot + overlay, virtualized
      tree, selection, candidates, details, scene panel.

### Phase 3 — Locator synthesis + scene authoring

- [x] INS-3.1 Synthesis + stability scoring + verification with fixtures.
- [x] INS-3.2 Lossless scene edit API (`impresario_core::scene_edit`) with
      byte-preserving tests.
- [x] INS-3.3 Scene operations, column resolution, conflict handling and
      undo. There is no file watcher: the client re-opens after commit and
      stale revisions are refused.
- [x] INS-3.4 Web UI scene flow: diff preview, commit, re-open and undo.
      There is no automatic post-commit locator verification.
- [x] INS-3.5 MCP inspector tools: compact view, wait-for-change, find,
      describe, suggest, verify, input, scene tools, structured errors. The
      optional MCP view resource/subscription is not implemented.

### Phase 4 — Android / Android TV

- [ ] **Deferred.** Requires the on-device helper APK, ADB transport
      integration and real devices. The proto/capability surface is ready;
      the inspector returns `NOT_SUPPORTED` until the helper advertises
      `INSPECTOR`.

### Phase 5 — Apple (iOS, tvOS)

- [ ] **Deferred.** Requires the XCTest runner and signing/install on real
      devices. No speed claims are made.

### Phase 6 — Flutter

- [ ] **Deferred.** The Dart package, semantics bridge and widget inspector
      enrichment are not shipped in v1. The UI/MCP return
      `FLUTTER_SEMANTICS_OFF` when a Flutter surface is detected without
      semantics.

### Phase 7 — Hardening and release

- [x] INS-7.1 Lease/scheduler integration — **partial**: the
      `inspector.run_conflict` config exists, but v1 has no shared session or
      preemption; a concurrent run is not coordinated. Documented limitation.
- [x] INS-7.2 Security items: loopback default, Host/Origin checks, token,
      CSP, path sandboxing, atomic writes, secure-field masking. Deviation:
      the SPA authenticates non-loopback EventSource/initial loads with
      `?token=` (no cookie exchange yet).
- [ ] INS-7.3 Full benchmark run — **deferred** with hardware; the mock
      pipeline benchmark is implemented.
- [x] INS-7.4 Docs added (`CLI.md`, `WEB.md`, `MCP.md`, this file). The
      anti-breakage gates are **not** all enforced: CI has no `buf breaking`
      step, no run-path latency gate, no old/new handshake matrix, and the
      LSP/TS grammar does not yet complete/hover locator cells.
- [x] INS-7.5 Final checklist updated honestly (§6).

---

## 6. Definition-of-done status (honest)

| Item | Status |
|---|---|
| Web UI + MCP on all seven families + Flutter | **Partial**: web/webOS/Tizen acquisition; other families degrade with `NOT_SUPPORTED`; Flutter diagnosis only. |
| Viewport-only acquisition/transport/UI | **Yes** for web (injected script prunes before returning); other agents pending. |
| Auto-update on scroll/navigation/modal/focus | **Partial**: web polling at `inspector.fps` (default 4) plus post-input refresh; no push binding or change counter. |
| Select → ranked locators → scene add with column auto-fill → verify | **Partial**: select/suggest/add/fill/replace/validate/undo work for web; column ambiguity/missing cases are tested. The written locator is **not executable by the runner yet**, and there is no post-commit verification. |
| Graceful degradation + fuzz/property suites | **Partial**: tolerant parsers and property tests for delta/geometry; fuzzing of every agent payload is deferred with those agents. |
| Flutter addressability + fallback | **Deferred** (diagnosis only). |
| Master changes, leases, capability negotiation, config/CLI | Capability/config/CLI done; no shared sessions, leases or preemption (documented). |
| Anti-breakage gates | Existing suites untouched; proto is append-only by inspection; legacy selectors preserved; the run-path refactor is limited to accepting `Strategy::Locator` (which the connector then rejects). `buf breaking`, run-path latency and handshake-matrix gates are **not** wired. |
| Docs + notes reviewed | Docs added; owner review pending. |

---

## 7. Test and benchmark log

All local test runs are **not claimed** (no local Rust build/test). CI is the
authority. New tests live next to their code:

- `impresario-locator`: parser/printer round-trip, role map, matcher,
  synthesis ranking, dynamic-text rejection, stability golden cases.
- `impresario-inspector-core`: delta apply == full snapshot property,
  clipping geometry, pruning, selection re-resolution, compact snapshot,
  column resolution.
- `impresario-core::scene_edit`: byte-preserving edits on every repo scene,
  append/fill/replace/rename/remove, revision conflict.
- connector: inspector script ES5 lint, snapshot payload deserialization,
  capability advertisement.
- master: inspector HTTP routing/security tests, MCP tool schema/structured
  errors, scene preview/commit/undo, bench invariants.

Benchmark harness: `impresario inspector bench` runs the hardware-free core
pipeline over deterministic synthetic snapshots (`scroll`, `navigate`, `modal`)
and emits JSON or a table with p50/p95/p99/mean for acquisition build, prune,
diff, apply, compact and serialize, view-vs-total nodes, full/delta/compact
payload bytes, a mock single-threaded CPU budget and a mock `event_to_render`
pipeline metric. It accepts `--device` only as a report label; it does not
contact a device. Real-device and Appium numbers are manual and will be
committed here only when observed.

---

## 8. Deviations and risks

- **No shared device session with runs** (spec §3.2/§12.1.1): dedicated
  process; contention is possible. Mitigation: explicit read-only/busy
  errors, documented.
- **Locator cells are not executable by the runner yet** (spec requirement 3):
  the shared matcher is used by the inspector, but the connector run path
  rejects `Strategy::Locator` with a structured invalid-selector error.
  Mitigation: the grammar is isolated in one crate; closing it needs an async
  resolve-then-act path, not a second matcher.
- **Polling instead of CDP binding push** (spec §6.2): the process re-acquires
  at 1-30 fps; a change counter/binding push is a follow-up.
- **No bidi protobuf stream** (spec §4.2): unary RPCs + SSE; append-only
  proto leaves room.
- **No Android/Apple/Flutter acquisition**: structured `NOT_SUPPORTED`, no
  fabricated results.
- **Scene grammar extension**: additive and isolated; owner may veto. The LSP
  and TypeScript grammar do not yet complete/hover locator cells.
- **Same-platform actors**: not representable in the current table; explicit
  ambiguity error.
- **MCP resource subscription**: `impresario://inspector/{device}/view` and
  `notifications/resources/updated` are not implemented; use
  `inspector_wait_for_change`.
- **Token in query**: the SPA's EventSource and first non-loopback page load
  use `?token=`; a short-lived cookie exchange would avoid URL logging.
