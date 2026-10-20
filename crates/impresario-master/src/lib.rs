#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::todo,
        clippy::unreachable
    )
)]

//! impresario-master: CLI, planner, executor, reporters, the embedded LSP
//! and the MCP server.

pub mod cli;
pub mod definitions;
pub mod doctor;
pub mod exec;
pub mod lsp;
pub mod mcp;
pub mod plan;
pub mod redact;
pub mod render;
pub mod report;
pub mod reporters;
pub mod time_fmt;
pub mod tools_cmd;
pub mod wizard;
