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

//! impresario-connector: gRPC server plus CDP backends for Chromium-based TVs
//! and browsers (spec section 10).
//!
//! Module map:
//! - [`server`]: the tonic `Agent` service (handshake, session, out-of-band
//!   screenshot, logs, shutdown) and the [`server::SessionManager`].
//! - [`session`]: one CDP session per handshake; owns the backend, the browser
//!   process (for `launch` endpoints) and the ordered command loop.
//! - [`exec`]: the platform semantics of section 5.6 / 6.1-6.4 on top of a
//!   [`backends::CdpBackend`].
//! - [`backends`]: [`backends::raw::RawCdpBackend`] (default) and the optional
//!   `chromiumoxide` backend.
//! - [`script`]: the embedded ES5 helper script (`window.__impresario`).
//! - [`pages`]: DevTools page discovery (`/json/list`) and endpoint resolution.
//! - [`keys`]: the web key table (spec 6.2) used for CDP and JS dispatch.

pub mod backends;
pub mod cli;
pub mod errors;
pub mod exec;
pub mod focus;
pub mod keys;
pub mod launch;
pub mod pages;
pub mod png;
pub mod script;
pub mod server;
pub mod session;

pub use errors::ConnectorError;
