# The impresario DSL

Tests are written as two kinds of plain-text file:

| File | Purpose |
|---|---|
| `*.scene` | One screen. Maps logical element names (**Pieces**) to per-platform selectors in a table. |
| `*.theatre` | Reusable **Stages**, composed of Scene blocks and nested `Perform:` calls. |
| `*.play` | One test file. Contains **Acts** (test cases) that perform Stages, optionally with an `Examples:` table. |

All three formats follow the lexical rules below. `impresario validate` checks a
whole workspace, `impresario parse <file> [--json]` checks one file. Every
diagnostic has a stable code documented in [ERRORS.md](ERRORS.md).

---

## 1. Lexical rules

- **Encoding**: UTF-8; LF or CRLF line endings; a leading UTF-8 BOM is
  tolerated and stripped.
- **Indentation**: spaces only. A tab in the indentation is `IMP-DSL-001`.
  The first indented line of a file fixes the indent unit (4 spaces in all
  examples); every further indentation must be a multiple of it
  (`IMP-DSL-002`). Indentation carries the block structure: depth 0, 1 and 2.
- **Comments**: a line whose first non-space characters are `//`. Comments
  and blank lines are ignored and may appear anywhere. `#` is **not** a
  comment (it is valid CSS, so it must stay usable inside selectors).
- **Strings**: single (`'...'`) or double (`"..."`) quoted. The escapes are
  `\\`, `\'`, `\"`, `\n`, `\t` and `\u{XXXX}` (1–6 hex digits, e.g.
  `\u{1F600}`). Any other escape is an error. A string may not span lines; an
  unterminated string or invalid escape is `IMP-DSL-003`.
- **Interpolation** (inside strings only):
  - `${env.NAME}` — process environment variable `NAME` (master side),
  - `${var.NAME}` — play variable declared with `Var: NAME = '...'`.
  Anything that is not exactly `${env.IDENT}` or `${var.IDENT}` (unknown
  namespace, bad identifier, missing `}`) stays literal text (I-03 in
  [DECISIONS.md](DECISIONS.md)). An undefined `${var.X}` is `IMP-DSL-014`,
  reported statically at the reference; `${env.X}` is resolved at plan time
  and reports the variable name, never a value.
- **Durations**: integer + unit, no spaces: `500ms`, `2s`, `1m`. Range
  `1ms..=10m`; `0ms`, `11m`, `5x`, `1.5s` are `IMP-DSL-015`.
- **Identifiers** (scene, piece, variable and tag names): must start with an
  ASCII letter; scene/piece/variable names continue with ASCII letters,
  digits and `_`. Tags are `[a-z0-9_-]+`. UpperCamelCase is recommended for
  scenes and pieces (warning `IMP-DSL-020`).
- **Free text** (`Play:`, `Description:`, `Act:`, `Stage:` values): the rest
  of the line, trimmed, 1..=200 characters.
- **Keywords are case-sensitive**: `Scene Platforms Play Description App Var Act
  Tags Theatre Stage Timeout Perform Examples Action Check`.

---

## 2. Scene files

A scene is one table. The optional `Platforms:` line controls where the
**scene applies** and defaults to `cross-platform`; it is independent of which
platform columns the table happens to contain.

```text
Scene: LoginScreen
Platforms: cross-platform

| Piece         | web                   | android        | ios                         |
|---------------|-----------------------|----------------|-----------------------------|
| LoginTitle    | h1.title              | id:login_title | accessibilityId:login_title |
| UsernameInput | #username             | id:username    | accessibilityId:username    |
| SubmitButton  | button[type='submit'] | id:login_btn   | accessibilityId:login      |
```

- The first logical line is `Scene: <Name>`; `Platforms:` may follow directly
  before the table. Anything else in those positions is `IMP-DSL-005`; an
  unknown keyword elsewhere is `IMP-DSL-004` (with a "did you mean" hint).
- The first table column is `Piece`; every further column is a platform key.
  A `-` cell or a missing column means “no selector written here”, so the
  D-07 fallback chain applies.
- `Scene:` names the scene; the **file name is irrelevant**, but a mismatch
  with the file stem is the warning `IMP-DSL-021` (name files after scenes,
  e.g. `LoginScreen.scene`). Scene names must be unique across the workspace
  (`IMP-DSL-006`); piece names must be unique within one scene
  (`IMP-DSL-007`).
- `Platforms:` is either `cross-platform` (all devices) or a comma list of
  platform names (`web, webos, tizen, android, androidtv, ios, tvos`). A
  scene applies to a device iff it is `cross-platform` or lists the device's
  platform **exactly** — no implicit family matching (`webos` does not
  "include" `web` here). A play step on a device the scene does not apply to
  is `IMP-DSL-017`, raised per device by `validate` and by the planner.
- A piece used by a play step on a device whose selector is missing after the
  fallback chain below is `IMP-DSL-012`.
- The pre-release parser accepts only this table form. `Pieces:` blocks are
  rejected with `IMP-DSL-005`; there is no compatibility path.

**Fallback chain (D-07)**: `webos→web`, `tizen→web`, `androidtv→android`,
`tvos→ios`. The key itself is always tried first.

### Selector grammar by key (section 5.3)

Prefix parsing rule: a prefix is recognised only when the value starts with
exactly one of the listed lowercase/camelCase tokens followed by `:`.
Anything else is a bare value (web keys only).

| Key(s) | Syntax | Wire strategy |
|---|---|---|
| `web`, `webos`, `tizen` | bare value = CSS; `css:<css>`; `xpath:<xpath>`; `text:<exact visible text>`; `textContains:<substring>` | `CSS`, `XPATH`, `TEXT`, `TEXT_CONTAINS` |
| `android`, `androidtv` | prefix **required**: `id:<resource>`, `text:<exact>`, `textContains:<s>`, `desc:<content-desc>`, `class:<fqcn>` | `ANDROID_ID`, `TEXT`, `TEXT_CONTAINS`, `ANDROID_DESC`, `ANDROID_CLASS` |
| `ios`, `tvos` | prefix **required**: `accessibilityId:<id>`, `label:<exact>`, `labelContains:<s>`, `predicate:<NSPredicate format>`, `type:<ElementType>` | `ACCESSIBILITY_ID`, `LABEL`, `LABEL_CONTAINS`, `PREDICATE`, `IOS_TYPE` |

Notes:

- Android `id:` accepts a full resource id (`com.pkg:id/name`) or a short
  name, which the agent expands with the bound app's Android `id` package. Android
  has no XPath.
- Apple `type:` accepts `XCUIElement.ElementType` names in lowerCamel
  (`button`, `textField`, `secureTextField`, `staticText`, `cell`, `image`,
  `switch`, `other`, ...); an unknown name is `IMP-DSL-011`.

---

## 3. Play files

A play contains Acts. Each Act performs one or more reusable stages defined in
`.theatre` files. Scenes are tables (section 2); there is no inline `Stage:`
form in a play — it is rejected with `IMP-DSL-005`.

Structure:

- Depth 0: `Play`, `Description`, `App`, `Var`, `Act`, `Tags`, `Platforms`.
- Depth 1 (inside an Act): `Examples` and `Perform`.
- `Play:` is the first non-comment line. `Description:` may follow directly.
  `App: <name>` may follow `Play:`/`Description:` and must precede the first
  `Var:`; it binds the play to an app from `impresario.apps.config` (or the
  global layer). `--app` overrides it; two selected plays binding different
  apps is `IMP-APP-004` unless `--app` resolves it.
- `Var:` declarations follow before the first `Act:`.
- `Tags:` and `Platforms:` must directly follow their `Act:` line.
- An Act has at least one `Perform:` call. Act names are unique within the
  play (`IMP-DSL-016`).
- `Tags:` is a comma list of `[a-z0-9_-]+` used by `run --tag/--exclude-tag`.
- `Platforms:` is a comma list of platform names. When present, the Act runs
  only on devices of those platforms and is reported `skipped` elsewhere.
- `Var: NAME = 'value'` defines a play variable (a string: interpolation and
  escapes apply). `--var NAME=value` on the command line overrides it. A
  duplicate `Var:` for the same name keeps the **last** declaration (I-04).
- A referenced stage must exist (`IMP-DSL-031`); arguments and parameters are
  checked with `IMP-DSL-032`/`035`/`036`/`037`.

### 3.1 Theatres and stages

Reusable stages live in `.theatre` files. Stage names are globally unique
across the workspace; theatre names are unique as well. A stage may mix Scene
blocks and nested `Perform:` calls in source order. `Timeout:` (optional, at
most once, before the first Scene block) is the default poll timeout for the
steps of its scene blocks. `Audience:` (optional, theatre or stage level) is a
comma list of platforms, or `cross-platform`, that narrows where the stage is
available. The old stage-level `Platforms:` spelling is rejected.

```text
Theatre: User Authentication Feature stages

Stage: Navigate and Login
    Timeout: 15s
    Scene: LoginScreen
        Check: LoginTitle
        Action: '${var.USER}' => UsernameInput
        Action: click => SubmitButton

Stage: Fill(user, password = 'secret')
    Scene: LoginScreen
        Action: '${param.user}' => UsernameInput
        Action: '${param.password}' => PasswordInput

Stage: Login(user, password = 'secret')
    Perform: Navigate and Login
    Perform: Fill(user = '${param.user}', password = '${param.password}')
```

A play's Act performs stages and may carry one `Examples:` table. Each row is
a separate Act execution:

```text
Play: User Authentication Feature
Description: Validating secure user entry across platforms.
App: shop
Var: USER = 'test_user_impresario'

Act: Successful Login Flow(user = 'alice', password = 'secret')
Tags: smoke, auth
Platforms: android, ios, webos
    Examples:
        | user  | password |
        | alice | wonderland |
        | bob   | builder    |
    Perform: Login(user = '${example.user}', password = '${example.password}')
```

- Stage and Act parameters are identifiers; a parameter may have a string
  default. `Perform: Stage(name = 'value', ...)` binds parameters by name.
- A stage reference may be written unqualified (`Login`) or
  theatre-qualified (`Authentication.Login`); an exact stage name containing
  a dot wins, otherwise the theatre and stage must both exist. Stage names
  remain globally unique, so qualification is for readability only (I-117).
- `${param.NAME}` interpolates a stage/Act parameter. `${example.NAME}`
  interpolates the current `Examples:` cell (play Acts only). `${var.NAME}`
  and `${env.NAME}` work as before.
- `--arg 'Act.param=value'` (repeatable) overrides a same-named Examples
  column and the declared default. One execution per Examples row is kept.
- A stage called from another stage sees its own parameters; arguments are
  checked for unknown names (`IMP-DSL-032`) and missing required parameters
  (`IMP-DSL-037`). Cycles (`IMP-DSL-034`) and nesting deeper than 64
  (`IMP-DSL-041`) are rejected before any device is touched.
- A stage whose referenced scene does not apply to the current device is
  skipped as a whole; no partial stage prefix runs.
- Interpolation is defined for quoted strings only: action text, `Perform:`
  argument values and parameter defaults. Durations, timeouts, counts,
  selectors and platform lists must be literals; `${...}` in those positions
  is left as text and rejected by their normal grammar (I-117).
- `impresario fmt` is the canonical writer for all three file kinds (I-111).

### 3.2 Playback probes (`Showing`, `Player`)

A `Check:` may inspect media truth instead of an element property. Probes are
tri-state (`pass`, `fail`, `unverifiable`), not simple booleans:

```text
Check: Showing => PlayerRegion
Check: not Showing => Poster
Check: Showing => PlayerRegion with coverage>=50%, decoded, source~".m3u8" timeout 12s

Check: Player => VideoContainer Playing
Check: Player => VideoContainer not Paused DRM-enabled Widevine
Check: Player => VideoContainer DRM-disabled
Check: Player => VideoContainer Playing with position>30s, resolution>=1080p, muted
Check: Player => VideoContainer DRM-enabled with license=active, not keyStatus=expired
```

`Showing` means the selected piece's pixels/frames are really being presented
(not merely present in the DOM/tree). Its `with` options are
`coverage>=N%` (measured painted region), `decoded` and
`source~"substring"`; `source` is web-only (`IMP-DSL-013` elsewhere).
`Player` accepts one status, `DRM-enabled`/`DRM-disabled` with an optional
provider, and `with` assertions on probe properties. A bare property means
truthy; `not` negates one term. Values are canonicalized: durations to
seconds, percentages to fractions, `1080p`/`4k`/`8k` to numeric height.

The default required confidence is `corroborated`. Missing evidence is
`unverifiable`; it never becomes a pass. `--unverifiable warn` makes those
steps non-fatal for one run. Evidence sources, per-platform reachability and
the optional in-app SDK are documented in `docs/PROBES.md`; the generated
property/provider matrix is in `docs/PARITY.md`.

---

## 4. Grammar (EBNF)

`INDENT`/`DEDENT` are produced by the scanner; `EOL` ends a logical line.

```ebnf
scene-file    = "Scene:" , ident , EOL , [ platforms-line ] , EOL? , scene-table ;
platforms-line= "Platforms:" , ( "cross-platform" | platform , { "," , platform } ) , EOL ;
platform      = "web" | "webos" | "tizen" | "android" | "androidtv" | "ios" | "tvos" ;

table         = header-row , separator-row , { data-row } ;
header-row    = "|" , text , { "|" , text } , "|" , EOL ;
data-row      = "|" , text , { "|" , text } , "|" , EOL ;
scene-table   = table ;  (* first header cell is "Piece", then platform keys *)

theatre-file  = "Theatre:" , text , EOL , [ "Description:" , text , EOL ] , stage , { stage } ;
stage         = "Stage:" , text , [ "(" , param , { "," , param } , ")" ] , EOL
              , INDENT , [ "Timeout:" , duration , EOL ]
              , { ( scene-block | perform-line ) } , DEDENT ;
param         = ident , [ "=" , string ] ;
perform-line  = "Perform:" , text , [ "(" , argument , { "," , argument } , ")" ] , EOL ;
argument      = ident , "=" , string ;

play-file     = "Play:" , text , EOL , [ "Description:" , text , EOL ] , [ "App:" , ident , EOL ] , { var-decl } , act , { act } ;
var-decl      = "Var:" , ident , "=" , string , EOL ;
act           = "Act:" , text , [ "(" , param , { "," , param } , ")" ] , EOL
              , [ "Tags:" , tag , { "," , tag } , EOL ]
              , [ platforms-line ]
              , [ examples ]
              , perform-line , { perform-line } ;
examples      = "Examples:" , EOL , INDENT , table , DEDENT ;

scene-block   = "Scene:" , ident , EOL , INDENT , step , { step } , DEDENT ;
step          = action-line | check-line ;

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
check-line   = "Check:" , ( probe-check | piece , [ cond ] ) , [ tmo ] , EOL ;
probe-check  = [ "not" ] , "Showing" , "=>" , piece , [ "with" , show-opt , { "," , show-opt } ]
             | "Player" , "=>" , piece , { player-term } , [ "with" , assertion , { "," , assertion } ] ;
show-opt     = "coverage" , ">=" , integer , "%" | "decoded" | "source" , "~" , dq-string ;
player-term  = [ "not" ] , status | ( "DRM-enabled" | "DRM-disabled" ) , [ [ "not" ] , provider ] ;
assertion    = [ "not" ] , property , [ ( "=" | "!=" | ">=" | "<=" | ">" | "<" | "~" ) , value ] ;
cond         = "is" , ( "visible" | "hidden" | "enabled" | "disabled" | "focused" )
             | "text" , string | "textContains" , string | "count" , integer ;
tmo          = "timeout" , duration ;
direction    = "up" | "down" | "left" | "right" ;
keyname      = "ok"|"enter"|"back"|"up"|"down"|"left"|"right"|"home"|"menu"|"play"|"pause"|"playpause"
             | "rewind"|"fastforward"|"red"|"green"|"yellow"|"blue"|"backspace"
             | "num0"|"num1"|"num2"|"num3"|"num4"|"num5"|"num6"|"num7"|"num8"|"num9"
             | "volumeup"|"volumedown"|"mute"|"channelup"|"channeldown"
             | "stop"|"record"|"info"|"guide"|"exit"|"subtitle"|"audio"|"search"|"settings" ;
piece        = ident ;
```

The table header and separator cells may be padded for readability; the
formatter pads them to display width. Every data row must have the same
number of cells (`IMP-DSL-039`); duplicate columns are `IMP-DSL-038` and an
unknown `${example.X}` column is `IMP-DSL-040`. A stage/theatre name defined
twice is `IMP-DSL-030`/`IMP-DSL-033`.

An unknown verb is `IMP-DSL-010` (with a hint); an unknown key name is
`IMP-DSL-018`. `tmo` is not allowed on `key`, `back`, `wait`, `screenshot`
and `app` (`IMP-DSL-022`). `Check: P` with no condition means
`is visible`.

---

## 5. Step semantics (normative)

**Timeout precedence** (highest first): step `timeout` > Stage `Timeout:` >
CLI `--timeout` > `timeout_default_ms` from `impresario.config`. The timeout
is the maximum time the executor polls for the element/condition (poll
interval `poll_interval_ms`). It is not the total step budget.

### Direction semantics (they differ on purpose)

- `scroll <dir>` names the direction **in which you want to reach more
  content**: `scroll down` reveals content further down, which on a touch
  screen is a finger moving **up**.
- `swipe <dir>` names the direction of **finger movement**: `swipe up` moves
  the finger up (revealing content below on scrollable lists).

`scroll <dir> [n] [=> P]` performs `n` scroll steps (default 1; one step is
≈80% of the container's visible extent on touch platforms, one D-pad press
or one page-unit on TV platforms) inside container `P`, or on the primary
scrollable of the screen when omitted. `scrollto => P` repeats scroll steps
(down first, then up if the element was not found and the container reached
its end) up to `scroll_max_attempts` until `P` is visible; failure is
`ELEMENT_NOT_FOUND` with `details.scroll_attempts`.

### Text entry (`type`, shorthand, `append`, `clear`)

The three verbs are distinct and exact (D-06):

| Form | Final field content |
|---|---|
| `'abc' => P` = `type 'abc' => P` | exactly `abc`: focus, clear existing content, enter `abc` |
| `append 'abc' => P` | previous content + `abc`: focus, keep content, add at the end |
| `clear => P` | empty string |

`type` is atomic "replace": the executor must fail with `NOT_INTERACTABLE`
(never silently succeed) when the element is not editable. On web, `input`
and `change` events are dispatched after every entry so frameworks
(React/Vue/Angular) observe the value. Interpolation is resolved by the
master before the command is sent; `action.text` on the wire is always the
resolved string.

### Click / tap

Waits (up to the timeout) for the element to exist, be visible and be
enabled, then taps its center (touch/pointer platforms) or focuses it and
presses OK/Select (TV platforms). Disabled or obscured at timeout:
`NOT_INTERACTABLE`; never exists: `ELEMENT_NOT_FOUND`.

### Select

`select 'Option' => P`: web sets the `<select>` value whose text equals the
option (then dispatches `input`/`change`); Android clicks `P` then the
element with that text; iOS/tvOS uses the picker wheel or taps the menu item
with that label. Option not found: `ELEMENT_NOT_FOUND` with
`details.option`.

### Checks

Checks always auto-wait (D-09): they poll until the condition holds or the
timeout elapses. There is no `waitfor`.

- `visible`: element exists and is displayed. `hidden`: element does not
  exist or is not displayed. `enabled`/`disabled`: element exists and the
  flag matches. `focused`: element exists and has focus.
- `text 'x'`: element text equals `x` exactly (after the platform text rule
  in the spec, section 6.4). `textContains 'x'`: substring. `count N`: the
  selector matches exactly N elements (N may be 0).
- A check fails with `CHECK_FAILED` when the timeout elapses; `details`
  carry `expected`, `observed` and `matched_count`. Checks never return
  `ELEMENT_NOT_FOUND`.

### Remote/hardware keys

`key <name> [count]` sends `RemoteKey` presses (count is the repeat, default
1). Not every key exists on every platform (e.g. `ok` only on Android TV and
tvOS; `home` only on iOS/tvOS; colour keys only on TV platforms); the
planner raises `IMP-DSL-013` before any device is touched. The full matrix is
in [PARITY.md](PARITY.md).

### Master-side steps (never sent on the wire)

- `wait <duration>` sleeps on the master (warning `IMP-DSL-019`; prefer a
  check with a timeout).
- `screenshot 'name'` sends a `Screenshot` command; the master writes the
  PNG to `<report_output>/screenshots/<run-id>/<device>/<name>.png`, with the
  name sanitized to `[A-Za-z0-9._-]` and at most 80 characters.

### App control

`app launch | terminate | restart` maps to `AppControl`. Support per
platform is part of the parity matrix; `restart` is implemented as
terminate + launch by the executors.

### Secrets

Reports, logs and the console always show the **source text** of a step as
written (`'${env.TEST_PASSWORD}'`), never the resolved string. Any observed
text equal to a resolved `${env.*}` value is replaced by `***` in reports
and logs (implementation lands with the reporters; see spec section 5.6).

---

## 6. Lints and diagnostics

Errors prevent execution; lints are warnings:

| Code | Meaning |
|---|---|
| `IMP-DSL-019` | lint: fixed wait (prefer a check with a timeout) |
| `IMP-DSL-020` | lint: scene/piece name is not UpperCamelCase |
| `IMP-DSL-021` | lint: scene name does not match the file name |
| `IMP-DSL-042` | lint: scene is never used by a stage |
| `IMP-DSL-043` | lint: piece is never used by a step |
| `IMP-DSL-044` | lint: stage is never performed by an Act |
| `IMP-DSL-045` | lint: theatre has no stage reachable from an Act |
| `IMP-DSL-046` | lint: duplicate `Audience:` declaration (the last one wins) |
| `IMP-DSL-057` | lint: variable is never referenced |
| `IMP-DSL-047` | lint: `Examples:` column is never used |
| `IMP-DSL-048` | lint: duplicate `Var:` declaration (the last one wins) |
| `IMP-DSL-049` | lint: Act without `Platforms:` uses a piece with no selector for a configured platform (add `Platforms:` or selectors) |
| `IMP-DSL-050` | lint: required stage parameter is omitted by every `Perform:` call |

The “Act without `Platforms:` that uses a piece unavailable on a configured
platform” rule is enforced as the existing per-platform `IMP-DSL-012` error,
which is stronger than a lint and names the device/platform.

The complete code catalog (including the configuration and runtime codes) is
in [ERRORS.md](ERRORS.md). Examples for every code live in the parser test
suite (`crates/impresario-core/tests/negative_cases.rs`).

---

## 7. A complete walk-through

`impresario workspace new --example` writes this workspace (also committed under `examples/`):

```text
impresario.config
impresario.devices.config
impresario.apps.config
scenes/LoginScreen.scene
scenes/DashboardScreen.scene
theatres/login.theatre
plays/login.play
```

`impresario validate` parses all three file kinds, resolves references and
applies the lints per configured device. `impresario parse
examples/plays/login.play --json` dumps the typed AST (every node carries
`Span { file, line, col, len }`, columns are 1-based and count characters,
not bytes).

---

## 8. Formatting and Act arguments

```text
impresario fmt [PATHS...] [--check]
impresario run ... [--arg 'Act.param=value']...
```

- `fmt` defaults to the configured `scenes_dir`, `theatres_dir` and
  `plays_dir`. It is line-oriented: comments and intentional single blank
  lines survive, keyword casing and 4-space indentation are canonical,
  table columns are aligned by Unicode display width and absent cells are
  rendered as `-`. Formatting is idempotent; `--check` exits 1 and prints
  `would reformat <path>` without writing. A comment inside a table splits
  the alignment run (a documented limitation; the text and comment are
  preserved).
- `migrate` was removed before 1.0.0. The parser accepts only table scenes,
  `.theatre` stages and `Perform:` plays; old files fail with a diagnostic
  instead of being rewritten.
- `--arg ACT.PARAM=VALUE` is repeatable. Precedence is CLI `--arg`, then a
  same-named `Examples:` column, then the parameter default. Malformed pairs
  or names that do not match any selected Act parameter are diagnostic
  `IMP-DSL-032` before a device is touched.
- Reports carry the resolved Act arguments/example row in
  `ActReport.example`; values whose key contains `password`, `passwd`,
  `secret`, `token` or `pin` are shown as `***`, and all other values pass
  through the normal secret redactor.
