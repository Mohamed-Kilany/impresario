//! Editor language service shared by the embedded LSP and the standalone
//! VS Code extension (WASM). The crate is deliberately free of async, LSP and
//! filesystem dependencies: callers provide the source buffers and workspace
//! configuration.

pub mod actions;
pub mod completion;
pub mod diagnostics;
pub mod formatting;
pub mod model;
pub mod navigation;
pub mod service;
pub mod text;
