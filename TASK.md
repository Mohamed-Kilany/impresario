# TASK - impresario authoritative implementation specification

Status: consolidated source of truth. This file merges the baseline
specification (formerly `TASK-1.md`), the second-generation work packages
(formerly `TASK-2.md`) and the third-generation delta (formerly `TASK-3.md`).

Read `IMPRESARIO.md`, `README.md`, `docs/DECISIONS.md`, `docs/STATUS.md`,
`docs/SPIKES.md`, `docs/PROBES.md`, `docs/PARITY.md` and `docs/SIGNING.md`
before changing code.

The working rules are cumulative: spike before commit; no unverified claims;
protocol facts verified from sources; licenses recorded; secrets only in the
OS keychain; tests, docs, decisions and STATUS updated with each work package;
fail loudly (`UNSUPPORTED_ACTION`); no silent no-ops; generated PARITY; honest
STATUS; Rust gates green at every commit (`cargo fmt --check`, clippy with
`-D warnings`, `cargo test --workspace`).

Precedence and the resolved conflicts are in Appendix A. The answered open
items are Part III section 13. Where this file and `docs/DECISIONS.md`
disagree, the later numbered decision wins and this file is corrected in the
same change.

---
# Part I - impresario baseline specification (v2)

> Audience: an AI coding agent that will implement this project end to end.
> Normative words: **MUST**, **MUST NOT**, **SHOULD**, **MAY** are used as in RFC 2119.
> This document supersedes the v1 brief. Section 19 lists what changed and why.

---

## 0. How to work

1. **Work in phases (section 17).** Finish a phase, run its acceptance checks, commit, then continue. Phase 0 is a set of risk spikes that MUST be done first.
2. **Never claim something works unless you ran it.** If a component cannot be built or run in your environment (Android emulator, Xcode, a real TV), say so in `docs/STATUS.md`, ship it with unit tests, and add a manual verification checklist.
3. **Do not invent APIs.** If you are unsure of a crate, Android, or XCTest API, read its docs or source. Pin all dependency versions. Versions named in this document are hints, not facts: verify them.
4. **This document is the source of truth.** If something is genuinely unspecified, take the simplest option, record it in `docs/DECISIONS.md`, and continue. Do not silently deviate: if you must deviate, record why.
5. Production Rust code MUST NOT contain `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, or unchecked indexing. Enforce with `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::todo, clippy::unreachable)]` in every crate (relaxed inside `#[cfg(test)]`).
6. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` MUST pass at every commit.
7. Anything intentionally unsupported MUST fail loudly with `UNSUPPORTED_ACTION` (never silently no-op) and appear in `docs/PARITY.md`.

---

## 1. Product, scope, glossary

`impresario` is a concurrent cross-platform UI test automation system. Tests are written once in a concise DSL and executed against:

| Device platform | Runtime | Executor |
|---|---|---|
| `web` | Desktop Chromium (mainly for CI and fixtures) | `impresario-connector` (CDP) |
| `webos` | LG webOS web apps | `impresario-connector` (CDP) |
| `tizen` | Samsung Tizen web apps | `impresario-connector` (CDP) |
| `android` | Android phones/tablets, any app | Kotlin agent (UiAutomator2) |
| `androidtv` | Android TV, any app | Kotlin agent (UiAutomator2 + D-pad) |
| `ios` | iPhone/iPad, any app | Swift agent (XCUITest) |
| `tvos` | Apple TV, any app | Swift agent (XCUITest + `XCUIRemote`) |

Glossary:
- **Scene** (`.scene`): one screen; maps logical element names (**Pieces**) to per-platform selectors.
- **Play** (`.play`): one test file. Contains **Acts** (test cases). An Act contains **Stages** (ordered phases). A Stage contains **Scene blocks**; a Scene block contains **steps** (Action or Check lines).
- **Master**: the `impresario` CLI binary. **Connector**: host-side daemon for CDP TVs. **Agent**: on-device native executor (Android, Apple). **Executor**: connector or agent (anything implementing the gRPC `Agent` service).
- **Session**: one handshake plus one `Session` stream between master and an executor for one device.

Out of scope for v2 (do not build): web desktop multi-tab management, image-based matching, video recording, cloud device farms, Windows host support for iOS/tvOS (impossible), parallel sessions on one physical device.

---

## 2. Decision log (binding; copy into `docs/DECISIONS.md`)

| ID | Decision |
|---|---|
| D-01 | Master is always the gRPC **client**; every executor is a gRPC **server** implementing one `Agent` service. |
| D-02 | Native agents are **single-session**. The connector is **multi-session** (one CDP connection per TV). |
| D-03 | CDP on TVs uses a **raw CDP backend** (JSON-RPC over WebSocket) by default. `chromiumoxide` is an optional backend (`cdp_backend: chromiumoxide`), because old TV Chromium builds break strict typed bindings. |
| D-04 | Config files are **YAML** (`serde_yaml_ng`), keys `snake_case`, unknown keys are errors. |
| D-05 | DSL line parsing: hand-written indentation scanner + `winnow` for line grammars. Diagnostics via `miette`. |
| D-06 | `'text' => P` is shorthand for `type 'text' => P`. **`type` REPLACES the field content** (clear, then enter text). Appending is a separate verb, `append`. |
| D-07 | Selector keys are exactly the device platform names (`web, webos, tizen, android, androidtv, ios, tvos`), with fallback chain `webos→web`, `tizen→web`, `androidtv→android`, `tvos→ios`. |
| D-08 | Comments in DSL files are lines starting with `//` (not `#`, which is valid CSS). |
| D-09 | `Check:` lines always **auto-wait** (poll) until the condition holds or the timeout elapses. There is no separate `waitfor`. |
| D-10 | `wait <duration>` and sleeping are **master-side only** and never sent over the wire. |
| D-11 | Logs: `StreamLogs` RPC is the single wire mechanism for executor logs. Host-side collectors (logcat, syslog) are master features. |
| D-12 | No session resume. After a transport failure the current Act fails with `TRANSPORT`; the master reconnects with a new session for the next Act. |
| D-13 | Android: **one** instrumentation APK; phone vs TV behavior is chosen at runtime from the handshake platform. Apple: shared `AgentCore` library, two UI-test bundles (iOS, tvOS). |
| D-14 | Apple server library: `grpc-swift` 1.x (NIO) while the OS floor is below the minimum of grpc-swift 2. Re-evaluate when the floor is raised. Verify in Phase 0. |
| D-15 | iOS `classChain` is NOT supported (it is a WebDriverAgent feature, not XCUITest API). |
| D-16 | `core` crate has **no dependency on the proto crate**; conversions live in `impresario-clients`. |
| D-17 | App reset policy `app_reset: per_act | per_play | never`, default `per_act`. |
| D-18 | If a gRPC server cannot run on a device (Phase 0 spike fails), fall back to length-prefixed protobuf frames over TCP behind the same `AgentClient` trait. Record the outcome in `docs/STATUS.md`. |

---

## 3. Architecture

```
                         ┌────────────────────────────── host machine ──────────────────────────────┐
 editor ──LSP/stdio──┐   │                                                                          │
                     ▼   │   impresario (master)                                                    │
 CLI ───────────► impresario ─┬─ gRPC ─► impresario-connector ──CDP/WebSocket──► LG webOS / Tizen / Chromium
                         │   │                                                                      │
                         │   ├─ gRPC over `adb forward` ──────────► Android agent (instrumentation) │
                         │   ├─ gRPC over `iproxy`/network ───────► iOS agent  (XCUITest runner)    │
                         │   └─ gRPC over network ────────────────► tvOS agent (XCUITest runner)    │
                         └──────────────────────────────────────────────────────────────────────────┘
```

### 3.1 Repository layout

```
impresario/
├── Cargo.toml                       # workspace (resolver = "2")
├── rust-toolchain.toml
├── buf.yaml / buf.gen.yaml          # proto lint + breaking checks
├── proto/impresario/v1/impresario.proto
├── crates/
│   ├── impresario-proto/            # prost/tonic generated code (build.rs); no logic
│   ├── impresario-core/             # config, DSL (lexer/parser/AST/validation), selectors, keys, durations, diagnostics, error catalog
│   ├── impresario-clients/          # AgentClient, DeviceTransport impls, core<->proto conversions, MockAgent (testing)
│   ├── impresario-connector/        # bin `impresario-connector`: gRPC server + CDP backends
│   └── impresario-master/           # bin `impresario`: CLI, planner, executor, reporters, LSP
├── agents/
│   ├── android/                     # Gradle: :agent-core, :driver-uiautomator, :agent-instrumentation, :agent-target-stub
│   └── apple/                       # SwiftPM package `AgentCore` + Xcode project (host stubs + UI test bundles for iOS and tvOS)
├── editors/vscode/                  # minimal client + TextMate grammar
├── fixtures/web/                    # HTML fixtures for connector tests
├── examples/                        # output of `init`, used by tests
├── scripts/                         # gen-swift.sh, gen-kotlin.sh, ci helpers
└── docs/                            # DSL.md PROTOCOL.md DEVICES.md PARITY.md LSP.md ERRORS.md DECISIONS.md STATUS.md
```

### 3.2 Crate dependency rules
- `core` ← `clients`, `connector`, `master`. `proto` ← `clients`, `connector`. `clients` ← `master`. No other edges. `core` MUST NOT depend on `tokio`, `tonic`, or `prost` (keeps the LSP and parser light and testable).
- Suggested crates (verify current versions): `tokio`, `tokio-util` (CancellationToken), `tonic`, `prost`, `tonic-build` family, `protoc-bin-vendored`, `serde`, `serde_yaml_ng`, `serde_json`, `clap` (derive), `thiserror`, `miette`, `tracing`, `tracing-subscriber`, `winnow`, `url`, `regex`, `tokio-tungstenite`, `futures`, `base64`, `quick-junit`, `tower-lsp-server` (maintained fork of tower-lsp; verify) , `notify`, `notify-debouncer-full`, `chromiumoxide` (optional feature), `insta`, `proptest`, `assert_cmd`, `tempfile`, `mdns-sd` (optional).

---

## 4. Configuration

Both files are YAML. Parsing structs use `#[serde(deny_unknown_fields)]`. Errors carry file, line, column, a message, and a "did you mean" hint for near-miss keys. All errors in a file are collected and reported together.

### 4.1 `impresario.config`

```yaml
project: impresario_tests
timeout_default_ms: 10000
screenshot_on_failure: true
report_output: ./reports
```

Optional keys with defaults:

| Key | Default | Rule |
|---|---|---|
| `plays_dir` | `./plays` | directory |
| `scenes_dir` | `./scenes` | directory |
| `retries` | `0` | 0..=5 |
| `parallel_devices` | `4` | 1..=64 |
| `app_reset` | `per_act` | `per_act` / `per_play` / `never` |
| `poll_interval_ms` | `100` | 20..=2000 |
| `scroll_max_attempts` | `10` | 1..=100 |
| `focus_max_steps` | `50` | 1..=500 |
| `agent_start_timeout_ms` | `60000` | 1000..=600000 |
| `command_grace_ms` | `2000` | 0..=30000 |
| `reporters` | `[console, junit, json]` | subset of `console, junit, json, html` |

`timeout_default_ms` MUST be in `[100, 600000]`. `report_output` parent MUST exist or be creatable.

### 4.2 `impresario.devices.config`

```yaml
devices:
  - name: living_room_tv
    platform: webos
    target_endpoint: ws://192.168.1.50:9998
    app_url: http://localhost:8080/index.html   # optional
  - name: flagship_android
    platform: android
    target_endpoint: emulator-5554
    app_id: com.example.shop
  - name: shield_tv
    platform: androidtv
    target_endpoint: 192.168.1.60:5555
    app_id: com.example.shop.tv
  - name: test_iphone
    platform: ios
    target_endpoint: 00008110-001A2345021E001E
    app_id: com.example.shop
  - name: apple_tv
    platform: tvos
    target_endpoint: 00008110-001B3456021E002F
    agent_address: 192.168.1.70:50062
    app_id: com.example.shop.tv
```

Per-device fields:

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | unique, `^[a-z0-9][a-z0-9_-]{0,62}$` |
| `platform` | yes | closed enum, see section 1 |
| `target_endpoint` | yes | validated per platform (below) |
| `app_id` | android, androidtv, ios, tvos | package name / bundle id of the app under test |
| `app_url` | no (web, webos, tizen) | URL to navigate to on launch/reset; if absent, the existing page is attached and reset = reload |
| `agent_address` | no | `host:port` of the executor. Defaults: connector `127.0.0.1:50051`; Android `127.0.0.1:<auto forwarded>`; iOS `127.0.0.1:<auto forwarded>`; **tvOS: required unless Bonjour discovery is enabled** |
| `connector` | no (web, webos, tizen) | alias of `agent_address` for readability |
| `options` | no | string map of platform-specific options (section 4.3) |

`target_endpoint` validation:
- `web`: the literal `launch` or `launch:headless` (connector starts Chromium), or a `ws://`/`wss://` URL.
- `webos`, `tizen`: `ws://` or `wss://` URL with host and port. Path optional: `ws://host:port` (connector resolves the page via `http://host:port/json/list`) or a full `.../devtools/page/<id>`.
- `android`, `androidtv`: adb serial, regex `^[A-Za-z0-9._:-]{4,64}$` (covers `emulator-5554`, `192.168.1.60:5555`, hardware serials).
- `ios`, `tvos`: UDID: `^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{16}$` (modern devices), or 40 hex characters (legacy), or an RFC 4122 UUID (simulators).

### 4.3 Device `options` (all optional)

| Option | Platforms | Values / meaning |
|---|---|---|
| `cdp_backend` | web, webos, tizen | `raw` (default) or `chromiumoxide` |
| `key_dispatch` | web, webos, tizen | `cdp` (default, `Input.dispatchKeyEvent`) or `js` (synthetic `KeyboardEvent`); use `js` if the TV ignores synthetic input events |
| `page_match` | webos, tizen | substring matched against page `url` or `title` when `/json/list` returns several pages |
| `clear_app_data` | android, androidtv | `true` runs `pm clear <app_id>` before each (re)launch |
| `agent_port` | android, androidtv, ios, tvos | device-side port of the agent (defaults: Android 50061, iOS 50062, tvOS 50062) |
| `animations_disabled` | ios, tvos | `true` passes launch arguments to reduce animations (mitigates quiescence stalls) |

### 4.4 Load order and `validate`
`impresario validate [PATH...]` loads both configs, parses every `.scene` and `.play` under the configured directories (or given paths), runs semantic validation against **every** configured device, and exits 0/2. It touches no device. `impresario init` output MUST pass `validate`.

---

## 5. The DSL

### 5.1 Lexical rules
- UTF-8; LF or CRLF; a UTF-8 BOM is tolerated and stripped.
- Indentation: spaces only (a tab in indentation is `IMP-DSL-001`). The first indented line in a file fixes the indent unit; every indentation MUST be a multiple of it (`IMP-DSL-002`).
- Comments: a line whose first non-space characters are `//`. Blank lines are ignored.
- Strings: single or double quoted. Escapes: `\\ \' \" \n \t \u{XXXX}`. Unterminated string is `IMP-DSL-003`. A string may not span lines.
- Interpolation, **inside strings only**: `${env.NAME}` (process environment) and `${var.NAME}` (play variable, section 5.3). A missing variable is `IMP-DSL-014`, reported statically for `var` and at plan time for `env` (the error names the variable, never a value).
- Durations: integer + unit `ms`, `s`, or `m` (`500ms`, `2s`, `1m`). Range `1ms..=10m`.
- Identifiers: `[A-Za-z][A-Za-z0-9_]*` (scene, piece, variable names). UpperCamelCase is recommended (lint `IMP-DSL-020` as a warning for scenes and pieces).
- Free text (Play, Act, Stage names; descriptions): rest of the line, trimmed, 1..=200 chars, no leading/trailing whitespace.
- Keywords are case-sensitive: `Scene Platform Pieces Play Description Var Act Tags Platforms Stage Timeout Action Check`.

### 5.2 Scene file

```
Scene: LoginScreen
Platform: cross-platform

Pieces:
    LoginTitle:    web="h1.title",  android="id:login_title",  ios="accessibilityId:login_title"
    UsernameInput: web="#username", android="id:username",     ios="accessibilityId:username"
    PasswordInput: web="#password", android="id:password",     ios="accessibilityId:password"
    SubmitButton:  web="button[type='submit']", android="id:login_btn", ios="accessibilityId:login"
```

Rules:
- Piece lines MUST be indented under `Pieces:` (the v1 sketch showed them unindented; v2 requires indentation for consistency with the play grammar).
- `Scene:` value is the scene's identity (the filename is irrelevant; mismatch with the file stem is warning `IMP-DSL-021`). Scene names are unique across the workspace (`IMP-DSL-006`); piece names are unique within a scene (`IMP-DSL-007`).
- `Platform:` is `cross-platform` (applies to all devices) or a comma list of device platform names. A scene applies to a device iff it is `cross-platform` or lists the device's platform **exactly** (no implicit family matching).
- A piece line is `Name: key="selector" {, key="selector"} [, index=N]`. Keys are the platform names (D-07). `index` is a 0-based integer applied when a selector matches several elements (default 0). A key listed twice is an error.
- Each selector is a double-quoted string (escape `\"` and `\\`). The value must satisfy the selector grammar of its key (below), otherwise `IMP-DSL-011`.

### 5.3 Selector grammar by key

Prefix parsing rule: a prefix is recognised only if the value starts with exactly one of the listed lowercase/camelCase tokens followed by `:`. Anything else is a bare value.

| Key(s) | Syntax | Wire strategy |
|---|---|---|
| `web`, `webos`, `tizen` | bare value = CSS; `css:<css>`; `xpath:<xpath>`; `text:<exact visible text>`; `textContains:<substring>` | `CSS`, `XPATH`, `TEXT`, `TEXT_CONTAINS` |
| `android`, `androidtv` | prefix **required**: `id:<resource>`, `text:<exact>`, `textContains:<s>`, `desc:<content-desc>`, `class:<fqcn>` | `ANDROID_ID`, `TEXT`, `TEXT_CONTAINS`, `ANDROID_DESC`, `ANDROID_CLASS` |
| `ios`, `tvos` | prefix **required**: `accessibilityId:<id>`, `label:<exact>`, `labelContains:<s>`, `predicate:<NSPredicate format>`, `type:<ElementType>` | `ACCESSIBILITY_ID`, `LABEL`, `LABEL_CONTAINS`, `PREDICATE`, `IOS_TYPE` |

Notes: Android `id:` accepts a full resource id (`com.pkg:id/name`) or a short name, which the agent expands with the device's `app_id` package. XPath on Android is NOT supported (UiAutomator2 has none). `type:` accepts XCUIElement.ElementType names in lowerCamel (`button`, `textField`, `secureTextField`, `staticText`, `cell`, `image`, `switch`, `other`, ...); an unknown name is `IMP-DSL-011`.

### 5.4 Play file

```
Play: User Authentication Feature
Description: Validating secure user entry across platforms.
Var: USER = 'test_user_impresario'

Act: Successful Login Flow
Tags: smoke, auth
Platforms: android, ios, webos

Stage: Navigate and Login
    Timeout: 15s
    Scene: LoginScreen
        Check: LoginTitle
        Action: '${var.USER}' => UsernameInput
        Action: '${env.TEST_PASSWORD}' => PasswordInput
        Action: click => SubmitButton

Stage: Verify Outcome
    Scene: DashboardScreen
        Check: WelcomeBanner is visible timeout 20s
```

Structure and indentation:
- Depth 0: `Play`, `Description`, `Var`, `Act`, `Tags`, `Platforms`, `Stage`.
- Depth 1 (inside a Stage): `Timeout`, `Scene`.
- Depth 2 (inside a Scene block): `Action`, `Check`.
- `Tags:` and `Platforms:` MUST directly follow their `Act:` line. A `Stage:` belongs to the nearest preceding `Act:`.
- One `Play:` per file, first non-comment line. An Act has at least one Stage; a Stage at least one Scene block; a Scene block at least one step. Act names unique within the play, Stage names unique within the Act (`IMP-DSL-016`).
- `Tags`: comma list of `[a-z0-9_-]+`. `Platforms`: comma list of device platform names; if present, the Act runs only on devices of those platforms and is reported `skipped` elsewhere.
- `Var: NAME = '<string>'` defines a play variable. CLI `--var NAME=value` overrides it. `${var.X}` for an undefined X is `IMP-DSL-014`.
- A Scene block's `Scene:` MUST name an existing scene (`IMP-DSL-008`); pieces used in its steps MUST exist in that scene (`IMP-DSL-009`, with "did you mean"). Pieces are not cross-scene; the same scene may appear in several Stages.
- A Scene block is only valid on devices to which the scene applies (`IMP-DSL-017` is raised at plan time per device, not at parse time).

### 5.5 Full grammar (EBNF; `INDENT`/`DEDENT` are produced by the scanner; `EOL` ends a logical line)

```ebnf
scene-file   = scene-line , platform-line , "Pieces:" , EOL , INDENT , piece-line , { piece-line } , DEDENT ;
scene-line   = "Scene:" , ident , EOL ;
platform-line= "Platform:" , ( "cross-platform" | platform , { "," , platform } ) , EOL ;
piece-line   = ident , ":" , sel-pair , { "," , sel-pair } , [ "," , "index" , "=" , integer ] , EOL ;
sel-pair     = platform , "=" , dq-string ;
platform     = "web" | "webos" | "tizen" | "android" | "androidtv" | "ios" | "tvos" ;

play-file    = "Play:" , text , EOL , [ "Description:" , text , EOL ] , { var-decl } , act , { act } ;
var-decl     = "Var:" , ident , "=" , string , EOL ;
act          = "Act:" , text , EOL , [ "Tags:" , tag , { "," , tag } , EOL ]
             , [ "Platforms:" , platform , { "," , platform } , EOL ] , stage , { stage } ;
stage        = "Stage:" , text , EOL , INDENT , [ "Timeout:" , duration , EOL ] , scene-block , { scene-block } , DEDENT ;
scene-block  = "Scene:" , ident , EOL , INDENT , step , { step } , DEDENT ;
step         = action-line | check-line ;

action-line  = "Action:" , action , EOL ;
action       = string , "=>" , piece , [ tmo ]                        (* shorthand for: type *)
             | "type"      , string , "=>" , piece , [ tmo ]          (* REPLACE content *)
             | "append"    , string , "=>" , piece , [ tmo ]          (* keep content, add at end *)
             | "clear"     , "=>" , piece , [ tmo ]
             | "click"     , "=>" , piece , [ tmo ]
             | "longpress" , [ duration ] , "=>" , piece , [ tmo ]    (* default 1000ms *)
             | "focus"     , "=>" , piece , [ tmo ]
             | "select"    , string , "=>" , piece , [ tmo ]
             | "scroll"    , direction , [ integer ] , [ "=>" , piece ] , [ tmo ]   (* count default 1 *)
             | "scrollto"  , "=>" , piece , [ tmo ]
             | "swipe"     , direction , [ "=>" , piece ] , [ tmo ]
             | "key"       , keyname , [ integer ]                    (* repeat count, default 1 *)
             | "back"
             | "wait"      , duration
             | "screenshot", string
             | "app"       , ( "launch" | "terminate" | "restart" | "background" ) ;
check-line   = "Check:" , piece , [ cond ] , [ tmo ] , EOL ;
cond         = "is" , ( "visible" | "hidden" | "enabled" | "disabled" | "focused" )
             | "text" , string | "textContains" , string | "count" , integer ;
tmo          = "timeout" , duration ;
direction    = "up" | "down" | "left" | "right" ;
keyname      = "ok"|"enter"|"back"|"up"|"down"|"left"|"right"|"home"|"menu"|"play"|"pause"|"playpause"
             | "rewind"|"fastforward"|"red"|"green"|"yellow"|"blue"|"backspace" ;
piece        = ident ;
```

`Check: P` with no condition means `is visible`. `tmo` is not allowed on `key`, `back`, `wait`, `screenshot`, `app` (`IMP-DSL-022`).

### 5.6 Step semantics (normative)

**Timeout precedence** (highest first): step-level `timeout` > Stage `Timeout:` > CLI `--timeout` > `timeout_default_ms`. The timeout is the maximum time the executor polls for the element/condition (poll interval `poll_interval_ms`).

**Direction semantics.** `scroll <dir>` names the direction in which you want to reach more content: `scroll down` reveals content further down (equivalent to a finger moving up). `swipe <dir>` names the direction of finger movement: `swipe up` moves the finger up. These differ on purpose and are documented in `docs/DSL.md`.

**Text entry (the three verbs are distinct and exact):**
| Form | Final field content |
|---|---|
| `'abc' => P` (= `type 'abc' => P`) | exactly `abc`: focus, clear existing content, enter `abc` |
| `append 'abc' => P` | previous content + `abc`: focus, keep content, add `abc` at the end |
| `clear => P` | empty string |

Executors MUST implement `type` as an atomic "replace" and MUST fail with `NOT_INTERACTABLE` (not silently succeed) when the element is not editable. After `type`/`append`/`clear` on web, the executor fires `input` and `change` events so frameworks (React/Vue/Angular) observe the value.

**Click/tap.** Waits (up to the timeout) for the element to exist, be visible, and be enabled; then taps its center (touch/pointer platforms) or focuses it and presses OK/Select (TV platforms; section 6.3). If the element exists but is disabled/obscured at timeout: `NOT_INTERACTABLE`; if it never exists: `ELEMENT_NOT_FOUND`.

**Scroll.** `scroll <dir> [n] [=> P]` performs `n` scroll steps (default 1; one step ≈ 80% of the container's visible extent on touch platforms, one D-pad press or one page-unit on TV platforms) on container `P`, or on the primary scrollable of the screen if omitted. `scrollto => P` repeats scroll steps (down first, then up if the element was not found and the container reached its end) up to `scroll_max_attempts` until `P` is visible; failure is `ELEMENT_NOT_FOUND` with `details.scroll_attempts`.

**Select.** `select 'Option' => P`: web = set the `<select>` value whose text equals the option, then dispatch events; Android = click `P` then click the element with that text; iOS = picker wheel `adjust(toPickerWheelValue:)` or tap the menu item with that label. Option not found: `ELEMENT_NOT_FOUND` (`details.option`).

**Checks.** Semantics hold on every platform:
- `visible`: element exists and is displayed (platform definitions in 6.4). `hidden`: element does not exist or is not displayed. `enabled`/`disabled`: element exists and the flag matches. `focused`: element exists and has focus.
- `text 'x'`: element text equals `x` exactly (after the platform text rule in 6.4). `textContains 'x'`: substring. `count N`: the selector matches exactly N elements (N may be 0).
- A check passes as soon as the condition is true; it fails with `CHECK_FAILED` when the timeout elapses. The failure `details` contain `expected`, `observed`, and `matched_count`. Checks never return `ELEMENT_NOT_FOUND`.

**Master-side steps (never sent on the wire):** `wait` and the `screenshot 'name'` file naming. `screenshot` sends a `Screenshot` command; the master writes the returned PNG to `<report_output>/screenshots/<run-id>/<device>/<sanitized-name>.png` (name sanitized to `[A-Za-z0-9._-]`, max 80 chars).

**App control.** `app launch|terminate|restart` map to `AppControl`. Support per platform is in 6.1.

**Secrets.** Reports, logs, and the console MUST show the **source text** of a step as written (`'${env.TEST_PASSWORD}'`), never the resolved string. Any observed text equal to a resolved `${env.*}` value MUST be replaced by `***` in reports and logs.

### 5.7 Parser deliverables
- Typed AST with a `Span { file, line, col, len }` on every node; `impresario parse <file> --json` dumps it.
- Recovering parser: collects as many errors as possible per file; never panics on any input (proptest + `cargo-fuzz` target).
- Semantic validation pass: scene/piece/variable resolution, selector grammar, duplicates, platform applicability, unsupported-action detection per device platform (6.1) at plan time.
- Diagnostics render with `miette` (file, line, caret, hint). Catalog in `docs/ERRORS.md`:
  `IMP-CFG-001 unknown key`, `002 missing field`, `003 invalid value`, `004 duplicate device`, `005 bad endpoint for platform`;
  `IMP-DSL-001 tab indentation`, `002 inconsistent indent`, `003 unterminated string`, `004 unknown keyword`, `005 missing/misplaced header`, `006 duplicate scene`, `007 duplicate piece`, `008 unknown scene`, `009 unknown piece`, `010 unknown action`, `011 invalid selector`, `012 missing selector for platform`, `013 unsupported action on platform`, `014 undefined variable`, `015 invalid duration`, `016 duplicate act/stage`, `017 scene not applicable to device`, `018 unknown key name`, `019 lint: fixed wait`, `020 lint: naming`, `021 lint: scene/file mismatch`, `022 timeout not allowed here`.
- Tests: `insta` snapshots for the example files and for AST JSON; at least 60 negative cases (one per code above plus edge cases); round-trip tests for durations and escapes.

---

## 6. Platform behavior (normative)

Legend: ✅ full · ◐ limited (see note) · ✖ unsupported (`UNSUPPORTED_ACTION`; the planner detects this statically and reports `IMP-DSL-013` before any device is touched).

### 6.1 Action support

| Action | web | webos | tizen | android | androidtv | ios | tvos |
|---|---|---|---|---|---|---|---|
| `click` | ✅ | ✅ focus+OK | ✅ focus+OK | ✅ | ✅ focus+OK | ✅ | ✅ focus+select |
| `type` / `append` / `clear` | ✅ | ✅ programmatic | ✅ programmatic | ✅ | ✅ | ✅ | ◐ requires focus + keyboard |
| `longpress` | ◐ mouse hold | ◐ OK held | ◐ OK held | ✅ | ◐ injected OK hold | ✅ | ✅ `press(.select, forDuration:)` |
| `focus` | ✅ | ✅ | ✅ | ✖ | ◐ D-pad only | ✖ | ◐ D-pad only |
| `select` | ✅ native `<select>` | ✅ | ✅ | ◐ | ◐ | ◐ | ✖ |
| `scroll` | ✅ | ✅ | ✅ | ✅ | ◐ D-pad | ✅ | ◐ D-pad |
| `scrollto` | ✅ | ✅ | ✅ | ✅ | ◐ same as `focus` | ✅ | ◐ same as `focus` |
| `swipe` | ✖ | ✖ | ✖ | ✅ | ✖ | ✅ | ✖ |
| `back` | ✅ history | ✅ back key | ✅ back key | ✅ | ✅ | ◐ nav-bar back button, else edge swipe | ✅ menu |
| `key <k>` | table 6.2 | 6.2 | 6.2 | 6.2 | 6.2 | 6.2 | 6.2 |
| `screenshot` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `app launch` | ✅ navigate | ✅ navigate | ✅ navigate | ✅ | ✅ | ✅ | ✅ |
| `app terminate` | ✖ | ✖ | ✖ | ✅ | ✅ | ✅ | ✅ |
| `app restart` | ✅ reload/navigate | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `app background` | ✖ | ✖ | ✖ | ✅ | ✅ | ✅ | ✅ |

| Check | web | webos | tizen | android | androidtv | ios | tvos |
|---|---|---|---|---|---|---|---|
| `visible` `hidden` `enabled` `disabled` `text` `textContains` `count` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `focused` | ✅ | ✅ | ✅ | ✅ | ✅ | ✖ | ✅ |

Notes: `focus` is ✖ on touch Android/iOS because out-of-process UI automation cannot move programmatic focus. `swipe` is ✖ on TV platforms (no touch). On TV platforms `scroll` presses the D-pad in the given direction `n` times (container focused first when `=> P` is given).

### 6.2 Remote/hardware keys

Key names are the DSL names. Values in cells are the dispatched code/constant. ✖ means unsupported (planner error). Verify every code against device documentation and keep the table in one source file per executor with unit tests.

| Key | web (DOM keyCode) | webos | tizen | android (KeyEvent) | androidtv | ios | tvos (`XCUIRemote.Button`) |
|---|---|---|---|---|---|---|---|
| `ok` | 13 | 13 | 13 | ✖ | 23 DPAD_CENTER | ✖ | `.select` |
| `enter` | 13 | 13 | 13 | 66 ENTER | 66 | ✖ | ✖ |
| `back` | ✖ (use `back`) | 461 | 10009 | 4 BACK | 4 | ✖ (use `back`) | `.menu` |
| `up/down/left/right` | 38/40/37/39 | same | same | 19/20/21/22 | same | ✖ | `.up/.down/.left/.right` |
| `home` | ✖ | ✖ | ✖ | 3 HOME | 3 | `XCUIDevice.press(.home)` | `.home` |
| `menu` | ✖ | ✖ | ✖ | 82 MENU | 82 | ✖ | ✖ |
| `play` | ✖ | 415 | 415 | 126 | 126 | ✖ | ✖ |
| `pause` | ✖ | 19 | 19 | 127 | 127 | ✖ | ✖ |
| `playpause` | ✖ | ✖ | ✖ | 85 | 85 | ✖ | `.playPause` |
| `rewind` | ✖ | 412 | 412 | 89 | 89 | ✖ | ✖ |
| `fastforward` | ✖ | 417 | 417 | 90 | 90 | ✖ | ✖ |
| `red/green/yellow/blue` | ✖ | 403/404/405/406 | 403/404/405/406 | ✖ | 183/184/185/186 | ✖ | ✖ |
| `backspace` | 8 | 8 | 8 | 67 DEL | 67 | ✖ | ✖ |

Tizen note: the app under test must have registered media/color keys with `tizen.tvinputdevice.registerKey` for them to reach the page; document this in `docs/DEVICES.md`. On `web`, `key` dispatches keyboard events to the focused element.

### 6.3 TV focus model

TV UIs are focus driven. Definitions:
- `click => P` on a TV platform = ensure `P` has focus, then press OK/Select.
- **Programmatic focus** is available only on `web`, `webos`, `tizen` (`element.focus()` via CDP; then verified with `document.activeElement`).
- On `androidtv` and `tvos`, focus is moved **only by D-pad presses** (no programmatic focus).
- If programmatic focus is unavailable or fails to take effect on web TVs, fall back to D-pad traversal.

**D-pad traversal algorithm** (shared logic, implemented once per executor language behind a `FocusProbe` interface: `currentFocusFrame()`, `targetFrame()`, `press(direction)`, `isTargetFocused()`; unit-testable with a fake):
1. Locate the target (wait up to the timeout). If it is already focused, done.
2. Loop up to `focus_max_steps`:
   a. Read the focused element's frame. If unreadable, press `ok`-neutral navigation `down` once and retry (counts as a step).
   b. Compute `dx, dy` between the target center and the focused center. Choose the direction along the dominant axis (ties prefer vertical).
   c. Press it. Re-read focus. If the focused frame did not change, try the other axis once; if that also does not change focus, add a `blocked` entry to the trail.
   d. If the target is now focused, done.
3. Loop guard: keep a set of `(focusedFrameHash, direction)` pairs; seeing a pair twice means a cycle.
4. Failure → `FOCUS_UNREACHABLE` with `details.focus_trail` (JSON array of `{step, direction, focused_label, frame}`), `details.steps`, and `details.reason` in `budget|cycle|blocked`.

### 6.4 Visibility, enabled, text definitions

| Platform | `visible` | `enabled` | element text (for `text`, `textContains`) |
|---|---|---|---|
| web, webos, tizen | element exists, computed `display != none`, `visibility == visible`, `opacity > 0`, bounding rect has non-zero width and height, and intersects the viewport | not `disabled` property and `aria-disabled != "true"` | `innerText` trimmed; for `input/textarea/select`, the `value` |
| android, androidtv | element exists and `visibleBounds` has non-zero area | `isEnabled` | `UiObject2.text` (empty string if null) |
| ios, tvos | `exists` and frame non-empty and intersects the app window | `isEnabled` | `value as? String` if non-empty and not equal to `placeholderValue`, else `label` (placeholder text of an empty field MUST read as `""`) |

`visible` for a click additionally requires the element to be hit-testable (web: `elementFromPoint` at the center is the element or a descendant; iOS: `isHittable`).

---

## 7. Wire protocol

### 7.1 `proto/impresario/v1/impresario.proto` (complete contract; extend only backward-compatibly)

```proto
syntax = "proto3";
package impresario.v1;

service Agent {
  rpc Handshake(HandshakeRequest) returns (HandshakeResponse);
  rpc Session(stream Command) returns (stream Event);
  rpc CaptureScreenshot(ScreenshotRequest) returns (ScreenshotResponse);   // out-of-band; may overlap an in-flight command
  rpc StreamLogs(LogRequest) returns (stream LogEntry);
  rpc Shutdown(ShutdownRequest) returns (ShutdownResponse);
}

enum Platform { PLATFORM_UNSPECIFIED = 0; WEB = 1; WEBOS = 2; TIZEN = 3; ANDROID = 4; ANDROID_TV = 5; IOS = 6; TVOS = 7; }

message HandshakeRequest {
  string protocol_version = 1;           // "1.0.0"
  string master_version = 2;
  string device_name = 3;
  Platform platform = 4;
  string target_endpoint = 5;
  string app_id = 6;
  string app_url = 7;
  map<string, string> options = 8;
  string auth_token = 9;                 // required when the executor is configured with a token
}
message HandshakeResponse {
  bool accepted = 1;
  string session_id = 2;
  string agent_version = 3;
  string protocol_version = 4;
  repeated Capability capabilities = 5;
  DeviceInfo device = 6;
  ErrorInfo error = 7;                   // set iff !accepted
}
enum Capability {
  CAPABILITY_UNSPECIFIED = 0; MULTI_SESSION = 1; LOGS = 2; SCREENSHOT = 3;
  TOUCH_GESTURES = 4; DPAD_FOCUS = 5; PROGRAMMATIC_FOCUS = 6; KEY_EVENTS = 7; APP_CONTROL = 8;
}
message DeviceInfo {
  string model = 1; string os_name = 2; string os_version = 3;
  uint32 screen_width_px = 4; uint32 screen_height_px = 5; float pixel_ratio = 6;
  string runtime = 7;                    // e.g. "Chromium/79", "UiAutomator2", "XCUITest"
}

enum SelectorStrategy {
  SELECTOR_STRATEGY_UNSPECIFIED = 0;
  CSS = 1; XPATH = 2; TEXT = 3; TEXT_CONTAINS = 4;
  ANDROID_ID = 5; ANDROID_DESC = 6; ANDROID_CLASS = 7;
  ACCESSIBILITY_ID = 8; LABEL = 9; LABEL_CONTAINS = 10; PREDICATE = 11; IOS_TYPE = 12;
}
message Selector { SelectorStrategy strategy = 1; string value = 2; uint32 index = 3; }

enum ActionType {
  ACTION_TYPE_UNSPECIFIED = 0;
  CLICK = 1; TYPE = 2; APPEND = 3; CLEAR = 4; LONG_PRESS = 5; FOCUS = 6;
  SELECT = 7; SCROLL = 8; SCROLL_TO = 9; SWIPE = 10; BACK = 11;
}
// TYPE replaces content (clear, then enter). APPEND keeps content. CLEAR leaves "".
enum Direction { DIRECTION_UNSPECIFIED = 0; UP = 1; DOWN = 2; LEFT = 3; RIGHT = 4; }

message RunAction {
  ActionType type = 1;
  Selector selector = 2;                 // absent only for SCROLL/SWIPE (screen-level) and BACK
  string text = 3;                       // TYPE, APPEND, SELECT (option text)
  Direction direction = 4;               // SCROLL, SWIPE
  uint32 count = 5;                      // SCROLL steps (default 1)
  uint32 duration_ms = 6;                // LONG_PRESS (default 1000)
}

enum CheckKind {
  CHECK_KIND_UNSPECIFIED = 0;
  VISIBLE = 1; HIDDEN = 2; ENABLED = 3; DISABLED = 4; FOCUSED = 5;
  TEXT_EQUALS = 6; TEXT_CONTAINS_CHECK = 7; COUNT = 8;
}
message ValidateCheck {
  Selector selector = 1; CheckKind kind = 2; string expected_text = 3; uint32 expected_count = 4;
}

enum RemoteKey {
  REMOTE_KEY_UNSPECIFIED = 0;
  KEY_OK = 1; KEY_ENTER = 2; KEY_BACK = 3; KEY_UP = 4; KEY_DOWN = 5; KEY_LEFT = 6; KEY_RIGHT = 7;
  KEY_HOME = 8; KEY_MENU = 9; KEY_PLAY = 10; KEY_PAUSE = 11; KEY_PLAYPAUSE = 12;
  KEY_REWIND = 13; KEY_FASTFORWARD = 14; KEY_RED = 15; KEY_GREEN = 16; KEY_YELLOW = 17; KEY_BLUE = 18;
  KEY_BACKSPACE = 19;
}
message KeyPress { RemoteKey key = 1; uint32 repeat = 2; uint32 interval_ms = 3; }   // repeat default 1, interval default 100

message Screenshot { string label = 1; }

enum AppOp { APP_OP_UNSPECIFIED = 0; LAUNCH = 1; TERMINATE = 2; RESTART = 3; }
message AppControl { AppOp op = 1; bool clear_data = 2; }

message Cancel { string target_command_id = 1; }

message Command {
  string command_id = 1;                 // unique within the session; echoed in the result
  uint32 timeout_ms = 2;                 // element/condition wait budget enforced by the executor
  oneof body {
    RunAction run_action = 10;
    ValidateCheck validate_check = 11;
    KeyPress key_press = 12;
    Screenshot screenshot = 13;
    AppControl app_control = 14;
    Cancel cancel = 15;
  }
}

enum ErrorCode {
  ERROR_CODE_UNSPECIFIED = 0;
  ELEMENT_NOT_FOUND = 1; TIMEOUT = 2; NOT_INTERACTABLE = 3; STALE_ELEMENT = 4;
  FOCUS_UNREACHABLE = 5; UNSUPPORTED_ACTION = 6; INVALID_SELECTOR = 7; CHECK_FAILED = 8;
  APP_CRASHED = 9; CANCELLED = 10; TRANSPORT = 11; PROTOCOL = 12; INTERNAL = 13;
  ALREADY_IN_USE = 14; DEVICE_UNAVAILABLE = 15; UNAUTHENTICATED = 16;
}
message ErrorInfo { ErrorCode code = 1; string message = 2; map<string, string> details = 3; }

message CommandResult {
  string command_id = 1;
  bool ok = 2;
  uint32 duration_ms = 3;
  ErrorInfo error = 4;                   // set iff !ok
  bytes screenshot_png = 5;              // set for Screenshot commands
  string observed_text = 6;
  uint32 observed_count = 7;
}
message Heartbeat { uint64 seq = 1; uint64 unix_ms = 2; string current_command_id = 3; }
message SessionClosed { string reason = 1; ErrorInfo error = 2; }
message Event { oneof body { CommandResult result = 1; Heartbeat heartbeat = 2; SessionClosed closed = 3; } }

message ScreenshotRequest {}
message ScreenshotResponse { bytes png = 1; uint32 width = 2; uint32 height = 3; ErrorInfo error = 4; }

enum LogLevel { LOG_LEVEL_UNSPECIFIED = 0; DEBUG = 1; INFO = 2; WARN = 3; ERROR = 4; }
message LogRequest { LogLevel min_level = 1; }
message LogEntry { int64 unix_ms = 1; LogLevel level = 2; string source = 3; string message = 4; }  // source: "agent", "console", ...

message ShutdownRequest { bool terminate_app = 1; }
message ShutdownResponse {}
```

### 7.2 Protocol rules (normative)
1. **Session identity.** `Handshake` returns `session_id`. Every later RPC MUST carry it in gRPC metadata `x-impresario-session-id`. When a token is configured, every RPC MUST carry `x-impresario-token`; missing/invalid → `UNAUTHENTICATED`.
2. **Compatibility.** Major versions must match, else `accepted=false` with `PROTOCOL`. If the master's minor is greater than the executor's, the master MUST NOT use features outside `capabilities`.
3. **Single-session executors** (all native agents) reject a second `Handshake` while a session is active: `ALREADY_IN_USE`.
4. **Ordering.** Commands in a `Session` stream are executed strictly in order, one at a time. Exactly **one** `CommandResult` is returned per `Command`, with the same `command_id`, in command order, including failures and cancellations.
5. **Cancel.** `Cancel` is processed immediately (even while another command runs): the target command finishes as soon as practical with `CANCELLED`; cancel of an unknown id is a `PROTOCOL` error result for the cancel command itself.
6. **No crash on bad input.** Executors never terminate the stream because of a bad command; they return `ok=false` with `ErrorInfo`. Unknown oneof bodies → `UNSUPPORTED_ACTION`. Unspecified enums → `PROTOCOL`.
7. **Deadlines.** `timeout_ms` is the executor's wait budget. The master enforces `timeout_ms + command_grace_ms`; on overrun it sends `Cancel`, waits 2 s, then declares `TIMEOUT`, tears down the session, and fails the Act. Executors SHOULD answer `TIMEOUT` themselves before the master's deadline.
8. **Heartbeat.** Executors emit `Heartbeat` every 5 s on the `Session` stream. Three missed heartbeats (15 s) = `TRANSPORT` failure (D-12: no resume).
9. **Screenshots.** In-order screenshots use the `Screenshot` command. `CaptureScreenshot` RPC exists for failure capture when the session is hung; it MAY overlap an in-flight command and MUST NOT disturb it.
10. **Logs.** `StreamLogs` streams executor-origin logs (`source`): connector → page console/log events; agents → agent-internal log lines. Host-side collectors (adb logcat, iOS syslog) are run by the master transports (section 9).
11. **Limits.** gRPC max message size 16 MiB both directions (screenshots). HTTP/2 keepalive 10 s.
12. **Text safety.** `text` fields are UTF-8. Executors MUST NOT log `RunAction.text` at INFO or above.

### 7.3 Lifecycle (sequence, per device)
1. `DeviceTransport.prepare()` (adb forward + start instrumentation; iproxy + start xcodebuild runner; ensure connector).
2. Connect to the executor; retry with exponential backoff (200 ms → 2 s) up to `agent_start_timeout_ms`.
3. `Handshake`; verify protocol/capabilities; open `Session` stream; start `StreamLogs` (best effort).
4. For each Act, per `app_reset`: `AppControl{RESTART, clear_data}` (the first Act of the run issues `LAUNCH`).
5. Execute steps; on failure and `screenshot_on_failure`, send a `Screenshot` command next in order (or `CaptureScreenshot` if the session is hung).
6. After the last Play: `Shutdown`, close the stream, `DeviceTransport.teardown()`.

---

## 8. Master (`impresario-master`)

### 8.1 CLI

```
impresario init [--force]
impresario validate [PATH...]
impresario parse <file> [--json]
impresario devices list
impresario devices check [NAME...]      # transport prepare + handshake + DeviceInfo, then teardown
impresario run [--play <file|dir|glob>...] --device <name>... 
               [--tag <t>...] [--exclude-tag <t>...] [--act <exact name>...]
               [--var K=V...] [--timeout <ms>] [--retries <n>] [--reporter <list>] [--fail-fast]
impresario lsp
global: --config <path> --devices-config <path> --workspace <dir> -v/-q --no-color
```

- `--play` defaults to `plays_dir`. `--tag` repeated = OR; `--exclude-tag` wins over `--tag`; `--act` filters by exact name.
- `--device` may repeat or be `all`. Each selected device runs every selected play.
- `impresario init` writes exactly: `impresario.config`, `impresario.devices.config` (the five example devices from 4.2, plus a commented `web` example), `plays/login.play` (5.4 without the optional `Var` and `Platforms` lines is also acceptable but the version shown in 5.4 is preferred), `scenes/LoginScreen.scene` (5.2), `scenes/DashboardScreen.scene` (below), and appends `reports/` to `.gitignore` (creating it if needed). It refuses to overwrite existing files without `--force` and prints each created path. The example set MUST be self-consistent: every scene/piece referenced by the example play exists, and `validate` passes.

```
Scene: DashboardScreen
Platform: cross-platform

Pieces:
    WelcomeBanner: web="h2.welcome", android="id:welcome_banner", ios="accessibilityId:welcome_banner"
    LogoutButton:  web="#logout",    android="id:logout_btn",      ios="accessibilityId:logout"
```

### 8.2 Planner (static, before any device is touched)
For each selected device, compile each selected Play into an `ExecutionPlan`:
`Vec<PlannedAct { name, tags, steps: Vec<PlannedStep> }>`; `PlannedStep { span, source_text, kind, timeout, command: Option<proto-free Command> }`.
Compilation: filter Acts (platform, tags, names) → apply scene applicability → resolve each piece with the fallback chain (D-07) → parse selector grammar → interpolate variables (CLI vars > play vars; `env` read now; secrets recorded for redaction) → check action/key support against 6.1/6.2 → compute effective timeout (5.6). Any error aborts the run with exit code 2 and lists **all** errors.

### 8.3 Execution
- **Concurrency.** One task per device, bounded by `parallel_devices` (semaphore), all in a `JoinSet`. Within a device, Plays and Acts run strictly sequentially; steps run strictly in order. A device is never driven by two sessions at once.
- **Outcomes.** Step failure fails its Stage and Act (remaining steps of the Act are `skipped`); the next Act still runs unless `--fail-fast`. `--fail-fast` sets a global `CancellationToken`: no new Acts start; in-flight steps finish their current command; other devices abort at the next command boundary and are marked `aborted`.
- **Retries.** `--retries` overrides config `retries`. A failed Act is retried from its first Stage after `AppControl{RESTART}` (+ `clear_data` when configured). Every attempt is recorded; an Act that passes after a failure is marked `flaky: true` and counts as passed.
- **App reset.** Per `app_reset`: before every Act (`per_act`), before the first Act of each Play (`per_play`), or never.
- **Failure capture.** On a failing step: record error code/message/details, request a screenshot (if enabled), attach the last 200 log entries.
- **Shutdown.** First Ctrl-C: stop scheduling, `Cancel` in-flight commands, `Shutdown` executors, tear down transports, write reports (marked `interrupted`), exit 130. Second Ctrl-C: exit 130 immediately.
- **Exit codes.** `0` all passed (skipped allowed) · `1` any test failure · `2` config/DSL/planning error · `3` infrastructure/device error (no handshake, transport lost before any step result) · `130` interrupted.

### 8.4 Reporters (`report_output/<run-id>/`)
- **console**: colored tree `Play > Act > Stage > step` with durations, step source text, failures with code/message, and a final summary.
- **junit** (`junit.xml`): one `testsuite` per device+play, one `testcase` per Act (attempt count in properties).
- **json** (`report.json`, schema in Appendix B).
- **html** (`report.html`): self-contained (inline CSS, base64 screenshots).
- Screenshots in `screenshots/<device>/`. `run-id` is a timestamp + short random suffix.

### 8.5 Logging
`tracing` with `RUST_LOG`/`-v`. In `lsp` mode, logs go to **stderr or a file only** (stdout carries the protocol). Secrets are redacted (5.6).

---

## 9. Device transports (`impresario-clients`)

- `AgentClient`: tonic client wrapper: `connect(addr)`, `handshake(req)`, `open_session()`, `send(Command) -> CommandResult` (awaits the matching result; enforces the master-side deadline of 7.2.7), `cancel(id)`, `capture_screenshot()`, `stream_logs()`, `shutdown()`. Converts `core` types to proto types (D-16).
- `trait DeviceTransport { async fn prepare(&mut self, dev: &Device) -> Result<Endpoint>; async fn teardown(&mut self) -> Result<()>; fn log_sources(&self) -> Vec<LogSource>; }`, one implementation per family:
  - `ConnectorTransport` (web, webos, tizen): if `agent_address` is loopback and nothing listens, spawn `impresario-connector serve` as a child (killed on drop); otherwise just connect.
  - `AdbTransport` (android, androidtv): uses argument arrays (never shell strings). Steps: verify `adb devices` lists the serial and it is `device`; install/refresh the agent APKs when their version hash differs; (optional) `pm clear`; pick a free local port; `adb -s S forward tcp:<local> tcp:<agent_port>`; spawn `adb -s S shell am instrument -w -r -e class com.impresario.agent.AgentRunner -e port <agent_port> <test-package>/androidx.test.runner.AndroidJUnitRunner` as a monitored child process (stdout captured for diagnostics); optionally spawn host-side `adb logcat` filtered to the app pid as a `LogSource`. Teardown: `Shutdown` RPC → `adb forward --remove` → kill child → `am force-stop` of the agent package.
  - `AppleTransport` (ios, tvos): macOS hosts only (otherwise `DEVICE_UNAVAILABLE` with an explanation). iOS device: `iproxy <local>:<agent_port> -u <udid>` (libimobiledevice; clear error if missing); iOS **and tvOS** simulators: direct `127.0.0.1:<agent_port>` (the UDID is an RFC 4122 UUID, so no `agent_address` is required); a physical tvOS: uses `agent_address` (network). Start the runner: `xcodebuild test-without-building -xctestrun <file> -destination 'id=<udid>' -only-testing:<bundle>/AgentRunnerTests/testServe -test-timeouts-enabled NO -parallel-testing-enabled NO` as a monitored child; wait for the port (and optionally Bonjour `_impresario._tcp`) up to `agent_start_timeout_ms`. Teardown: `Shutdown` → kill child (process group) → kill `iproxy`.
- `MockAgent` (in `impresario-clients::testing`): an in-process tonic server with scriptable behavior (delays, failures, hung commands, dropped streams) used by orchestrator integration tests.
- All child processes are kill-on-drop, have captured stderr, and every wait has a timeout.

---

## 10. Connector (`impresario-connector`)

CLI: `impresario-connector serve [--listen 127.0.0.1:50051] [--token <t>] [--allow-remote]`, `ping <ws-url>`, `pages <host:port>` (lists `/json/list` pages). Binding to a non-loopback address requires `--allow-remote` AND (`--token` or TLS flags); otherwise refuse to start.

### 10.1 Structure
`SessionManager` (map session_id → `CdpSession`) · per-session command executor (one task, strict ordering, cancellation via `CancellationToken`) · `trait CdpBackend` · `RawCdpBackend` (default) · `ChromiumoxideBackend` (feature `chromiumoxide-backend`, selected by option `cdp_backend`) · embedded helper script · key tables.

### 10.2 `CdpBackend` trait (the only surface the executor uses)
`evaluate(expr) -> serde_json::Value` (with `returnByValue`, `awaitPromise`), `dispatch_key(KeyEvent)`, `dispatch_mouse(MouseEvent)`, `insert_text(text)`, `screenshot() -> png bytes`, `navigate(url)`, `reload()`, `console_events() -> Stream`, `close()`.

### 10.3 Raw backend requirements
- WebSocket via `tokio-tungstenite`; JSON-RPC ids; request/response correlation map; per-call timeout; events dispatched to listeners. Required methods only: `Runtime.evaluate`, `Runtime.enable` (console), `Log.enable`, `Page.enable`, `Page.navigate`, `Page.reload`, `Page.captureScreenshot`, `Input.dispatchKeyEvent`, `Input.dispatchMouseEvent`, `Input.insertText`. Tolerate unknown fields and missing optional fields; never depend on newer methods such as `Target.*`. If `Page.captureScreenshot` is unavailable on an old runtime, return `UNSUPPORTED_ACTION` for screenshots with a clear message (and keep working otherwise).
- **Page resolution.** If the endpoint has no `/devtools/page/<id>`, `GET http://host:port/json/list` (fallback `/json`), keep `type == "page"` entries, apply `page_match`, and if exactly one remains use its `webSocketDebuggerUrl` (rewrite the host if the TV returns an unroutable one). Zero or several → `DEVICE_UNAVAILABLE` listing candidates.
- **Helper script** `window.__impresario`: injected idempotently before each command (check for the object; inject if missing; do NOT rely on `addScriptToEvaluateOnNewDocument`). It MUST be **ES5-only** (no arrow functions, `let/const`, template strings, `async/await`, optional chaining, spread) because old TV browsers (Chromium 38 and up) must run it; enforce with ESLint `ecmaVersion: 5` in CI. It provides: `find(strategy, value, index)`, `count`, `isVisible`, `isEnabled`, `isFocused`, `textOf`, `rectOf`, `hitTest(el)`, `scrollIntoView`, `focus`, `setValue(el, text, mode)` (`replace|append|clear`), `selectOption`, `scrollBy`.
- **Element lookup** uses the strategy from the command (CSS via `querySelectorAll`, XPath via `document.evaluate`, text via a tree walk), applies `index`, and returns a serializable descriptor, never a live node handle across calls (re-find on each poll → no stale-element class of bugs).
- **Polling.** Connector-side loop: evaluate every `poll_interval_ms` until the condition is satisfied or `timeout_ms` elapses; one evaluate per poll.
- **Click.** `web`: scroll into view, compute center, require `hitTest` success (else `NOT_INTERACTABLE` with `details.obscured_by`), dispatch `mouseMoved`, `mousePressed`, `mouseReleased`. `webos`/`tizen`: ensure focus (6.3), then dispatch `ok` keyDown+keyUp.
- **Text.** `type`: focus, `setValue(replace)` using the native value setter then dispatch `input` and `change`; if the element is `contenteditable`, select-all then `Input.insertText`. `append`/`clear` analogous. If `setValue` leaves the value different from the requested text, fall back to select-all + `Input.insertText` and re-verify; if still different → `NOT_INTERACTABLE`.
- **Keys.** Table-driven (6.2) with `key`, `code`, `windowsVirtualKeyCode`, `nativeVirtualKeyCode`; `key_dispatch: js` constructs `KeyboardEvent` with overridden `keyCode/which` and dispatches on `document.activeElement` (or `document.body`).
- **Scroll.** `scrollBy` on the container (or `window`); `scrollto` loops per 5.6; TV D-pad scroll per 6.1.
- **App control.** `LAUNCH`/`RESTART` = navigate to `app_url` if configured, otherwise reload; `TERMINATE` → `UNSUPPORTED_ACTION`.
- **`web` platform.** `launch[:headless]` spawns Chromium (`CHROME_BIN` or PATH lookup) with `--remote-debugging-port=0 --user-data-dir=<tempdir>` plus `--headless=new` when headless, parses `DevTools listening on ws://...` from stderr, and kills the process on session close.
- **Limitations (document in DEVICES.md):** only debuggable apps can be driven (webOS: apps launched in developer mode with `ares-inspect`; Tizen: apps launched in debug mode with the debug port forwarded via `sdb`). Retail/production apps cannot be inspected. Inspector ports vary; the connector never assumes 9998.

---

## 11. Android agent (`agents/android`, Kotlin)

### 11.1 Modules (D-13)
- `:agent-core` (pure Kotlin/JVM, no Android APIs): generated proto/gRPC-Kotlin stubs, gRPC server, session state, ordered command dispatcher, error mapping, `FocusNavigator`, key tables; defines `interface UiDriver`.
- `:driver-uiautomator` (Android library): `UiDriver` implementation on UiAutomator2.
- `:agent-instrumentation` (instrumentation APK, `AgentRunner` test class): wires server + driver; blocks until `Shutdown`.
- `:agent-target-stub` (trivial app): satisfies Android's requirement that an instrumentation declares a target package.
- `minSdk 26` by default (record the decision); `INTERNET` permission; `<uses-feature android:name="android.software.leanback" android:required="false"/>` and `android:name="android.hardware.touchscreen" android:required="false"` so the same APK installs on TVs.

### 11.2 Server
Listens on `127.0.0.1:<agent_port>` (instrumentation arg `port`, default 50061). **Phase 0 spike:** prove a gRPC server (`grpc-netty-shaded` or `grpc-netty`; `grpc-okhttp` is client-only) runs inside an instrumentation process on an emulator and a real device. If not, apply D-18.
A single `Channel<Command>` consumer on a single-thread dispatcher executes commands in order. `Cancel` cancels the active coroutine job; because UiAutomator calls are blocking, cancellation latency is at most one poll interval (document). Heartbeats every 5 s from a separate coroutine.

### 11.3 `UiDriver` interface (everything touching UiAutomator is behind it, so dispatcher logic is JVM-testable with a fake)
```kotlin
interface UiDriver {
  fun find(sel: SelectorSpec, timeoutMs: Long): List<ElementHandle>
  fun count(sel: SelectorSpec): Int
  fun click(e: ElementHandle); fun longClick(e: ElementHandle, ms: Long)
  fun setText(e: ElementHandle, text: String); fun text(e: ElementHandle): String
  fun clear(e: ElementHandle)
  fun isVisible(e: ElementHandle): Boolean; fun isEnabled(e: ElementHandle): Boolean; fun isFocused(e: ElementHandle): Boolean
  fun bounds(e: ElementHandle): Rect; fun focusedBounds(): Rect?
  fun pressKey(code: Int, repeat: Int, intervalMs: Long); fun pressBack()
  fun scroll(container: ElementHandle?, dir: Direction, steps: Int); fun swipe(e: ElementHandle?, dir: Direction)
  fun screenshotPng(): ByteArray
  fun launch(appId: String, tv: Boolean); fun terminate(appId: String); fun clearData(appId: String)
  fun deviceInfo(): DeviceInfo
}
```

### 11.4 Implementation rules
- **Find.** Poll every `poll_interval_ms` until found or timeout. Mapping: `ANDROID_ID` → `By.res(pkg, name)` (short names expanded with `app_id`), `TEXT` → `By.text`, `TEXT_CONTAINS` → `By.textContains`, `ANDROID_DESC` → `By.desc`, `ANDROID_CLASS` → `By.clazz`. Apply `index`. A `StaleObjectException` triggers one re-find; a second consecutive one → `STALE_ELEMENT`.
- **click.** Phone: wait visible+enabled → `click()`. TV: focus per 6.3 via `FocusNavigator` (using `By.focused(true)` and `visibleBounds`), then `pressKey(DPAD_CENTER)`.
- **type (replace) / append / clear.** `type`: click to focus (phone) or D-pad focus (TV), then `setText(text)` (this REPLACES). `append`: `setText(current + text)`. `clear`: `clear()`/`setText("")`. After the call, read the text back; mismatch → fallback (ASCII only): select all via key events, delete, send characters with `UiDevice.executeShellCommand("input text ...")` with strict escaping; non-ASCII content with no working `setText` → `UNSUPPORTED_ACTION` with an explanation. Read-back mismatch after fallback → `NOT_INTERACTABLE`.
- **longpress** phone: `UiObject2.longClick`-style gesture honoring `duration_ms`; TV: focus per 6.3, then inject a real key hold (`UiAutomation.injectInputEvent`: down → `FLAG_LONG_PRESS` repeat → up, ≥ 600 ms) so the focused view's long-press timer fires and no trailing click is delivered (I-66).
- **select.** Click the spinner/dropdown, then click the element whose text equals the option; failure → `ELEMENT_NOT_FOUND` with `details.option`.
- **scroll / swipe / scrollto.** Phone: `UiObject2.scroll/fling` or `Gestures`, with `UiScrollable` as helper; swipe via `UiObject2.swipe` or screen-level `UiDevice.swipe`. TV: D-pad presses. `scrollto` per 5.6.
- **back.** `pressBack()`.
- **Checks.** Poll until satisfied; mapping per 6.4 (`visibleBounds` non-empty; `isEnabled`; `isFocused`; text = `UiObject2.text ?: ""`; count = `findObjects().size`).
- **App control.** `launch`: shell `monkey -p <pkg> -c android.intent.category.LAUNCHER 1` (phone) or `-c android.intent.category.LEANBACK_LAUNCHER` (TV); `terminate`: `am force-stop <pkg>`; `restart` = terminate + (optional `pm clear`) + launch + wait for the package to be in the foreground (`UiDevice.wait(Until.hasObject(By.pkg(pkg)))`). Crash detection: if the target process disappears or a crash dialog (`android:id/aerr_*`) appears during a command → `APP_CRASHED`.
- **Screenshots.** `UiDevice.takeScreenshot(file)` → bytes (documented limitation: `FLAG_SECURE` windows render black).
- **Logs.** `StreamLogs` streams agent-internal logs only; app logs come from the master's host-side `adb logcat` collector (D-11).
- **Tests.** JVM tests for the dispatcher and `FocusNavigator` using a fake `UiDriver` (ordering, one-result-per-command, cancel, error mapping, focus traversal: reachable, blocked, cycle, budget). Instrumented tests (emulator) against `:agent-target-stub`, documented in `docs/DEVICES.md` with exact Gradle/adb commands.

---

## 12. Apple agent (`agents/apple`, Swift)

### 12.1 Layout (D-13, D-14)
- SwiftPM package `AgentCore` (pure Swift; no XCTest import): generated protobuf/gRPC code (**committed**; regenerate with `scripts/gen-swift.sh`, CI diff-checks), server (`grpc-swift` 1.x on SwiftNIO), ordered dispatcher (`actor`), error mapping, `FocusNavigator`, key tables, `protocol UIDriver`.
- Xcode project: `AgentHost-iOS`, `AgentHost-tvOS` (empty host apps), UI-test bundles `AgentUITests-iOS`, `AgentUITests-tvOS` containing `AgentRunnerTests` whose `testServe()` starts the server with `XCUIDriver` and **blocks until `Shutdown`** (`continueAfterFailure = false`; run with `-test-timeouts-enabled NO`).
- OS floor: iOS 16 / tvOS 16 unless Phase 0 shows otherwise. **Phase 0 spikes:** (a) a NIO gRPC server runs and accepts connections inside an XCUITest runner on a real iPhone (via `iproxy`), a simulator, and a real Apple TV (via direct network); (b) local-network permission behavior on iOS/tvOS. Record results; if the server cannot run, apply D-18.

### 12.2 `UIDriver` protocol
```swift
protocol UIDriver {
  func find(_ s: SelectorSpec, timeout: TimeInterval) throws -> [ElementRef]
  func count(_ s: SelectorSpec) throws -> Int
  func tap(_ e: ElementRef) throws
  func press(_ e: ElementRef, duration: TimeInterval) throws
  func text(_ e: ElementRef) -> String            // per 6.4 (placeholder reads as "")
  func typeText(_ e: ElementRef, _ t: String) throws
  func deleteAll(_ e: ElementRef) throws
  func isVisible(_ e: ElementRef) -> Bool; func isEnabled(_ e: ElementRef) -> Bool; func isHittable(_ e: ElementRef) -> Bool
  func hasFocus(_ e: ElementRef) -> Bool          // tvOS only
  func frame(_ e: ElementRef) -> CGRect; func focusedFrame() -> CGRect?
  func remote(_ b: RemoteButton, repeat: Int, interval: TimeInterval) throws   // tvOS
  func pressHome() throws
  func scroll(_ c: ElementRef?, _ d: Direction, steps: Int) throws; func swipe(_ e: ElementRef?, _ d: Direction) throws
  func screenshotPNG() throws -> Data
  func launch(_ bundleId: String) throws; func terminate(_ bundleId: String) throws; func activate(_ bundleId: String) throws
  func deviceInfo() -> DeviceInfo
}
```

### 12.3 Implementation rules
- **Target app.** `XCUIApplication(bundleIdentifier: app_id)`; `launch()` when not running, `activate()` otherwise; arguments `animations_disabled` option → launch arguments that reduce animation.
- **Selector mapping.** `ACCESSIBILITY_ID` → `descendants(matching: .any).matching(identifier:)`; `LABEL` → `label == %@`; `LABEL_CONTAINS` → `label CONTAINS %@`; `PREDICATE` → `NSPredicate(format:)`; `IOS_TYPE` → `XCUIElement.ElementType` by name. **`NSPredicate(format:)` raises an Objective-C exception on bad syntax, which Swift cannot catch:** wrap construction in a tiny ObjC shim (`try/catch` → `NSError`) and map failures to `INVALID_SELECTOR`. Apply `index` with `element(boundBy:)`.
- **Waiting.** Poll `exists`/conditions every `poll_interval_ms` up to the timeout (do not rely on `waitForExistence` for state checks other than existence).
- **click (iOS).** Wait for `exists && isHittable && isEnabled`; `tap()`. Exists but not hittable at timeout → `NOT_INTERACTABLE`.
- **type (replace).** Tap to focus; read current text (per 6.4); if non-empty, `typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: current.count) + text)` (this REPLACES); else `typeText(text)`. Verify by reading back; mismatch → `NOT_INTERACTABLE`. **`append`**: tap, then `typeText(text)` (cursor at end after tap-to-focus is NOT guaranteed: first press the delete key zero times and send the "end" navigation by tapping the field's trailing edge coordinate (documented limitation: on iOS `append` is `◐` and verified by read-back)). **`clear`**: delete `current.count` characters.
- **tvOS.** Focus by `FocusNavigator` over `XCUIRemote.shared.press(...)`, reading `hasFocus`; `click` = focus then `.select`; text entry works only when a text field has focus and the keyboard is up; otherwise `NOT_INTERACTABLE` with an explanatory message.
- **longpress.** iOS `press(forDuration:)`; tvOS `XCUIRemote.shared.press(.select, forDuration:)` after focus.
- **select.** iOS: picker wheel `adjust(toPickerWheelValue:)`, or tap menu item matching the label; not found → `ELEMENT_NOT_FOUND`.
- **scroll/swipe.** iOS: coordinate drags (container or app window) per direction semantics in 5.6; tvOS `scroll` = remote presses.
- **back.** iOS: tap the navigation bar's back button (`app.navigationBars.buttons.element(boundBy: 0)`), else edge swipe (left-edge drag); tvOS: `.menu`.
- **Checks.** Poll to satisfaction; mapping per 6.4; `focused` only on tvOS (`hasFocus`).
- **Quiescence risk.** XCUITest waits for the target app to become idle before queries; apps with perpetual animations can stall. Mitigations: `animations_disabled`, polling with short per-query budgets, and documenting the limitation; verify in Phase 6.
- **Screenshots.** `XCUIScreen.main.screenshot().pngRepresentation`.
- **Logs.** `StreamLogs` = agent-internal logs; optional host-side syslog collector in `AppleTransport` (best effort).
- **Network.** iOS reached via `iproxy` (no local-network permission needed); tvOS needs the configured `agent_address`; optional Bonjour `_impresario._tcp` (TXT: `udid`, `protocol`, `platform`) is a stretch goal (Phase 8). Document the local-network permission step.
- **Concurrency.** The dispatcher is a Swift `actor`; XCUITest calls run on the main thread as required; one command at a time; `Cancel` cancels the active `Task` between poll iterations.
- **Tests.** `swift test` on macOS for dispatcher/`FocusNavigator`/key tables/selector parsing with a fake `UIDriver`. `xcodebuild build-for-testing` for both UI-test bundles. Real-device runs documented in `docs/DEVICES.md` (signing, trust, developer mode, tvOS pairing, exact `xcodebuild` and `iproxy` commands).

---

## 13. Embedded LSP (`impresario lsp`)

- `tower-lsp-server` (or another maintained async LSP library) over **stdio**; tokio runtime; logs to stderr/file only.
- **Workspace index:** `HashMap<SceneName, SceneInfo { uri, name_span, platforms, pieces: Vec<PieceInfo { name, name_span, selectors }> }>` built with `impresario-core`. `notify` (debounced 200 ms, recursive) watches `scenes_dir`, `plays_dir`, and the two config files; editor temp/swap files are ignored; if `scenes_dir` does not exist yet, watch its parent until it appears.
- **Overlay documents:** open editor buffers (`didOpen/didChange`) take precedence over disk for both `.play` and `.scene` files; the index re-parses an open scene buffer immediately (before it is saved).
- **Position encoding:** negotiate UTF-8 if the client offers it, else convert byte spans to **UTF-16** code units (do not assume bytes).
- **Completion** (trigger characters `:`, ` `, `'`, `>`; computed from the current line prefix and the *nearest enclosing `Scene:` line of lower indentation*, scanning upward; works even when the rest of the file has errors):
  | Context (line prefix) | Suggestions |
  |---|---|
  | `Scene: ` inside a Stage | all scene names |
  | `Action: <anything> => ` | pieces of the enclosing scene |
  | `Check: ` | pieces of the enclosing scene; after a piece + space: `is visible/hidden/enabled/disabled/focused`, `text`, `textContains`, `count`, `timeout` |
  | `Action: ` (start) | action verbs, `'` string starter |
  | `key ` | key names (filtered by devices' platforms when known) |
  | `Tags: ` | tags already used in the workspace |
  | `Platforms: ` / `Platform: ` | platform names (and `cross-platform` in scenes) |
  | inside `Pieces:` entries | platform keys (`web=`, `android=`, ...) and selector prefixes valid for that key |
  Suggested piece items show `detail` = per-platform selectors; sort by name.
- **Diagnostics** (debounce 150 ms, on open/change/save): every parser/semantic error from `impresario-core`, plus per-device warnings ("piece `X` has no selector for platform `P` (device `D`)", unsupported action for a device platform). The device set is all devices in `impresario.devices.config` (changeable via workspace setting `impresario.devices`).
- **Hover** on a piece: per-platform selectors table. **Go to definition**: `Scene: X` → scene header; piece in a step → its line in the scene file. **Document symbols**: Play/Act/Stage outline; Scene/Pieces outline. **Rename** of a piece/scene is out of scope.
- **Deliverables:** `docs/LSP.md` (VS Code client snippet, Neovim `lspconfig` snippet), `editors/vscode` (minimal extension launching `impresario lsp` plus a TextMate grammar for `.play`/`.scene`).
- **Tests:** in-process client in tokio tests per feature, including incomplete/invalid documents, unsaved scene buffers, UTF-16 offset conversion with multi-byte characters, and file-watch updates.

---

## 14. Cross-cutting requirements

- **Errors.** `thiserror` enums per crate with `#[source]` chains and stable codes; `miette` at the CLI boundary only. `main` converts errors to exit codes (8.3).
- **Async.** `tokio` multi-thread runtime; bounded channels; blocking work (process spawn, file IO in hot paths) via `spawn_blocking` or `tokio::process`; every spawned task is joined or cancelled on shutdown (`JoinSet` + `CancellationToken`); every network wait has a timeout.
- **Security.** Executors bind loopback by default; non-loopback needs `--allow-remote` plus token/TLS; tokens never logged; subprocesses use argument arrays; user-provided strings are never interpolated into shell commands except the explicit, escaped ASCII `input text` fallback; secrets redaction per 5.6; screenshots may contain sensitive data: document `report_output` handling in the README.
- **Determinism.** Seeds/time are injected in tests; no reliance on wall-clock in unit tests.
- **Docs** (all required): `DSL.md` (grammar + semantics + examples), `PROTOCOL.md` (sequence diagrams + rules), `DEVICES.md` (setup per platform, including webOS developer mode/`ares-inspect`, Tizen debug mode/`sdb forward`, `adb`, `iproxy`, signing), `PARITY.md` (generated from the tables in section 6, plus real-device verification status), `ERRORS.md`, `DECISIONS.md`, `STATUS.md`, `README.md` (quick start that works from a clean clone).

---

## 15. Testing

| Layer | What |
|---|---|
| `core` unit | config validation (every rule in 4), selector parsing and fallback chain, duration/escape parsing, key tables |
| `core` parser | `insta` snapshots, 60+ negative cases, proptest/fuzz no-panic |
| master integration (`assert_cmd`) | `init` then `validate`; `run` against `MockAgent`: pass, step failure, timeout, hung command, cancel, dropped stream, reconnect for next Act, retries/flaky, `--fail-fast`, multi-device parallelism, exit codes, secret redaction in all reporters |
| connector | headless Chromium with `fixtures/web/*.html`: input handled by framework-style listeners, hidden/disabled/obscured elements, delayed elements, `<select>`, scrolling lists, D-pad focus grid, shadow of `contenteditable`; run the raw backend and (when enabled) the chromiumoxide backend against the same fixtures; ES5 lint of the helper script |
| `type`/`append`/`clear` | explicit tests on every executor path proving `type` replaces, `append` appends, `clear` empties (fixture input pre-filled with `"old"`: `type 'new'` → `new`, `append 'x'` → `oldx`) |
| native agents | JVM and `swift test` dispatcher tests with fakes (ordering, one-result-per-command, cancel, unsupported combos, focus traversal reachable/blocked/cycle/budget); emulator/simulator runs against stub targets documented, executed when available |
| LSP | per-feature in-process tests (13) |
| proto | `buf lint`, `buf breaking --against main`; CI check that generated Swift is up to date |

CI (GitHub Actions): Rust matrix (ubuntu, macos): fmt, clippy, test, ES5 lint; Android job: `./gradlew test` (+ optional emulator job); Apple job on macOS: `swift test`, `xcodebuild build-for-testing` (iOS, tvOS); proto job: buf + generation drift.

---

## 16. Reporting schema

See Appendix B.

---

## 17. Phases and acceptance

**Phase 0: risk spikes (throwaway code allowed, findings recorded in `docs/STATUS.md`).**
1. gRPC server inside an Android instrumentation process (emulator + device if available). 2. NIO gRPC server inside an XCUITest runner on iOS and tvOS (and local-network behavior). 3. Raw CDP against the oldest available TV or an old Chromium build: `Runtime.evaluate`, key events, screenshot; chromiumoxide compatibility check. *Accept:* each spike has a written pass/fail and the fallback chosen (D-18 if needed).

**Phase 1: foundation.** Workspace, `impresario-proto`, `core` config + validation + diagnostics, CLI skeleton with `init`, `validate`, `devices list`. *Accept:* `init` then `validate` passes; every config error code has a negative test.

**Phase 2: DSL.** Scanner, parser, AST, validation, `parse --json`, `docs/DSL.md`. *Accept:* examples parse; 60+ negative cases; fuzz/proptest no panics; `type`/`append`/`clear` grammar tests.

**Phase 3: protocol, clients, mock, orchestrator.** Full proto, `AgentClient`, `MockAgent`, planner, executor, retries, reset policy, reporters, exit codes, graceful shutdown. *Accept:* the master integration suite (section 15) passes without devices.

**Phase 4: connector.** Raw backend, helper script, key tables, page resolution, `web` launch, chromiumoxide optional backend. *Accept:* fixture suite passes in headless Chromium for both backends.

**Phase 5: Android agent.** Modules, server, driver, dispatcher, transports (`AdbTransport`). *Accept:* JVM tests pass; instrumented run on an emulator against the stub target passes if an emulator is available (else the manual checklist is complete and `STATUS.md` says "not run").

**Phase 6: Apple agent.** Package, UI-test bundles, `AppleTransport`. *Accept:* `swift test` passes; `build-for-testing` succeeds for iOS and tvOS when Xcode is available (else documented as unverified).

**Phase 7: LSP.** All features, tests, docs, TextMate grammar, VS Code client. *Accept:* LSP test suite passes.

**Phase 8: hardening.** `PARITY.md` generated and reconciled with verified behavior, `STATUS.md` finalized, README quick start executed from a clean clone, CI green, optional Bonjour discovery. *Accept:* section 18.

---

## 18. Definition of done
- All phases accepted; CI green; no `unwrap/expect/panic` in non-test code (enforced by clippy).
- `cargo install --path crates/impresario-master`, `impresario init`, `impresario validate`, and `impresario run` against the mock (and, where available, a real emulator/simulator/TV) work from a clean clone as written in the README.
- `docs/STATUS.md` states, per platform, exactly what was verified on real hardware/emulators versus only unit-tested, and which Phase 0 fallbacks were taken.
- `docs/PARITY.md` matches observed behavior; every unsupported combination returns `UNSUPPORTED_ACTION` and is planner-detected where statically knowable.

---

## 19. Changes from v1 (and why)

| v1 problem | v2 fix |
|---|---|
| Text-entry semantics were contradictory (`'text'` described as "clear-not-implied" and also as "append sugar", `type` as "replace") | One rule (D-06, 5.6): `'text' => P` ≡ `type` = **replace**; `append` appends; `clear` empties. Proto has `TYPE`, `APPEND`, `CLEAR` only; tests prove each. |
| Scene referenced undefined pieces/scenes (`LoginTitle`, `DashboardScreen`, `WelcomeBanner`) | `init` ships both scenes with all pieces; `validate` must pass on `init` output. |
| Scene keys lacked TV platforms; `web` was both a platform key and implied | Platform names = selector keys; explicit fallback chain; `web` is a real device platform for CI. |
| `app_id` was used but absent from the device config | Added `app_id`, `app_url`, `agent_address`, `options`. |
| "Bidirectional gRPC" with unclear dialing | D-01: master always dials; executors are servers. |
| `StreamLogs` duplicated in `Event`; `Wait` and `waitfor` redundant; `CaptureScreenshot` vs command ambiguity | One log mechanism (D-11); `wait` master-side (D-10); checks auto-wait (D-09); ordered `Screenshot` command plus out-of-band RPC with defined roles. |
| Programmatic focus claimed for Android/tvOS (impossible from UiAutomator/XCUITest) | Programmatic focus only on web-runtime TVs; D-pad traversal elsewhere with a specified algorithm and failure modes. |
| `swipe` allowed on TVs "via synthesized touch" (contradiction) | `swipe` is ✖ on TV platforms. |
| `${env.X}` shown unquoted although interpolation was "inside strings" | Interpolation only inside strings; example fixed; secrets redaction rules added. |
| iOS `classChain` selector (not XCUITest API) | Removed (D-15). |
| Android `xpath` selector (not supported by UiAutomator2) | Removed. |
| Two Android modules/flavors for phone/TV | One instrumentation APK with runtime profile (D-13). |
| `serde_yaml` (deprecated), `tower-lsp` (stale), chromiumoxide primary on old TVs | `serde_yaml_ng`, maintained LSP fork, raw CDP default (D-03/D-04). |
| Retry/reset semantics undefined; timeout precedence unclear; exit code 130 not in the table | Sections 5.6, 8.3 define all of them. |
| LSP details missing (position encoding, unsaved buffers, stdout logging) | Section 13. |
| No verification of risky assumptions (gRPC servers on-device, old TV CDP) | Phase 0 spikes with a defined fallback (D-18). |

---

## Appendix A: complete example workspace
Files produced by `init` are exactly those in 4.1/4.2 (config files), 5.2 (`LoginScreen.scene`), section 8.1 (`DashboardScreen.scene`), and 5.4 (`plays/login.play`). Example run:

```
$ impresario run --play plays/login.play --device flagship_android --device living_room_tv
▶ flagship_android (android)
  Play: User Authentication Feature
    Act: Successful Login Flow [smoke, auth]
      Stage: Navigate and Login ........ ✔ 2.4s
      Stage: Verify Outcome ............ ✔ 1.1s
▶ living_room_tv (webos)
  Play: User Authentication Feature
    Act: Successful Login Flow [smoke, auth] ✔ 4.9s
2 devices · 2 acts · 2 passed · 0 failed · 0 skipped · exit 0
```

## Appendix B: `report.json` schema (abridged)

```json
{
  "run_id": "20261016-185512-a1b2",
  "started_at": "2026-10-16T18:55:12+03:00",
  "duration_ms": 12345,
  "status": "passed|failed|interrupted|error",
  "devices": [{
    "name": "flagship_android", "platform": "android", "device_info": { "model": "...", "os_version": "..." },
    "plays": [{
      "name": "User Authentication Feature", "file": "plays/login.play",
      "acts": [{
        "name": "Successful Login Flow", "tags": ["smoke", "auth"],
        "status": "passed|failed|skipped|aborted", "flaky": false,
        "attempts": [{
          "n": 1, "status": "passed", "duration_ms": 3500,
          "stages": [{
            "name": "Navigate and Login", "status": "passed",
            "steps": [{
              "source": "Action: '${env.TEST_PASSWORD}' => PasswordInput",
              "span": { "file": "plays/login.play", "line": 13, "col": 9 },
              "status": "passed", "duration_ms": 220,
              "error": null
            }]
          }],
          "failure": { "step_index": 3, "code": "ELEMENT_NOT_FOUND", "message": "...", "details": {},
                       "screenshot": "screenshots/flagship_android/failure-1.png", "log_tail": ["..."] }
        }]
      }]
    }]
  }]
}
```
`failure` is present only on failed attempts. All `source` strings are unresolved source text.

---

# Part II - impresario second generation (v2 work packages)

Status: normative delta spec. Read `IMPRESARIO.md`, `README.md`, `TASK.md`, `docs/DECISIONS.md`
(all of D-01…D-18, I-01…I-90) and `docs/STATUS.md` **before touching anything**.

Where this file conflicts with `TASK.md` or an earlier decision, **this file wins**, and the
conflict is recorded as a new decision in `docs/DECISIONS.md` (continue numbering at `I-91`; new
binding decisions of this spec are `D-19…`, listed in §2). Do not silently rewrite old decisions:
mark them `Superseded by I-nn` the way I-59/I-60 were.

Everything that makes the current system honest stays: fail loudly (`UNSUPPORTED_ACTION`), no
silent no-ops, `PARITY.md` generated from `impresario-core::support`, `STATUS.md` records what
is verified on real hardware and what is not, every deviation is a numbered decision, Rust gates
(`fmt --check`, `clippy -D warnings`, `test --workspace`) stay green at every commit.

---

## 0. Goals in one paragraph

impresario becomes a **self-contained, installable product**: one binary (plus embedded/managed
agent payloads) that works on Windows, macOS and Linux **without** `adb`, Xcode tools, `ares-*`,
`sdb` or the `tizen` CLI installed. It has a persistent, modern interactive terminal UI, a
global home folder, fully CLI-editable configuration with guided device setup, a clean split
between **devices** (where) and **apps** (what), built-in signing/re-signing for Apple and
Tizen, concurrent and background runs, and a redesigned theatrical DSL (scene / theatre / play)
with first-class LSP **and** a standalone (LSP-free) editor extension.

## 1. Non-negotiable working rules

1. **Spike before commit.** Work package W-DEP (§6) starts with time-boxed feasibility spikes
   whose go/no-go results are written to `docs/SPIKES.md` *before* any large implementation
   depends on them. A failed spike triggers the fallback ladder in §6.0, never a silent shortcut.
2. **No unverified claims.** Anything that cannot be exercised on this host's real hardware
   (known: Windows host + WSL, Galaxy A51, TCL Android TV, Realme RMX3997; **no** Mac, **no**
   webOS/Tizen/iOS/tvOS hardware unless the user says otherwise) is unit/fixture-tested against
   recorded protocol traces, gated behind `#[ignore]` hardware tests with exact instructions,
   and listed in `docs/STATUS.md` as *unverified*. Never write "works" for those.
3. **Protocol facts must be checked, not recalled.** Every wire-level detail named in this file
   (ports, service names, command strings) is a *starting hint from memory*. Verify it against
   the reference implementations named in §6 before coding, and cite the source in the decision.
4. **Licenses.** Before adding any crate or porting any logic, record its license in
   `docs/THIRD_PARTY.md`. Our crates are `MIT OR Apache-2.0`; weak-copyleft (e.g. MPL-2.0)
   dependencies are allowed only as unmodified dependencies and must be flagged to the user.
   Reference code under GPL/LGPL may be *studied for protocol facts* but not copied.
5. **Secrets.** Passphrases, Apple ID credentials, signing keys, tokens go to the OS keychain
   (`keyring` crate or equivalent) or an encrypted file in the global home — never plaintext in
   YAML, never in logs/reports (extend the existing redaction, spec 5.6).
6. **Commit per work package**, each with tests, docs, decisions and STATUS updated. No work
   package is "done" with a stub that returns success.
7. **Backwards compatibility is not required** for the DSL, `init`, or config schema (pre-1.0),
   but a one-shot `impresario migrate` must convert an existing workspace (§8.9).

## 2. New binding decisions (copy into DECISIONS.md as D-19…)

| ID | Decision |
|---|---|
| D-19 | **No external platform tools at runtime.** The product never shells out to `adb`, `xcodebuild`/`xcrun`/`simctl`/`devicectl`, `ideviceinstaller`, `ares-*`, `sdb`, `tizen`, `openssl`, `ssh`, `scp`. Everything is a Rust implementation or an embedded/managed payload (ladder in §6.0). The only tolerated host prerequisite is an OS driver/service we cannot legally or technically ship (e.g. Apple's USB multiplexer service on Windows, WinUSB for Android USB); each such case is explicit in `impresario doctor` with a fix hint. |
| D-20 | **Global home** `~/.impresario` (`%USERPROFILE%\.impresario` on Windows; override with `IMPRESARIO_HOME`). Created on first run. No `init` command. |
| D-21 | **Device ≠ App.** A *device* is only a name, platform and connection info. An *app* is a separate definition (ids, package path, launch options). A run binds play → app → device(s). |
| D-22 | **Three-part DSL**: `*.scene` (locators, tabular), `*.theatre` (reusable stages), `*.play` (acts composed of stages, parametrizable). Stage names are globally unique. |
| D-23 | **Config is editable from the CLI/TUI**, at two scopes (`global`, `workspace`), losslessly (comments and ordering of hand-edited files preserved). |
| D-24 | **Jobs.** A run is a *job* with an id. Jobs can run in the foreground, be detached to the background, be listed, attached, tailed and stopped. Multiple jobs run concurrently; a device is leased by at most one job at a time (single-session native agents, D-02). |
| D-25 | **Two editor modes** from one source of truth: the language server (primary) and a standalone mode needing no server process. Grammar/diagnostic parity between them is enforced by a shared conformance corpus. |

---

## 3. Work package W-CLI — a modern, persistent CLI

### 3.1 Product shape
- `impresario` with **no arguments** starts the interactive full-screen/inline TUI session
  (think Claude Code / Gemini CLI): persistent prompt, slash commands, history, live panels,
  status bar. Every capability is *also* a scriptable non-interactive subcommand
  (`impresario run …`, `impresario config set …`) with stable exit codes (I-21 stays) for CI.
- Non-TTY, `CI=1`, `--no-tui` or `--plain`: never start the TUI, print plain line output, no
  prompts (a missing required value is exit 2 with a precise message, not a hang).
- Suggested stack (verify and record in a decision): `ratatui` + `crossterm`, `inquire` or
  `cliclack` for wizards, `clap` derive + `clap_complete`, `tracing`, `owo-colors`/ratatui styles,
  `indicatif` for non-TUI progress. Must work in Windows Terminal, PowerShell, cmd, macOS
  Terminal/iTerm, and common Linux terminals; degrade to 16 colors and ASCII when needed.

### 3.2 TUI requirements
1. **Home screen**: workspace path, global home, devices (live reachability), apps, last run
   summary, running/background jobs, update/doctor warnings.
2. **Prompt with slash commands** and fuzzy completion: `/run`, `/jobs`, `/devices`, `/apps`,
   `/config`, `/tests`, `/open`, `/doctor`, `/fmt`, `/validate`, `/help`, `/quit`. Tab-complete
   device/app/play/act/stage/tag names. Persistent history (`~/.impresario/history`), Ctrl-R
   search, multi-line paste safe.
3. **Run view**: tree of devices → plays → acts → stages → steps with live status, elapsed time,
   current command, rolling log pane per device, screenshot path on failure, filter by
   status. Keys: `b` send to background, `x` stop, `f` follow log, `tab` switch device,
   `/` filter, `?` help. Terminal resize safe, no flicker, no corruption on panic (restore
   terminal in a panic hook).
4. **Test explorer** (§3.4) and **device/app/config editors** (§4, §5) are TUI screens with the
   same functions available as flags/subcommands.
5. **Persistence**: session state (last selection, last run profile, window layout) survives
   restarts (`~/.impresario/state.json`); a crashed TUI never loses a background job (§3.5).
6. Non-goals: mouse is optional-but-supported; no network calls for telemetry (none at all).

### 3.3 Global home layout
```
~/.impresario/
  config.yaml            # global config (defaults, endpoints, current workspace, ui prefs)
  devices.yaml           # global devices (reusable across workspaces)
  apps.yaml              # global apps
  keys/                  # generated adb keys, ssh keys (0600), signing identities (encrypted)
  agents/<version>/      # extracted/cached agent payloads (APKs, Apple runner bundle…)
  tools/<name>/<ver>/    # managed downloads when the ladder (§6.0) requires them, checksummed
  jobs/<job-id>/         # job state, logs, socket/pipe metadata
  history                # prompt history
  state.json             # TUI state
  logs/                  # rotated diagnostic logs
```
- Workspace discovery order: `--workspace <path>` → `IMPRESARIO_WORKSPACE` → `.impresario/` or
  `impresario.config` found walking up from CWD → `workspace` key in global config → CWD.
- The user can change the workspace path any time: `impresario workspace use <path>` (persists
  in global config), `workspace list|add|remove|current`. First run in an empty directory offers
  to create the workspace skeleton **from the TUI/wizard** (this replaces `init`; the example
  scaffold lives behind `impresario workspace new <path> [--example]`).
- Workspace config may live under `<workspace>/impresario.config` and device/app overrides in
  `<workspace>/impresario.devices.config`, `<workspace>/impresario.apps.config` (keep names;
  global ones are `~/.impresario/*.yaml`).
- **Precedence** (high → low): CLI flag → environment (`IMPRESARIO_*`) → workspace file →
  global file → built-in default. `impresario config get <key> --explain` prints the winning
  layer and the shadowed ones. Name collisions between global and workspace devices/apps:
  workspace wins and `devices list` marks `(overrides global)`.

### 3.4 Visibility and control over everything, including test files
- `impresario tests` (and the TUI explorer) lists scenes, theatres, plays, acts, stages, tags,
  platforms, parameters, the files they live in, and *where each is used*.
- Select what to run by any combination: file/glob, play, act (`--act "name"`), tag,
  `--exclude-tag`, platform, "only failed in last run", and an interactive checkbox tree.
  Show the resolved plan (`impresario run --dry-run`, `/plan`): act × device matrix, what is
  skipped and why (platform filter, unsupported action `IMP-DSL-013`), parameter bindings
  (secrets masked).
- Choose devices per selection: `--device a,b`, interactive picker, or a **run profile**
  (`impresario run --profile smoke`, `profiles:` in config; `impresario profile save|list|rm`).
- Test-file operations from the CLI: `new scene|theatre|play <name>` (templates), `show`, `open`
  (`$VISUAL`/`$EDITOR`, falling back to `code`), `rename` (updates every reference across
  the workspace), `fmt`, `validate`, `lint`, `grep <text>`, `where <stage|piece|scene>`.

### 3.5 Parallel and background execution
- A run = a **job** (id `YYYYMMDD-HHMMSS-xxxx`, as run ids today). `impresario run` is attached
  by default; `--detach` (or `b` in the TUI) hands the job to a **background supervisor**.
- Supervisor: a small per-user daemon (`impresario daemon`, auto-started on demand, exits when
  idle) reachable over a Unix domain socket / Windows named pipe (candidate: `interprocess`;
  verify). The daemon owns job execution, so closing the terminal does not kill detached jobs,
  and `Ctrl-C` in an attached view detaches-or-stops by explicit choice, never ambiguously.
- Commands: `jobs [--all]`, `attach <id>`, `logs <id> [-f]`, `stop <id>`, `rm <id>`,
  `report <id>`, `wait <id>` (exit code mirrors the job). Notifications on completion in the TUI
  (and OS notification if a crate makes that low-risk; optional).
- **Concurrency model**: any number of jobs; within a job devices run in parallel (today's
  behavior); across jobs a **device lease** prevents two jobs from driving one device. Conflict
  policy flag: `--on-busy wait|fail|queue` (default `wait` with visible queue position).
  Different devices, different platforms, even the same play on all of them at once must work
  (web + android + androidtv in one job is a test case).
- Exit codes and reporters are unchanged (I-21, I-23); detached jobs persist `report.json`
  exactly as foreground ones.
- Recovery: daemon crash/reboot marks interrupted jobs `interrupted`, never "running".

### 3.6 Acceptance for W-CLI
Scripted PTY tests (`portable-pty`/`expectrl`-style) for: first-run flow, slash command
completion, run view with a `MockAgent`, background detach → close → reattach → report;
snapshot tests of TUI frames (`ratatui` `TestBackend`). Manual checklist for the three Windows
shells in `docs/CLI.md`.

---

## 4. Work package W-CONFIG — everything editable from the CLI

### 4.1 Scope
All settings in both scopes are readable and writable from CLI **and** TUI:
`impresario config list|get|set|unset|edit|path|explain [--global|--workspace]`.
Keys are dotted (`reporters`, `timeout_default_ms`, `app_reset`, `ui.theme`, …); `set` validates
against the same schema the loader uses (unknown key = error, D-04) and refuses bad values with
the same diagnostics (`IMP-CFG-*`). `config edit` opens the file in `$EDITOR` and re-validates
on close (keeps the old file if invalid, shows the error, offers to reopen).

### 4.2 Lossless editing (hard requirement)
Edits made by the CLI must **preserve comments, key order and formatting** of hand-edited
files. Survey crates for round-trip YAML editing; if none is production-grade, record a
decision to move the **config format** to TOML (`toml_edit` is lossless) — migration included
in `impresario migrate`. This supersedes D-04's *format* only (snake_case, unknown-keys-are-errors
stay). Property-test: set then unset a key returns a byte-identical file.

### 4.3 Device wizard (multi-step, modeled on `ares-setup-device`)
`impresario device add|edit|remove|list|show|check|rename|import|export` (+ TUI screen).
`ares-setup-device` flow to emulate: pick **platform** → name → host/port → credentials/key
strategy → (platform specific step) → connectivity check → summary → save, with `--list`,
`--modify`, `--remove`, `--default`, and a non-interactive flag form of every step for scripts.
Wizard rules:
1. Every step shows a default, validates immediately, and can go **back** (`←`/`b`) and **cancel**
   without writing anything. Final screen shows the YAML to be written and the scope
   (`global` default; `--workspace` to scope locally).
2. Platform-specific steps (details in §6/§7):
   - **web**: `launch[:headless]` or CDP endpoint; `page_match`.
   - **android / androidtv**: discover (USB + network scan + mDNS), pair (Android 11+ wireless
     pairing code **and** classic `connect ip:5555`), role detection phone/TV, key generation
     stored in `keys/`, authorization prompt guidance.
   - **webos**: host, SSH port (9922 default for dev mode), user (`prisoner`), the dev-mode
     **passphrase / key server** flow (fetch key from the TV's dev-mode key service, decrypt with
     the passphrase, store it), host-key pinning, test connection, "Dev Mode app expires" warning.
   - **tizen**: host + sdb port, Developer Mode IP check, certificate profile selection or import
     (§7.4), DUID retrieval and display, test connection.
   - **ios / tvos**: discover via USB/Wi-Fi/mDNS, trust/pairing file creation (§6.2), Developer
     Mode check, signing identity selection (§7.1–7.2), tvOS remote pairing.
3. Secrets typed in a wizard go to the keychain; YAML stores only a reference (`secret: keychain:<name>`).
4. A device definition contains **only**: `name`, `platform`, `connection` (host/port/serial/udid/
   transport/endpoint), `auth` references (key/profile/identity names), `labels`, and
   transport-level `options`. No bundle id, package path, app URL, or per-app reset flags
   (see §5). `device check` runs the real probe (I-32) and shows the capabilities.

### 4.4 Acceptance
Golden tests of the wizard against a scripted answer stream (including back/cancel), lossless-edit
property tests, `config get --explain` tests for every precedence layer, and a doc table of every
key (generated from the schema, like PARITY).

---

## 5. Work package W-APPS — app definitions (D-21)

`impresario.apps.config` (workspace) / `~/.impresario/apps.yaml` (global), edited with
`impresario app add|edit|remove|list|show|check` (+ wizard, same UX rules as §4.3):

```yaml
apps:
  - name: shop
    description: Customer shop app
    platforms:
      android:   { id: com.acme.shop, package: ./build/app-release.apk, launch_activity: null }
      androidtv: { id: com.acme.shop.tv, package: ./build/tv.apk }
      ios:       { id: com.acme.shop, package: ./build/Shop.ipa }
      tvos:      { id: com.acme.shop.tv, package: ./build/ShopTV.ipa }
      webos:     { id: com.acme.shop, package: ./build/shop.ipk }
      tizen:     { id: AbCdEf1234.Shop, package: ./build/shop.wgt }
      web:       { url: "https://staging.acme.com" }
    options:
      clear_data: true            # was device option clear_app_data (I-18/I-82/I-84)
      launch_params: {}           # platform specific, documented
```
- `id` = Android package, Apple bundle id, webOS app id, Tizen app id (`<pkgid>.<name>`).
  `package` = the artifact to install/re-sign (optional: attach to an already-installed app).
- Binding: a play declares `App: shop` (header), overridable with `--app`; `app_reset`,
  `clear_data`, `app_path` (I-84) move from device options to app options. The planner reports
  `IMP-APP-*` errors (unknown app, no entry for the device's platform, package missing) before
  hardware is touched. A device never knows an app; an app never knows a device.
- `impresario app install <app> --device <d>` and `app uninstall`, `app check` (id present on
  device, signature valid for that device) are scriptable shortcuts that use the same
  install paths the runner uses.
- Update `docs/ERRORS.md` (new `IMP-APP-*`, `IMP-DEV-*`, `IMP-SIGN-*`, `IMP-JOB-*` families),
  `PROTOCOL.md` (handshake no longer carries per-device app options; carries the resolved app),
  and migrate every transport (`AdbTransport`, apple, connector spawn) to take app + device.

---

## 6. Work package W-DEP — eliminate the runtime environment (D-19)

### 6.0 The fallback ladder (apply per capability, in order, and record the rung chosen)
1. **Pure-Rust implementation** or a well-maintained crate (preferred, no payload).
2. **Embedded** minimal payload in the binary or release archive (`include_bytes!`/sidecar),
   extracted to `~/.impresario/agents|tools`, with SHA-256 verification.
3. **Managed download** on first use into `~/.impresario/tools/<name>/<version>/`, pinned
   version + checksum + signature where available, offline-installable by `impresario tools
   import <archive>`; always explicit and cancellable, never silent.
4. **System tool detection** as a last resort only for an optional convenience (never required
   for a test run); `impresario doctor` explains what is missing and why.
Each capability's chosen rung and rationale go in `docs/SPIKES.md` + a decision. Deliverable of
the work package: `impresario doctor` on a **clean VM with none of adb/Xcode/ares/sdb/tizen
installed** reports green for every platform whose hardware is reachable.

### 6.1 Android / Android TV (replace `adb`)
Needed capabilities (inventory from `AdbTransport`): device discovery (USB + TCP), `connect`,
wireless **pairing** (Android 11+ `adb pair`, SPAKE2 over TLS), RSA key auth (A_AUTH/A_STLS),
`shell` (+ exit status), `exec:` streams, APK **install** (`cmd package install` over streamed
`exec`/`sync` push), `am instrument` launch with output stream, `forward` fallback (I-62),
`logcat` streaming, `screencap`, `pm clear`/`force-stop`, `getprop`, `ip route` (agent endpoint
discovery).
- Candidates to evaluate in the spike: `adb_client` (pure-Rust ADB protocol client with direct
  TCP/USB device modes — verify current API, USB support on Windows, auth/pairing coverage),
  `mozdevice`, `rusb` for USB. Pairing/TLS may need our own implementation (`rustls`, a SPAKE2
  crate such as `spake2`); consult the AOSP `adb` sources (`pairing_auth`, `tls`) and the
  documented protocol (`OVERVIEW.TXT`, `SERVICES.TXT`, `SYNC.TXT`) for facts.
- Windows USB note: raw USB access needs a WinUSB-compatible driver; network ADB needs nothing.
  Support both, document the USB driver requirement honestly in `doctor`, and make the
  network path the default in the wizard.
- Keep the agent architecture (I-62): the master talks gRPC to the agent over the network;
  ADB-equivalent functions only install/start/log/screenshot.

### 6.2 iOS / tvOS from any PC (replace Xcode tools; user pointers: `idevice`, `apple-codesign`)
What `xcodebuild test` does for us today, and what must be reproduced:
1. device discovery + **pairing** (lockdown: usbmuxd, or network pairing for Wi-Fi/Apple TV),
2. **Developer Mode** check (cannot be enabled remotely: wizard instructs, `doctor` verifies),
3. **Developer Disk Image** mount (classic DDI ≤ iOS 16, personalized image iOS 17+, with the
   TSS/signing flow; fetch DDI assets as managed downloads),
4. **install** an `.ipa`/`.app` (installation_proxy + AFC upload),
5. **launch an XCTest runner without `xcodebuild`**: start the UI-test bundle through the
   developer services (testmanagerd / DTX instruments channel, `XCTestConfiguration`),
   the technique used by go-ios (`runtest`/`runwda`), pymobiledevice3 and tidevice for WDA.
   iOS 17+ needs the CoreDevice/RSD **tunnel** (remote pairing, QUIC/TCP tunnel to the device;
   on Windows a user-space/TUN approach — verify what the crate supports),
6. capture logs/syslog/screenshots as needed (we need little: gRPC does the rest),
7. **sign** everything (§7.1–7.2).
- Primary candidates: **`idevice`** (pure-Rust libimobiledevice-style crate — verify supported
  services: usbmuxd, lockdown, installation_proxy, AFC, mounter, DVT/instruments, testmanagerd,
  RSD/tunnel, remote pairing, tvOS) and **`apple-codesign`** (the library behind `rcodesign`:
  Mach-O/bundle/IPA signing, entitlements, provisioning profiles, certificate handling). Studying
  go-ios/pymobiledevice3 for protocol facts is encouraged (license rule §1.4).
- **The runner binary cannot be compiled without Xcode.** Therefore: the Xcode-built, *unsigned*
  runner bundle (`AgentUITests-iOS/-tvOS` + host app, from `agents/apple`) is produced **once per
  release in macOS CI** (GitHub macOS runner; add the job and the release artifact), embedded or
  fetched under the ladder (§6.0), and **re-signed on the user's machine** per device/team. Verify
  this is technically possible for a UI-test bundle + `XCTestConfiguration` (entitlements,
  `get-task-allow`, `com.apple.private.*` restrictions, injected frameworks signing order) in the
  spike, and write the exact, honest outcome.
- **Windows reality check:** USB access to iOS devices on Windows normally relies on Apple's
  **Apple Mobile Device Service** (iTunes / Apple Devices app) providing the usbmuxd socket.
  Evaluate: (a) use it when present (document as the one tolerated prerequisite, D-19),
  (b) a pure-Rust usbmuxd replacement over `rusb` (driver replacement implications), and
  (c) the network path (pairing file created once, then Wi-Fi lockdown + RSD). Decide and record.
  Linux: usbmuxd daemon or pure-Rust; macOS: system usbmuxd.
- Apple device acceptance without hardware: replay tests against recorded wire traces, a loopback
  fake lockdown/usbmuxd server for the transport state machine, and `swift test` stays as is.
  On-device steps become `#[ignore]` tests with a runbook in `docs/DEVICES.md`.
- Fall back honestly: if a spike proves (5) impossible for some OS versions, the CLI reports
  `IMP-DEV-xxx: requires macOS + Xcode for iOS ≥ N` with the reason, and `STATUS.md` says so.

### 6.3 webOS (replace `ares-*`)
`ares-setup-device / ares-package / ares-install / ares-launch / ares-inspect / ares-novacom`
are SSH + file copy + `luna-send` wrappers. Re-implement in Rust:
- **SSH/SFTP client** (`russh` + `russh-sftp`, or `ssh2`; evaluate the Windows story, pure-Rust
  preferred): key auth, dev-mode user, port 9922, host-key pinning, port forwarding for the
  inspector/CDP port, exec with exit status.
- **Dev-mode key retrieval**: the TV's Developer Mode app exposes a key service; the CLI fetches
  the encrypted private key and decrypts it with the passphrase (verify the cipher/KDF against the
  `ares-cli` sources, Apache-2.0 — **study the repository for facts**). Pure-Rust decryption (no
  `openssl` binary).
- **`.ipk` packaging** (`ar` archive containing `control.tar.gz` + `data.tar.gz` + `debian-binary`,
  with `appinfo.json` handling and the app id/version rules `ares-package` applies): implement with
  `ar`/`tar`/`flate2` crates; produce byte-compatible output; fixture-test against a package
  created by the official tool (checked into `fixtures/`, one-time creation by the user if needed).
- **Install/launch/close/list** via the same `luna-send`/`appinstalld` (or `ares-install`'s
  equivalent) commands over SSH — copy the *commands*, verified from sources.
- **Debug/inspect**: launch with the inspector enabled, forward the CDP port over SSH, hand
  `target_endpoint` to the existing connector. The connector stays the web/webOS/Tizen executor
  (D-03) and is spawned/embedded by the master as today (I-33) — now an embedded sidecar of the
  same release, not a PATH lookup.
- No hardware here: fixture tests with a **fake SSH server** (`russh` server side) replaying
  recorded `luna-send` transcripts; hardware tests `#[ignore]`d with a runbook.

### 6.4 Tizen TV (replace `sdb` and the `tizen` CLI)
- **sdb protocol** (derived from ADB; TV in Developer Mode listens on TCP 26101 — verify) implemented
  natively: connect/`devices`, `shell`, `push`/`pull` (sync), install, launch, debug-port query,
  forward. Source the facts from the open-source Tizen tooling sources and community
  implementations (e.g. TizenBrew, Samsung `tizen-sdb` sources if available) — verify, cite.
- **Package** (`.wgt` = zip) creation and **signing** natively (§7.4): `config.xml` handling,
  `author-signature.xml` + `signature1.xml` (W3C Widget Digital Signature / XML-DSig, RSA/ECDSA),
  with the author certificate and the **distributor certificate containing the device DUIDs**.
- Install (`vd_appinstall`/`pkgcmd`-equivalent on TVs — verify the exact on-device command),
  launch, **debug launch returning the inspector port**, forward to the connector's CDP endpoint.
- `device check` shows the DUID and whether the loaded distributor certificate covers it.

### 6.5 Web
Chrome discovery stays (I-35). Add **managed Chromium** (ladder rung 3) as an opt-in:
`impresario tools install chromium` (pinned build, checksum) so even a clean machine runs `web`.

### 6.6 `impresario doctor`
Per platform: what is needed, what was found, which rung provides it, and the exact fix for
anything missing (e.g. "Apple Devices service not running", "USB driver for Android not bound",
"TV Developer Mode off", "Developer Mode disabled on iPhone"). Machine-readable `--json`.
Doctor never installs without consent.

---

## 7. Work package W-SIGN — signing built into the CLI

### 7.1 Apple identity management
`impresario sign identity add|list|show|remove|renew` (+ wizard). Supported sources, in order of
implementation value (verify feasibility in the spike; record the legal/ToS notes):
1. **Existing Apple Developer identity**: import a `.p12` + provisioning profile(s), or
   App Store Connect API key (`.p8`, issuer, key id) to create/download certificates, App IDs,
   device registrations (UDID) and development profiles via the official API.
2. **Apple ID login (free or paid team)** using the provisioning flow of sideloading tools
   (AltServer/Sideloadly/SideStore-style: 2FA, anisette data, development certificate, wildcard or
   explicit App ID, profile for the device). Evaluate crates/prior art (`isideload`-style,
   `icloud-auth`, `apple-codesign`'s own App Store Connect support) and **warn clearly** about
   free-account limits (7-day expiry, app-ID and device quotas) and ToS risk. Apple ID password
   and session go to the keychain; anisette provider configurable.
Everything stored encrypted/keychain; expiry tracked; `doctor` and `devices check` warn
*N days before* a profile/cert expires and `sign renew` re-provisions.

### 7.2 What gets signed, automatically, at run time
For `ios`/`tvos` the planner's "prepare" step becomes: resolve identity → ensure device
registered → ensure profiles for **runner host app, UI-test bundle and the app under test** (the
latter only if we install it) → **re-sign** with `apple-codesign` (entitlements per target,
nested frameworks first, `get-task-allow`) → install → start runner. Re-sign output is cached
by (artifact hash, identity, device UDID) in `~/.impresario/cache/` to keep repeat runs fast.
Failure modes get dedicated `IMP-SIGN-*` codes with fixes (expired profile, device not in
profile, bundle id taken, 2FA required, entitlement not allowed on free team).
`impresario sign check <app> --device <d>` and `sign resign <ipa> --identity <i> --out <ipa>`
are exposed for manual use.

### 7.3 Tizen certificates
`impresario sign tizen profile add|list|show|remove` (author certificate + distributor
certificate + passwords, keychain-stored). Support: **import** existing profile files/`.p12`s
(always), and **create** an author certificate and obtain a distributor certificate bound to the
TV's DUID via Samsung's certificate services *if* the spike shows a documented/stable flow
(Samsung account auth); otherwise import-only, stated honestly.
### 7.4 Tizen repackage-and-sign pipeline
For each Tizen run: take the app's `.wgt`, **unpack → keep/replace signatures → re-sign with the
device's profile (DUID-matching distributor cert) → repack**, cache by (hash, profile, DUID),
install, verify. The pipeline must be a pure function with golden tests (signature files
structure, digest of every entry, deterministic zip ordering) and a validator that checks our
output against the W3C digsig rules. Errors: `IMP-SIGN-*` (DUID not in certificate, expired cert,
wrong author).
### 7.5 Android
Debug-signing of an unsigned APK when requested (`apksig`-equivalent in Rust or an embedded
debug keystore flow) — optional convenience; record decision.

---

## 8. Work package W-DSL — the theatrical DSL (D-22) and tooling

### 8.1 File types and meaning
| File | Theatrical name | Role |
|---|---|---|
| `*.scene` | the **set** | one screen: its locators (**pieces**), in a table |
| `*.theatre` | the **stagecraft** | named, reusable **stages**: sequences of actions/checks over scenes |
| `*.play` | the **performance** | **acts** = ordered sequences of stage references (with arguments) |
Rename internals accordingly (`Scene`, `Theatre`, `Stage`, `Play`, `Act`, `Piece`); keep terminology
consistent in errors, docs, reports, LSP and the TUI. Workspace layout: `scenes/`, `theatres/`,
`plays/` (configurable).

### 8.2 `.scene` — tabular locators
```
Scene: LoginScreen
Platforms: cross-platform

| Piece         | web                       | android          | ios                          | index |
|---------------|---------------------------|------------------|------------------------------|-------|
| LoginTitle    | h1.title                  | id:login_title   | accessibilityId:login_title  |       |
| UsernameInput | #username                 | id:username      | accessibilityId:username     |       |
| SubmitButton  | button[type='submit']     | id:login_btn     | accessibilityId:login        |       |
| PromoBanner   | .promo                    | -                | -                            | 2     |
```
- Header row defines columns: first column is the piece name, then any subset of the seven
  platform keys in any order (plus optional `index`, 0-based). Pipes delimit cells; a literal
  pipe is `\|`; cells are trimmed; quotes are optional and only needed to preserve edge spaces
  or when the cell would be misread (`"`/`'` accepted; same escapes as strings).
- **Absent value** = empty cell, `-`, `~`, or the words `none`/`null` (document one canonical
  spelling for the formatter: `-`). Absent means "no selector for this platform **here**": the
  fallback chain (D-07) then applies; if still unresolved, `IMP-DSL-012` is raised **only when a
  play step actually needs that piece on that device** (as today), plus a *hint* listing which
  platforms the piece does define. A fully absent row is a warning.
- A platform column missing from the table is equivalent to an all-absent column.
- Per-cell selector grammar is unchanged (§5.3 / `DSL.md`); the selector is validated with the
  correct prefix rules **per column** and diagnostics point at the **cell** (exact span).
- Uniqueness: scene name (workspace-wide), piece name (per scene), column name (per table).
- `Platforms:` stays (cross-platform | list) — but see 8.7: it may also be omitted (default
  cross-platform).

### 8.3 `.theatre` — stages
```
Theatre: Authentication

Stage: Login(user, password, remember = 'false')
    Description: Fills the login form and submits it.
    Scene: LoginScreen
        Check: LoginTitle
        Action: '${param.user}' => UsernameInput
        Action: '${param.password}' => PasswordInput
        Action: click => SubmitButton
    Scene: DashboardScreen
        Check: WelcomeBanner is visible timeout 20s

Stage: DashboardShown
    Timeout: 15s
    Scene: DashboardScreen
        Check: WelcomeBanner
```
- A theatre file defines **one or more stages**; `Theatre:` names the file's group (unique,
  workspace-wide). Stage names are **globally unique across all theatres** (`IMP-DSL-030`
  duplicate stage, with both locations in the diagnostic). Theatre-qualified references
  `Authentication.Login` are accepted for readability and required only if the formatter/LSP
  needs to disambiguate (it never needs to if uniqueness holds).
- Stage body = what `Stage:` contained before (`Timeout:`, scene blocks, actions, checks), now
  reusable. Stage **parameters**: `Name(param, param = 'default', …)`; reference as
  `${param.NAME}` inside strings, durations, counts where the grammar allows interpolation
  (decide the exact set; document). Missing required argument, unknown argument, duplicate
  parameter: `IMP-DSL-03x`.
- A stage may **call other stages** with `Perform: OtherStage(args)` (cycle detection →
  `IMP-DSL-03x`; depth limit documented). Keep this small but included — nested reuse is the
  point of stages.
- Platform applicability stays per scene block. A stage may declare `Platforms:` to restrict.

### 8.4 `.play` — acts built from stages
```
Play: User Authentication Feature
Description: Validating secure user entry across platforms.
App: shop
Var: USER = 'test_user_impresario'

Act: Successful login
Tags: smoke, auth
Platforms: android, ios, webos
    Perform: Login(user = '${var.USER}', password = '${env.TEST_PASSWORD}')
    Perform: DashboardShown

Act: Login rejects bad passwords
Tags: auth, negative
    Examples:
        | user      | password   |
        | alice     | wrong-1    |
        | bob       | wrong-2    |
    Perform: Login(user = '${example.user}', password = '${example.password}')
    Perform: LoginErrorShown
```
- An Act is **only** a sequence of `Perform:` lines (stage references with arguments). Inline
  actions/checks in an Act are not allowed — that is what theatres are for (the error explains
  and the LSP offers a quick-fix "extract to stage").
- **Parametrization** is first class: stage arguments (above), play `Var:` + `--var`, `${env.X}`,
  and **`Examples:` tables on an Act** (data-driven acts, one execution per row, each reported as
  `Act name [user=alice]` with secrets masked). Table cells use the same absent-value rules.
  Act-level parameters may also be declared `Act: Name(param, …)` and supplied from `Examples`
  or from CLI `--arg act.param=value`; decide, document, test.
- Uniqueness: act names within a play; stage references must resolve (`IMP-DSL-031` unknown
  stage with "did you mean"); `Perform` argument names/arity checked at validate time
  (`IMP-DSL-032`).
- Reports keep the hierarchy **play → act → stage → step** (stage nodes now come from the
  theatre; reports show the stage's source location). Update JUnit/JSON/HTML/console and
  `report.json` schema version.

### 8.5 Grammar and compatibility
Update `docs/DSL.md` (EBNF included), the scanner/parser in `impresario-core`, the AST (all nodes
keep `Span { file, line, col, len }`; table cells carry precise spans), planner expansion
(act × example rows × device), and `IMP-DSL-*` catalog (`ERRORS.md`). New file-kind mapping in
`impresario validate/parse/fmt`. Keep lexical rules (indent, comments `//`, strings, durations)
unless this section says otherwise; tables are the only new lexical construct and the scanner
must treat table lines as a block at depth 1 under `Examples:` or at depth 0 in scenes.

### 8.6 Formatter
`impresario fmt [--check] [PATH…]` for `.scene/.theatre/.play`: canonical keyword casing, indent
unit, blank-line rules, **column-aligned tables** (monospace-width aware: CJK/RTL safe — use
`unicode-width`; the project has Arabic content), `-` for absent cells, sorted nothing (never
reorders semantics), idempotent (property test: `fmt(fmt(x)) == fmt(x)`), preserves comments.
The same formatter powers LSP formatting.

### 8.7 Lints and validation additions
Unused scene/piece/stage/theatre/var, unused example column, shadowed var, Act without
`Platforms:` that uses a piece unavailable on some configured platform (list them), stage with
required param never supplied. All with stable codes, hints and (where mechanical) LSP quick-fixes.

### 8.8 Runtime/planner
Planner resolves play → acts → example rows → stage calls (params bound, nested expansion with
source-span chain for errors "in stage X called from act Y at line N") → per-device command list.
`IMP-DSL-013` still raised before hardware is touched. Secrets in params follow 5.6 redaction.

### 8.9 `impresario migrate`
Converts an existing workspace: scene piece lines → tables; each old `Stage:` inside an Act →
a stage in a generated theatre (named from the stage; collisions suffixed and reported) with the
Act referencing it; `init`-style config → global/workspace split; device `app_id`/options →
`apps` file. Idempotent, `--dry-run`, leaves a `.bak`. Golden tests on `examples/`.

---

## 9. Work package W-EDITOR — LSP and the standalone extension

### 9.1 LSP (primary, `impresario lsp`) — must cover all three file kinds
- **Diagnostics** (all codes of §8, live, per-cell for tables), **completion** (keywords by
  context; scene/piece/stage/theatre/act/param/var/env names; platform keys; selector prefixes per
  column; durations; `Perform:` argument names with defaults shown; `Examples:` column headers
  from the stage's params; app/device names in headers), **hover** (resolved selector per
  platform + fallback, stage signature + doc + where used, support-matrix verdict for the action
  on each platform), **go to definition / find references / rename** (pieces, scenes, stages,
  theatres, vars, params — workspace-wide, with prepare-rename), **document & workspace symbols**,
  **folding**, **semantic tokens**, **signature help** inside `Perform: Stage(`, **code actions**
  (extract inline steps to a stage, add missing platform column, fix did-you-mean, create missing
  stage/piece, make a duplicate unique, remove unused), **formatting / range formatting /
  on-type formatting** (table re-alignment as you type `|`), **inlay hints** (resolved params),
  **code lens** ("Run act on device…" → invokes the CLI/daemon, "N references").
- **Duplicate detection of anything that must be unique** (scene, piece, theatre, stage, act,
  param, column, `Var`, example column) is mandatory and reported on **every** occurrence with
  related-information links to the others. Cross-file uniqueness included, and updated live as
  files open/close/change on disk (watcher exists, I-76).
- Keep I-76 position-encoding behavior; extend the conformance tests per feature.

### 9.2 Standalone extension mode (no LSP)
`editors/vscode` gets a setting `impresario.mode: "lsp" | "standalone" | "auto"` (auto = LSP if the
`impresario` binary resolves, else standalone). Standalone must deliver the **same feature list**
above (diagnostics, completion, hover, definition/references/rename, symbols, folding, formatting,
code actions, duplicate detection) **inside the extension host**, with no child process.
- Preferred implementation: compile `impresario-core` (scanner, parser, validation, formatter,
  support matrix) to **WASM** and load it from the extension (one source of truth, zero drift).
  Acceptable fallback: a TypeScript implementation fed by a generated `language-spec.json`
  (`impresario spec --json`: keywords, platforms, selector prefixes, support tables, diagnostics
  catalog) — then a **conformance corpus** is mandatory (below). Record the choice (I-nn).
- **Parity gate**: `fixtures/lang-corpus/` holds `(file, expected-diagnostics.json,
  expected-format.txt, completion-probes.json)`. The Rust suite, the LSP suite and the extension's
  test suite (`@vscode/test-electron` + unit tests) all run the same corpus; CI fails on any
  divergence.
- The workspace index in standalone mode is built in the extension (file watcher via the VS Code
  API), debounced, incremental, and must stay responsive on 1,000+ files (benchmark in the tests).
- Packaging: `vsce` build, bundle with `esbuild`, strict TypeScript, ESLint, no runtime network
  access; syntax grammar updated for tables/new keywords; snippets for scene/theatre/play/act/stage.
- Also ship the same in `impresario` TUI where cheap: `fmt`, `validate` and duplicate checks are the
  shared core, no new logic.

---

## 10. Phasing (execute in this order; each phase ends green and documented)

| Phase | Content | Exit criteria |
|---|---|---|
| **0** | Read-in + **spikes** for §6.1–6.5 and §7.1/7.3 feasibility (`docs/SPIKES.md`): Android w/o adb (install + instrument + logcat on the A51/TCL TV from a clean PATH), `idevice`/`apple-codesign` capability matrix and iOS-without-Xcode verdict, SSH+luna-send for webOS, sdb protocol + signing for Tizen. Cross-platform TUI/daemon/IPC/lossless-config crate choices | Written go/no-go + chosen rung per capability + new decisions; user-visible summary of what is *not* possible and why |
| **1** | Global home, workspace resolution, config layering and lossless editing, `device`/`app` split + migration of transports, remove `init`, `workspace` commands | W-CONFIG/W-APPS acceptance; `migrate` for config part |
| **2** | New DSL (scene/theatre/play), parser, planner, formatter, validation, reports, `migrate` for DSL | DSL docs + golden corpus green; live E2E on web + phone + TV re-run with migrated workspace |
| **3** | Android and web fully tool-free (W-DEP 6.1, 6.5) incl. wizard | Live E2E on A51 + TCL TV with **no adb on PATH**; `doctor` green |
| **4** | TUI, jobs/daemon, parallel/background, test explorer | W-CLI acceptance, PTY tests, live multi-device concurrent run + detach/attach |
| **5** | LSP for the new DSL + standalone extension + shared corpus | W-EDITOR parity gate in CI |
| **6** | webOS and Tizen transports + packaging + signing (fixture/trace tested) | Unit/replay tests green; hardware runbooks; STATUS marks unverified |
| **7** | Apple: pairing/DDI/install/XCTest launch/sign via `idevice` + `apple-codesign`, macOS CI job producing the runner artifact | Replay + fake-lockdown tests green; honest STATUS; runbook for first real device |
| **8** | Docs sweep (`README`, `IMPRESARIO.md`, `DSL`, `PROTOCOL`, `DEVICES`, `PARITY`, `ERRORS`, `CLI`, `SIGNING`, `STATUS`), clean-VM `doctor` run, release packaging for Win/macOS/Linux (cargo-dist or equivalent), version bump | Definition of done below |

Phases 3 and 6/7 may be reordered by the Phase 0 verdicts, with the reason recorded.

## 11. Definition of done
1. A user on a clean Windows machine installs **one archive**, runs `impresario`, is walked through
   creating a device (Android over Wi-Fi) and an app, writes/edit a play using scene/theatre/play
   files with completion and duplicate detection (LSP **and** standalone), runs it, sends it to the
   background, starts a second job on another device, and reads both reports — with **no adb**, no
   Xcode, no ares, no sdb, no tizen CLI on the machine.
2. Apple and Tizen signing flows exist end to end in code, are covered by replay/fixture tests, and
   `docs/STATUS.md` states exactly which steps were and were not seen working on real hardware.
3. `impresario doctor` is accurate on a clean VM; every tolerated prerequisite is explicit.
4. All gates green: `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace`, Android JVM
   tests, `swift test` (Linux), extension tests, shared corpus parity, `parity_doc`.
5. `docs/DECISIONS.md` (D-19…, I-91…), `SPIKES.md`, `STATUS.md`, `ERRORS.md`, `THIRD_PARTY.md`
   updated; `IMPRESARIO.md` rewritten for the new shape.

## 12. Things the agent must ask the user instead of guessing
- Which Apple account type will be used (paid team / free Apple ID / existing `.p12`+profiles) and
  whether Apple ID automation (ToS-grey sideloading flow) is acceptable.
- Whether a Mac, an iPhone/Apple TV, a webOS TV or a Samsung Tizen TV can be made available for
  hardware verification, and when.
- Whether a managed download (ladder rung 3) is acceptable for Chromium/DDI assets or everything
  must be embedded in the release archive (size trade-off).
- Config format switch to TOML if lossless YAML editing proves unavailable (§4.2).

---

# Part III - impresario third generation (probes, resident runner, actors)

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

## 13. Clarifications (answered; binding)

These are the answers the user gave for the former open items. They are
binding decisions, recorded as D-34...D-43 in `docs/DECISIONS.md`.

1. **Scenes' `Platforms:` header** stays. It declares the selector columns of
   the scene; only plays/acts move to `Actors:` (D-34).
2. **Tag propagation**: tags declared on theatres/stages propagate transitively
   to the acts that perform them (D-35). Play to act inheritance and
   aggregation remain as in Part II.
3. **Audience granularity**: `Audience:` is file-level on theatres and may be
   narrowed per stage (D-36).
4. **`Showing ... with` options**: `coverage`, `decoded` and `source` are the
   supported options (D-37). No other options are invented.
5. **Unverifiable default** is `fail`; `warn` is opt-in per run via
   `--unverifiable` or in config (D-38).
6. **Watchers** support two modes: no-gap mode holds the device exclusively
   for the watcher lifetime; gap mode takes a per-iteration lease and releases
   it between iterations (D-39). The mode is explicit in watcher configuration;
   no hidden default may silently starve scheduled runs.
7. **Acceptance material** is public streaming sources plus local fixtures.
   No Netflix account, FairPlay asset or private license proxy is required or
   used. Hardware-specific paths that cannot be exercised here stay marked
   unverified, never "passed" (D-40).
8. **The optional in-app hook** is implemented as full SDKs (Android, Swift
   and JavaScript) with reference fixtures, not only a contract (D-41).
9. **Tray icon** is optional best-effort: implement it only where it is
   low-risk on the platform; its absence must never affect a run (D-42).

### 13.1 Environment facts for acceptance

- The Android phone/TV evidence remains the 2026-10-18/21 runs; this window
  adds no Android hardware. Device work always uses the project's native
  `libadb` transport, never a host `adb` process (D-19).
- On 2026-10-25 a Samsung Tizen TV (`192.168.93.37`), an LG webOS TV
  (`192.168.93.116`) and an Apple TV (`192.168.93.19`) are reachable. Tizen
  completed `device check` over native SDB (handshake without an AUTH
  challenge, capabilities, DUID `P7CIFS3MU3C26`, 864 apps, Tizen 9.0). A
  temporary self-signed profile then exercised the real install path: the TV
  accepted the per-WRTE-acknowledged upload and `0 vd_appinstall` for package
  `Impresario`, reached certificate validation, and rejected the chain
  (`[118, -12] Invalid certificate chain`), so a Samsung-issued profile is
  still required. The LG key server was fetched but SSH authentication lacks
  the Developer Mode passphrase; Apple identity is readable over AirPlay
  (`AppleTV5,3`, build `23J582`). No app has been installed, launched or
  played, and no probe has run on these TVs. The HD ledger (`HD-1`...`HD-8`,
  `I-133`) stays deferred until it does, and reachability alone is never
  reported as hardware verification.
- No local macOS/Xcode host is attached. Apple simulator execution is out of
  scope by user direction; physical tvOS is the active P0 path. The simulator
  workflow and helper scripts were removed, and no simulator or hardware-tested
  claim is made. Xcode targets are compiled by the unsigned `Apple Xcode` CI
  job only. WSL Swift builds cover the `AgentCore`/`AgentService` package on
  Linux. Physical tvOS pairing, free-Apple-ID signing, install, local-network
  permission and the remote focus path remain open (HD-1/I-149).

---

# Appendix A - consolidation rules and conflict resolutions

This file is the single authoritative TASK. Its parts are ordered by
precedence:

1. **Part III (third generation) overrides Part II and Part I.**
2. **Part II overrides Part I.**
3. Within a part, a later numbered decision overrides an earlier conflicting
   one; `docs/DECISIONS.md` remains the chronological decision record and is
   never rewritten (superseded rows say so).
4. References inside the parts to `TASK.md`, `TASK-1.md`, `TASK-2.md` and
   `TASK-3.md` mean Part I, Part I, Part II and Part III of this file
   respectively.

Explicit conflict resolutions:

- **Play/act platform selection**: Part III's `Actors:` replaces
  `Platforms:` on plays/acts (D-29). Scenes keep `Platforms:` as column
  declarations (D-34).
- **Stage uniqueness**: Part III's implicit `Perform` interface replaces
  Part II's globally-unique-stage-name rule (D-30). Uniqueness is per
  `(stage name, audience platform)`, and every implementation of one name must
  have the identical parameter signature.
- **Audience matching**: exact platform match, no webos-to-web or
  androidtv-to-android fallback (D-31). The selector fallback chain is
  untouched.
- **Probe verdicts**: tri-state `PASS`/`FAIL`/`UNVERIFIABLE`, with evidence
  classes and one verdict implementation in `impresario-core::probe`
  (D-26/D-27). Executors collect evidence; the master recomputes verdicts so
  native executors cannot drift.
- **Probe command shape**: one `Probe` wire command with a `kind` field
  (`Showing`, `Player`, `Drm`), not three RPCs (D-28).
- **`is visible` versus `Showing`**: `is visible` keeps its tree/attribute
  meaning; `Showing` requires rendered, decoded and unoccluded pixels/frames
  (D-33).
- **Everything in Part I that Part II/III do not replace remains binding**:
  action semantics, selector grammar, error taxonomy, report and exit-code
  rules, protocol rules, transport invariants and the testing gates.
- **D-19 remains binding**: no runtime invocation of external `ares-*`,
  `ssh`, `scp`, `sdb`, `tizen`, `openssl`, `ar`, `tar`, `gzip`, `adb`,
  `xcodebuild`, `xcrun`, `simctl`, `devicectl` or `ideviceinstaller`.
  Platform work is a native implementation or an embedded/managed payload.
- **Phase 6 completion is preserved.** The native webOS SSH/SFTP/luna and
  Tizen SDB transports, deterministic packaging, RSA/ECDSA XML-DSig signing,
  profile import, native probes, CLI integration and replay tests are already
  implemented (I-138...I-145). New probe work is additive and must not regress
  them.
- **No unverified claim is a success.** Fixtures/replay can prove code paths;
  hardware acceptance remains deferred and honest in `docs/STATUS.md`.

# Appendix B - current phase state at consolidation

Completed and committed before the third-generation work: Phases 0-5
(`41e57e7`).

Completed in the working tree at consolidation: Part II Phase 6 (native webOS
and Tizen transports, packaging, signing, diagnostics, hardening and docs),
plus the third-generation core probe model, proto/capabilities, connector
probes, DSL `Showing`/`Player`, policy plumbing and report status.

Outstanding at consolidation (tracked in `docs/STATUS.md` and the HD ledger):
Android/Apple native probe collectors and media-checking stubs, web/Tizen/webOS
media fixtures, the resident runner, actors/audience/interface DSL migration,
LSP/TUI/MCP parity, the full in-app SDKs, and all hardware-gated acceptance
(`HD-1`...`HD-8`, `I-133`). Phase ordering is Part III section 11 unless a
recorded decision reorders it.

## State after the playback-probe work (2026-10-25)

Completed on fixture/replay evidence since consolidation: the core evidence
model and verdict policy; the single `Probe` wire command and capabilities;
master canonicalization and `--unverifiable`; web/webOS/Tizen connector
collectors with video, audio-only and image fixtures plus a Tizen AVPlay parser
fixture; Android accessibility/screenshot/MediaSession/DRM and hook collectors
with JVM tests; Apple XCUITest coverage/hook collection with Swift tests; and
the web, Swift and Android in-app SDKs (D-41/D-43).

Still outstanding: the resident runner and watchers, the
actors/audience/interface DSL migration, LSP/TUI/MCP parity, and all
hardware-gated acceptance. Phase 6's native webOS/Tizen transports, packaging
and signing remain intact. See `docs/STATUS.md` for evidence and limits.

## Apple execution scope (2026-10-26 user direction)

The user rejected Apple simulator testing; physical tvOS is now the active P0
path. The simulator workflow, helper scripts, smoke test and CI job were
removed, and no Apple simulator or hardware execution is claimed. The tvOS
simulator loopback branch in config/`AppleTransport` remains as untested
support; it is not evidence. The Apple TV at `192.168.93.19` is reachable over
AirPlay, but this Windows host has no macOS/Xcode, Apple Mobile Device Support,
paired UDID or signed runner. Pairing, the free-7-day Apple ID signing flow,
install and the remote focus path still require a macOS host on the LAN and
remain HD-1/deferred. The unsigned `Apple Xcode` compile job and the Linux
Swift package tests remain valid build/unit-test evidence. The
actors/groups/audience Phase 2 work is parked, not abandoned, on
`master` (merged 2026-10-27).
