//! Embedded language server (`impresario lsp`, spec section 13).
//!
//! One server per workspace over stdio: `tower-lsp-server` on a tokio
//! runtime, logging to stderr only. The server keeps a workspace index built
//! with `impresario-core`, gives open editor buffers precedence over disk,
//! negotiates the position encoding, and watches the scenes/plays
//! directories and both config files. See `docs/LSP.md`.

mod analyze;
mod backend;
mod complete;
mod index;
mod nav;
mod position;
mod watch;

pub use backend::{Backend, Ctx};

use std::path::PathBuf;

use tower_lsp_server::{LspService, Server};

/// Serve the language server on stdio until the client closes the stream.
pub async fn run_stdio(base: PathBuf, config_path: PathBuf, devices_path: PathBuf) {
    let ctx = Ctx {
        base,
        config_path,
        devices_path,
    };
    let (service, socket) = LspService::new(|client| Backend::new(client, ctx.clone()));
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;
}

/// `.scene` files.
pub(crate) fn is_scene_file(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "scene")
}

/// `.play` files.
pub(crate) fn is_play_file(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "play")
}
