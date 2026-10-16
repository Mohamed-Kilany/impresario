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

//! The `impresario` CLI entry point.

fn main() {
    std::process::exit(impresario_master::cli::run());
}
