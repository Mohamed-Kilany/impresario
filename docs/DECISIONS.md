# Decision log

## Binding decisions (spec section 2, copied verbatim)

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

## Implementation decisions (made here; the spec leaves them open)

| # | Decision |
|---|---|
| I-01 | Edition 2021, `rust-toolchain.toml` channel `stable`; every dependency pinned to an exact version in the workspace `Cargo.toml`. |
| I-02 | Protoc resolution for `build.rs`, in order: `$PROTOC` as a file; `$PROTOC/bin/protoc[.exe]` (the environment here points `PROTOC` at a directory); `protoc-bin-vendored` for hermetic builds. `PROTOC` is set for the child process only. |
| I-03 | Interpolation recognises only the exact forms `${env.IDENT}` and `${var.IDENT}`. Any other `${`-sequence is literal text. |
| I-04 | A duplicate `Var:` declaration in one play is **last-wins** (documented, tested). |
| I-05 | `IMP-DSL-012` (missing selector), `IMP-DSL-013` (unsupported action/key/check) and `IMP-DSL-017` (scene not applicable) are reported by `validate` (against every distinct configured device platform) **and** by the planner at plan time. |
| I-06 | `buf.gen.yaml` is a documented placeholder: proto code generation is per-language (Rust `build.rs`, Gradle protobuf plugin, Swift scripts). `buf.yaml` disables the lint rules that conflict with the verbatim contract (`SERVICE_SUFFIX`, `ENUM_VALUE_PREFIX`, `PACKAGE_VERSION_SUFFIX`, `RPC_REQUEST_RESPONSE_UNIQUE`, `RPC_RESPONSE_STANDARD_NAME`). |
| I-07 | `docs/PARITY.md` is generated from `impresario-core::support` (single source of truth) so it can never drift from the planner; a consistency test lands with the docs work. |
| I-08 | `validate [PATH...]` with explicit paths validates exactly those files/directories; when no `.scene` file is among them but plays are, the configured `scenes_dir` is loaded as the scene index so play resolution works. |
| I-09 | `agent_address` is required for `tvos` devices: Bonjour discovery is not implemented, so no configuration can enable it (help text says so). |
| I-10 | Filesystem errors while reading DSL/config files are reported with `IMP-CFG-003` (the catalog has no dedicated code for I/O). |
| I-11 | `init` refuses to overwrite existing files without `--force` and exits `2`; created/updated paths are printed to stdout. |
| I-12 | Not-yet-implemented commands (`devices check`, `run`, `lsp`) fail loudly with a message and exit `2`. They are replaced phase by phase; the current state is in `docs/STATUS.md`. |
| I-13 | miette rendering happens at the CLI boundary only: `impresario-master::render` converts the character-based `Span` to a byte range, attaches the source text and a label. `core` stays without file contents. |
| I-14 | The "did you mean" helper (`suggest`) lives in `impresario-core::diag` and compares case-insensitively (edit distance on lowercased text, threshold 2). |
| I-15 | `timeout` is not accepted on `key`, `back`, `wait`, `screenshot`, `app` lines (`IMP-DSL-022`); the parser rejects it at parse time. |
| I-16 | Lint `IMP-DSL-020` applies to scene and piece names (UpperCamelCase recommended); the hint names the converted form. |
