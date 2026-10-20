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

## Notes

- `IMP-DSL-019..021` are warnings; everything else is an error.
- `IMP-DSL-012`, `IMP-DSL-013` and `IMP-DSL-017` need the configured devices;
  they are raised by `validate` per device platform and again by the planner
  before any device is touched (I-05 in `docs/DECISIONS.md`).
- Runtime failures (element not found, check failed, transport errors) use the
  protocol error codes in `proto/impresario/v1/impresario.proto`
  (`ErrorCode`), not the `IMP-*` catalog.
