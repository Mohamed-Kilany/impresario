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

//! impresario-master: CLI, planner, executor, reporters and the embedded LSP.

pub mod cli;
pub mod exec;
pub mod plan;
pub mod redact;
pub mod render;
pub mod report;
pub mod reporters;
pub mod time_fmt;
