#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unreachable
)]

//! Generates the `impresario.v1` prost/tonic types.
//!
//! `protoc` resolution order:
//! 1. `$PROTOC` from the environment (CI may provide one);
//! 2. the hermetic binary shipped by `protoc-bin-vendored`.
//!
//! The build fails loudly (exit 1) if either step fails; it never panics.

use std::path::{Path, PathBuf};

fn fail(msg: &str) -> ! {
    eprintln!("impresario-proto build failed: {msg}");
    std::process::exit(1)
}

/// Resolve a runnable `protoc`.
///
/// `$PROTOC` is honoured when it points at an executable file; when it points
/// at an install *directory* (common on Windows, where installers export the
/// directory), `<dir>/bin/protoc[.exe]` is tried next. Anything else falls
/// back to the hermetic binary shipped by `protoc-bin-vendored`.
fn resolve_protoc() -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("PROTOC") {
        let given = PathBuf::from(raw);
        if given.is_file() {
            return Ok(given);
        }
        for name in ["protoc.exe", "protoc"] {
            let candidate = given.join("bin").join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    protoc_bin_vendored::protoc_bin_path().map_err(|err| err.to_string())
}

fn is_file(path: &Path) -> bool {
    path.is_file()
}

fn main() {
    match resolve_protoc() {
        Ok(path) if is_file(&path) => std::env::set_var("PROTOC", &path),
        Ok(path) => fail(&format!(
            "resolved protoc is not a file: {}",
            path.display()
        )),
        Err(err) => fail(&format!("cannot locate protoc: {err}")),
    }

    let manifest_dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(err) => fail(&format!("CARGO_MANIFEST_DIR is not set: {err}")),
    };
    let proto_root = manifest_dir.join("../../proto");
    let proto_file = proto_root.join("impresario/v1/impresario.proto");
    if !proto_file.exists() {
        fail(&format!("proto file not found at {}", proto_file.display()));
    }

    println!("cargo:rerun-if-changed={}", proto_file.display());
    println!("cargo:rerun-if-env-changed=PROTOC");

    if let Err(err) = tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&[proto_file], &[proto_root])
    {
        fail(&format!("protobuf code generation failed: {err}"));
    }
}
