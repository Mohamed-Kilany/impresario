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

//! impresario-core: configuration, the impresario DSL, selectors, keys and
//! diagnostics. This crate has no async or gRPC dependencies (D-16).

pub mod apps;
pub mod ast;
pub mod command;
pub mod config;
pub mod diag;
pub mod duration;
pub mod fmt;
pub mod home;
pub mod keys;
pub mod migrate;
pub mod parser;
pub mod platforms;
pub mod scanner;
pub mod selectors;
pub mod settings;
pub mod span;
pub mod strings;
pub mod support;
pub mod validation;
pub mod workspace;
