# Status

Current development status, per component. Updated at every phase boundary.
"Verified" means an automated check (test or command) ran successfully in this
repository on this machine (Windows 11, Rust stable 1.94.x).

## Environment constraints

- Host: Windows. No Swift/Xcode toolchain: the Apple agent cannot be built or
  run here. Apple sources will be complete and reviewed, but building/running
  them is out of reach; manual verification checklists are provided instead
  (spec sections 12 and 15).
- Phase 0 spike "gRPC (NIO) server inside an XCUITest runner" therefore cannot
  be executed here. D-14/D-18 remain **unverified** until someone runs the
  spike on a Mac; this is a recorded gap, not a silent one.
- Android SDK is present (`%LOCALAPPDATA%\Android\Sdk`) with an `android-36.1`
  x86_64 emulator image, JDK 24, Gradle wrapper distributions cached.

## Components

| Component | State | Verification |
|---|---|---|
| `proto/impresario/v1` + `crates/impresario-proto` | done | codegen verified via `cargo build` (vendored protoc fallback exercised) |
| `impresario-core` config (`config.rs`) | done | unit tests: spec examples, unknown keys w/ hints, ranges, endpoints per platform, options applicability, duplicates |
| `impresario-core` scanner/parser/AST | done | 111 lib tests; 94-case negative matrix (`tests/negative_cases.rs`, one per IMP-DSL code + edges); proptest never-panics suite (3 generators + nasty corpus); `insta` snapshots of the example ASTs and of a multi-error file |
| `impresario-core` semantic validation | done | unit tests: 008/009/012/013/014/017/019/020/021 with device awareness |
| `impresario-core` workspace loader | done | unit tests incl. duplicate scenes 006 |
| `impresario init` / `validate` / `parse` / `devices list` | done | CLI unit tests + integration test against the committed `examples/` workspace (`tests/parse_json.rs`: JSON stdout snapshot, exit codes) |
| `impresario devices check` | not implemented yet | fails loudly (exit 2) |
| `impresario run` (planner/executor) | not implemented yet | fails loudly (exit 2) |
| `impresario lsp` | not implemented yet | fails loudly (exit 2) |
| `impresario-clients` (transports, AgentClient) | stub | — |
| `impresario-connector` (raw CDP) | stub | — |
| Android agent (Kotlin) | not started | — |
| Apple agent (Swift) | not started | — |
| Reporters (console/junit/json/html) | not started | — |
| Docs (`docs/*.md`, examples, fixtures) | partially (DSL, DECISIONS, STATUS, ERRORS) | DSL.md covers syntax, semantics and selectors; PROTOCOL/DEVICES/PARITY/LSP pending later phases |
| CI workflows | not started | — |

## Known gaps and risks

1. Apple agent: cannot be compiled here (no Swift). Documented; manual
   checklists required (spec 15).
2. Phase 0 spike 2 (NIO gRPC in XCUITest): unverified on this host (see
   above); D-18 fallback design stays available if the spike fails on a Mac.
3. `impresario-connector` will be verified against a local headless Chrome
   (present on this machine) using the fixtures in `fixtures/web`.
4. Tizen/webOS hardware is not available; the connector's CDP layer is tested
   against Chrome and the protocol-relevant fake (recorded in tests).
5. The `cargo-fuzz` target (`fuzz/fuzz_targets/parse.rs`) needs a nightly
   toolchain and `cargo-fuzz`, which are not installed here; it is shipped
   unverified. The stable-toolchain proptest suite
   (`crates/impresario-core/tests/parser_fuzz.rs`) covers the same
   never-panics property on every `cargo test`.
