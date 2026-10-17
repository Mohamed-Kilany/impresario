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

//! `impresario-connector` entry point (spec 10): `serve`, `ping`, `pages`.
//! Exit codes: 0 success, 1 runtime failure, 2 usage error (decision I-36).

use std::process::ExitCode;

use clap::Parser;

use impresario_connector::cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match impresario_connector::cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("impresario-connector: {}", error.message());
            ExitCode::from(error.exit_code())
        }
    }
}
