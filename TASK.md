# impresario: Implementation Specification (v2)

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
  - `AppleTransport` (ios, tvos): macOS hosts only (otherwise `DEVICE_UNAVAILABLE` with an explanation). iOS device: `iproxy <local>:<agent_port> -u <udid>` (libimobiledevice; clear error if missing); iOS simulator: direct `127.0.0.1:<agent_port>` (UDID is a UUID). tvOS: uses `agent_address` (network). Start the runner: `xcodebuild test-without-building -xctestrun <file> -destination 'id=<udid>' -only-testing:<bundle>/AgentRunnerTests/testServe -test-timeouts-enabled NO -parallel-testing-enabled NO` as a monitored child; wait for the port (and optionally Bonjour `_impresario._tcp`) up to `agent_start_timeout_ms`. Teardown: `Shutdown` → kill child (process group) → kill `iproxy`.
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
