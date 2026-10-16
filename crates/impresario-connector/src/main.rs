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

//! impresario-connector CLI entry point.

fn main() -> std::process::ExitCode {
    // The connector server is implemented in a later phase; until then the
    // binary explains itself. Spec section 10.
    eprintln!(
        "impresario-connector: the CDP connector server is not built yet (see docs/CONNECTOR.md)"
    );
    std::process::ExitCode::FAILURE
}
