//! Development helper: restart a paired Apple TV through the pure-Rust RSD
//! diagnostics relay. Usage: `restart_tv <host> <IMPRESARIO_HOME> [udid]`.
//!
//! This clears a wedged XCUITest/testmanagerd session between hardware runs.
//! It is an example, not a shipped CLI command.

use std::path::Path;

use impresario_apple_device::pairing::ApplePaths;
use impresario_apple_device::tunnel::AppleTunnel;

fn main() {
    // RSD plist parsing needs the same large worker stack as the CLI.
    let result = std::thread::Builder::new()
        .name("restart-tv".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            runtime.block_on(run()).map_err(|error| error.to_string())
        })
        .expect("spawn restart worker")
        .join()
        .expect("join restart worker");
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let host = args
        .next()
        .ok_or("usage: restart_tv <host> <home> [udid]")?;
    let home = args
        .next()
        .ok_or("usage: restart_tv <host> <home> [udid]")?;
    let udid = args.next();

    let paths = ApplePaths::new(Path::new(&home));
    let pairing = paths.load_pairing_for(udid.as_deref(), &host)?;
    let mut tunnel = AppleTunnel::connect(&host, &pairing).await?;
    println!("Restarting {} ({}).", tunnel.summary(), tunnel.host);
    tunnel.restart().await?;
    println!("Restart requested.");
    Ok(())
}
