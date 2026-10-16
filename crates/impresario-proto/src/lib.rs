#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unreachable
)]

//! Generated prost/tonic bindings for the impresario wire protocol.
//!
//! This crate contains no logic: it only exposes the `impresario.v1` package
//! generated from `proto/impresario/v1/impresario.proto` by `build.rs`.

pub mod v1 {
    #![allow(clippy::all)]
    #![allow(missing_docs)]
    #![allow(rustdoc::all)]
    include!(concat!(env!("OUT_DIR"), "/impresario.v1.rs"));
}
