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

//! impresario-clients: gRPC client, device transports, conversions, mock agent.
