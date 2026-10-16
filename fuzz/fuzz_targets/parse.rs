#![no_main]
//! `cargo fuzz` target mirroring `crates/impresario-core/tests/parser_fuzz.rs`:
//! the parser and scanner must never panic on arbitrary input (spec 5.7).
//!
//! This target is NOT verified on this machine (it needs a nightly toolchain
//! and `cargo-fuzz`; see docs/STATUS.md). The stable-toolchain proptest suite
//! covers the same property in CI.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = impresario_core::parser::parse_scene(text, std::path::Path::new("fuzz.scene"));
        let _ = impresario_core::parser::parse_play(text, std::path::Path::new("fuzz.play"));
    }
});
