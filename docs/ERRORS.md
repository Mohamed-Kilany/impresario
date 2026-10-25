# Diagnostic catalog

Every user-facing diagnostic carries a stable code. Codes never change; new
codes are appended. Rendered with `miette` (file, line, caret, help hint);
`impresario validate` reports **all** diagnostics of a file together.

## Configuration (`IMP-CFG-*`)

| Code | Meaning | Emitted by |
|---|---|---|
| `IMP-CFG-001` | unknown key (with a "did you mean" hint) | `impresario-core::config` |
| `IMP-CFG-002` | missing required field | `impresario-core::config` |
| `IMP-CFG-003` | invalid value (range, enum, type, kind of file) | `impresario-core::config`, CLI |
| `IMP-CFG-004` | duplicate device name | `impresario-core::config` |
| `IMP-CFG-005` | bad `target_endpoint` for the device platform | `impresario-core::config` |
| `IMP-CFG-006` | duplicate app name (within the merged app layers) | `impresario-core::apps` |

## Apps (`IMP-APP-*`)

| Code | Meaning | Emitted by |
|---|---|---|
| `IMP-APP-001` | a play's `App:` (or `--app`) names an app that is not defined | planner, `validate`, LSP |
| `IMP-APP-002` | the app has no entry (or no id) for a selected device's platform | planner, `app check` |
| `IMP-APP-003` | the app's `package` artifact does not exist at the resolved path | planner, `app check` |
| `IMP-APP-004` | selected plays bind different apps without `--app` | planner |

## Signing and packaging (`IMP-SIGN-*`)

| Code | Meaning | Emitted by |
|---|---|---|
| `IMP-SIGN-001` | package/ZIP layout error | `impresario package`, `sign tizen` |
| `IMP-SIGN-002` | signing profile not found | `sign tizen profile show/remove`, transport prepare |
| `IMP-SIGN-003` | PKCS#12 import failed | `sign tizen profile add` |
| `IMP-SIGN-004` | certificate expired or not yet valid | profile import and run-time prepare |
| `IMP-SIGN-005` | unsupported signing key or key/certificate mismatch | profile import and package signing |
| `IMP-SIGN-006` | TV DUID is not covered by the distributor certificate | transport prepare, `sign tizen package` |
| `IMP-SIGN-007` | XML-DSig validation failed | `sign tizen check`, cached-package validation |
| `IMP-SIGN-008` | profile password unavailable | profile import/signing (keychain or documented environment override) |
| `IMP-SIGN-009` | invalid profile/config metadata | profile handling and `.wgt` inspection |
| `IMP-SIGN-010` | signing I/O error | signing and packaging paths |

See `docs/SIGNING.md` for the full pipeline and the headless password
overrides.

## DSL (`IMP-DSL-*`)

| Code | Meaning | Example |
|---|---|---|
| `IMP-DSL-001` | tab indentation | a `\t` as the first indentation character |
| `IMP-DSL-002` | inconsistent indent | 4 spaces then 6 spaces |
| `IMP-DSL-003` | unterminated / invalid string | `Action: 'abc => P` |
| `IMP-DSL-004` | unknown keyword | `Steps:` instead of `Pieces:` |
| `IMP-DSL-005` | missing or misplaced header | `Pieces:` before `Scene:` |
| `IMP-DSL-006` | duplicate scene name (workspace-wide) | two files declaring `Scene: Login` |
| `IMP-DSL-007` | duplicate piece name in one scene | `Title` listed twice |
| `IMP-DSL-008` | unknown scene referenced by a play | `Scene: LoginScreeen` |
| `IMP-DSL-009` | unknown piece referenced in a scene block | `Check: LoginTitel` |
| `IMP-DSL-010` | unknown action verb | `Action: tap => P` |
| `IMP-DSL-011` | invalid selector for its key | `id:` with no value; android without prefix |
| `IMP-DSL-012` | piece has no selector for the device platform (after the D-07 fallback chain) | missing `android` selector |
| `IMP-DSL-013` | action, key or check not supported on the device platform (section 6.1/6.2) | `swipe` on `webos` |
| `IMP-DSL-014` | undefined `${var.X}` | `X` not declared by any `Var:` |
| `IMP-DSL-015` | invalid duration | `0ms`, `11m`, `5x` |
| `IMP-DSL-016` | duplicate Act/Stage name | two `Stage: Login` under one Act |
| `IMP-DSL-017` | scene not applicable to a device platform (exact match) | play uses a `tizen`-only scene on `android` |
| `IMP-DSL-018` | unknown key name in `Action: key <k>` | `key rewindd` |
| `IMP-DSL-019` | lint: fixed wait | `Action: wait 2s` |
| `IMP-DSL-020` | lint: naming (scene/piece not UpperCamelCase) | `login_screen` |
| `IMP-DSL-021` | lint: scene name does not match the file name | `Scene: Login` in `playlist.scene` |
| `IMP-DSL-022` | `timeout` not allowed here | `Action: back timeout 5s` |
| `IMP-DSL-030` | duplicate stage name across all theatres | two `Stage: Login` declarations |
| `IMP-DSL-031` | unknown stage referenced by `Perform:` | `Perform: LoginFlow` with no such stage |
| `IMP-DSL-032` | invalid stage arguments (unknown name, duplicate, bad `--arg`) | `Perform: Fill(usr = 'x')` |
| `IMP-DSL-033` | duplicate theatre name | two `Theatre: Auth` files |
| `IMP-DSL-034` | stage call cycle | A performs B, B performs A |
| `IMP-DSL-035` | duplicate parameter in one Act/Stage | `Stage: Fill(user, user)` |
| `IMP-DSL-036` | unknown parameter reference (`${param.X}`) | `X` is not declared by the Act/Stage |
| `IMP-DSL-037` | required stage parameter is not supplied | `Stage: Fill(user)` called as `Fill()` |
| `IMP-DSL-038` | duplicate `Examples:` column | `| user | user |` |
| `IMP-DSL-039` | `Examples:` row has a different cell count than the header | header 2 cells, row 3 cells |
| `IMP-DSL-040` | unknown `${example.X}` column | `X` not in the Act's `Examples:` header |
| `IMP-DSL-041` | stage nesting deeper than 64 | recursive stage composition |
| `IMP-DSL-042` | lint: scene is never used by a stage | unused `.scene` |
| `IMP-DSL-043` | lint: piece is never used by a step | unused table row / `Pieces:` entry |
| `IMP-DSL-044` | lint: stage is never reachable from an Act | unreferenced `.theatre` stage |
| `IMP-DSL-045` | lint: theatre has no stage reachable from an Act | unreferenced theatre |
| `IMP-DSL-046` | lint: variable is never referenced | unused `Var:` |
| `IMP-DSL-047` | lint: `Examples:` column is never used | column not bound to a parameter or `${example.*}` |
| `IMP-DSL-048` | lint: duplicate `Var:` declaration (last wins) | two `Var: USER` lines |
| `IMP-DSL-049` | lint: Act has no `Platforms:` but a used piece has no selector for a configured platform | Android-only piece in an Act planned for web and Android |
| `IMP-DSL-050` | lint: required stage parameter is omitted by every `Perform:` call | `Stage: Fill(user)` called as `Perform: Fill` everywhere |

## Notes

- `IMP-DSL-019..021` and `IMP-DSL-042..050` are warnings; everything else is an error.
- `IMP-DSL-049` is additive context for `IMP-DSL-012`: the latter still fails the
  step on the affected device, while the warning points at the Act and lists
  every missing piece so the author can add `Platforms:` or selectors.
- `IMP-DSL-050` is additive context for `IMP-DSL-037`: the first omitted
  argument is the error, and the warning highlights the declaration when no
  call site ever supplies it.
- `IMP-DSL-012`, `IMP-DSL-013` and `IMP-DSL-017` need the configured devices;
  they are raised by `validate` per device platform and again by the planner
  before any device is touched (I-05 in `docs/DECISIONS.md`).
- Runtime failures (element not found, check failed, transport errors) use the
  protocol error codes in `proto/impresario/v1/impresario.proto`
  (`ErrorCode`), not the `IMP-*` catalog.
