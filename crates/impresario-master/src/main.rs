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
    // Plist and RSD handshakes can recurse deeply; debug builds need more than
    // the Windows/Linux default main-thread stack. Keep the process entry point
    // on a roomy thread instead of failing with a bare stack overflow.
    let code = std::thread::Builder::new()
        .name("impresario".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(impresario_master::cli::run)
        .and_then(|handle| {
            handle
                .join()
                .map_err(|_| std::io::Error::other("impresario worker panicked"))
        });
    match code {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("impresario: {error}");
            std::process::exit(70);
        }
    }
}
