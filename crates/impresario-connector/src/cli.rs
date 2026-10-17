//! The `impresario-connector` command line (spec 10): `serve`, `ping` and
//! `pages`.
//!
//! Exit codes (decision I-36): 0 success, 1 runtime failure, 2 usage error.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Once;

use clap::{Args, Parser, Subcommand};

use crate::backends::raw::RawCdpBackend;
use crate::backends::CdpBackend;
use crate::pages::{self, HostPort};
use crate::server::ConnectorService;

#[derive(Debug, Parser)]
#[command(
    name = "impresario-connector",
    version,
    about = "impresario connector: a gRPC agent that drives Chromium-based TVs and browsers over CDP"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Serve the impresario Agent gRPC API.
    Serve(ServeArgs),
    /// Check that a CDP endpoint answers (`ping <ws-url>`).
    Ping(PingArgs),
    /// List the DevTools page targets of an endpoint (`pages <host:port>`).
    Pages(PagesArgs),
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:50051", value_name = "HOST:PORT")]
    pub listen: String,
    /// Shared token every client must present (spec 7.2.1).
    #[arg(long, value_name = "TOKEN")]
    pub token: Option<String>,
    /// Allow binding outside the loopback interface (requires `--token` or TLS).
    #[arg(long)]
    pub allow_remote: bool,
    /// PEM certificate chain; TLS is served when given with `--tls-key`.
    #[arg(long, value_name = "FILE")]
    pub tls_cert: Option<PathBuf>,
    /// PEM private key; TLS is served when given with `--tls-cert`.
    #[arg(long, value_name = "FILE")]
    pub tls_key: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PingArgs {
    /// A page-level `ws://` DevTools endpoint.
    #[arg(value_name = "WS_URL")]
    pub ws_url: String,
}

#[derive(Debug, Args)]
pub struct PagesArgs {
    /// A DevTools endpoint (`host:port` or `ws://host:port`).
    #[arg(value_name = "HOST:PORT")]
    pub address: String,
}

/// A CLI failure: usage errors exit 2, runtime failures exit 1 (I-36).
#[derive(Debug)]
pub enum RunError {
    Usage(String),
    Runtime(String),
}

impl RunError {
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            RunError::Usage(_) => 2,
            RunError::Runtime(_) => 1,
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            RunError::Usage(message) | RunError::Runtime(message) => message,
        }
    }
}

/// Run the CLI (blocking: builds the async runtime).
pub fn run(cli: Cli) -> Result<(), RunError> {
    init_logging();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| RunError::Runtime(format!("cannot start the async runtime: {error}")))?;
    runtime.block_on(run_async(cli))
}

async fn run_async(cli: Cli) -> Result<(), RunError> {
    match cli.command {
        Command::Serve(args) => serve(args).await,
        Command::Ping(args) => ping(args).await,
        Command::Pages(args) => pages(args).await,
    }
}

fn init_logging() {
    static LOGGING: Once = Once::new();
    LOGGING.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    });
}

/// `serve`: the gRPC `Agent` API (spec 10).
async fn serve(args: ServeArgs) -> Result<(), RunError> {
    let address: SocketAddr = args.listen.parse().map_err(|_| {
        RunError::Usage(format!(
            "'--listen {}' is not a socket address (HOST:PORT)",
            args.listen
        ))
    })?;
    let tls = match (&args.tls_cert, &args.tls_key) {
        (None, None) => None,
        (Some(cert), Some(key)) => Some((cert.clone(), key.clone())),
        _ => {
            return Err(RunError::Usage(
                "--tls-cert and --tls-key must be given together".to_string(),
            ))
        }
    };
    if !address.ip().is_loopback()
        && !(args.allow_remote && (args.token.is_some() || tls.is_some()))
    {
        return Err(RunError::Usage(format!(
            "refusing to bind to {address}: binding outside the loopback interface requires \
             --allow-remote together with --token or --tls-cert/--tls-key (spec 10)"
        )));
    }

    let service = ConnectorService::new(args.token.clone());
    let manager = service.manager().clone();
    let mut builder = tonic::transport::Server::builder();
    if let Some((cert, key)) = tls {
        builder = configure_tls(builder, &cert, &key)?;
    }
    tracing::info!(%address, "impresario-connector listening");
    let shutdown = async {
        if tokio::signal::ctrl_c().await.is_err() {
            futures::future::pending::<()>().await;
        }
        tracing::info!("shutdown signal received");
    };
    builder
        .add_service(impresario_proto::v1::agent_server::AgentServer::new(
            service,
        ))
        .serve_with_shutdown(address, shutdown)
        .await
        .map_err(|error| RunError::Runtime(format!("the gRPC server failed: {error}")))?;
    manager.teardown_all().await;
    tracing::info!("impresario-connector stopped");
    Ok(())
}

#[cfg(feature = "tls")]
fn configure_tls(
    builder: tonic::transport::Server,
    cert: &PathBuf,
    key: &PathBuf,
) -> Result<tonic::transport::Server, RunError> {
    let cert_pem = std::fs::read(cert)
        .map_err(|error| RunError::Runtime(format!("cannot read {}: {error}", cert.display())))?;
    let key_pem = std::fs::read(key)
        .map_err(|error| RunError::Runtime(format!("cannot read {}: {error}", key.display())))?;
    let identity = tonic::transport::Identity::from_pem(cert_pem, key_pem);
    builder
        .tls_config(tonic::transport::ServerTlsConfig::new().identity(identity))
        .map_err(|error| RunError::Runtime(format!("invalid TLS configuration: {error}")))
}

#[cfg(not(feature = "tls"))]
fn configure_tls(
    _builder: tonic::transport::Server,
    _cert: &PathBuf,
    _key: &PathBuf,
) -> Result<tonic::transport::Server, RunError> {
    Err(RunError::Runtime(
        "TLS support is not compiled in; rebuild impresario-connector with --features tls"
            .to_string(),
    ))
}

/// `ping <ws-url>`: connect and evaluate `1+1` over CDP.
async fn ping(args: PingArgs) -> Result<(), RunError> {
    let backend = RawCdpBackend::connect(&args.ws_url)
        .await
        .map_err(|error| RunError::Runtime(error.message))?;
    let value = backend
        .evaluate("1+1")
        .await
        .map_err(|error| RunError::Runtime(error.message))?;
    backend.close().await;
    println!("ok: {value}");
    Ok(())
}

/// `pages <host:port>`: list the `/json/list` page targets.
async fn pages(args: PagesArgs) -> Result<(), RunError> {
    let endpoint = parse_endpoint(&args.address)?;
    let list = pages::list_pages(&endpoint)
        .await
        .map_err(|error| RunError::Runtime(error.message))?;
    if list.is_empty() {
        println!("no page targets at {}", endpoint.authority());
        return Ok(());
    }
    for page in list {
        println!("{}\t{}\t{}\t{}", page.id, page.title, page.url, page.ws_url);
    }
    Ok(())
}

fn parse_endpoint(address: &str) -> Result<HostPort, RunError> {
    if address.contains("://") {
        let url = url::Url::parse(address)
            .map_err(|error| RunError::Usage(format!("'{address}' is not a URL: {error}")))?;
        let host = url
            .host_str()
            .ok_or_else(|| RunError::Usage(format!("'{address}' has no host")))?;
        let port = url
            .port()
            .ok_or_else(|| RunError::Usage(format!("'{address}' needs an explicit port")))?;
        Ok(HostPort {
            host: host.to_string(),
            port,
        })
    } else {
        HostPort::parse(address).map_err(|error| RunError::Usage(error.message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_accepts_host_port_and_urls() {
        let endpoint = parse_endpoint("192.168.1.50:9998").expect("host:port");
        assert_eq!(endpoint.authority(), "192.168.1.50:9998");
        let endpoint = parse_endpoint("ws://127.0.0.1:9222/devtools/page/1").expect("url");
        assert_eq!(endpoint.authority(), "127.0.0.1:9222");
        assert!(parse_endpoint("nohost").is_err());
        assert!(parse_endpoint("ws://127.0.0.1/devtools/page/1").is_err());
    }

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(RunError::Usage(String::new()).exit_code(), 2);
        assert_eq!(RunError::Runtime(String::new()).exit_code(), 1);
        assert_eq!(RunError::Usage("hi".to_string()).message(), "hi");
        assert_eq!(RunError::Runtime("ho".to_string()).message(), "ho");
    }

    #[test]
    fn cli_parses_the_documented_surface() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["impresario-connector", "serve"]).expect("serve");
        match cli.command {
            Command::Serve(args) => {
                assert_eq!(args.listen, "127.0.0.1:50051");
                assert!(args.token.is_none());
                assert!(!args.allow_remote);
            }
            other => panic!("unexpected command: {other:?}"),
        }
        let cli = Cli::try_parse_from([
            "impresario-connector",
            "serve",
            "--listen",
            "0.0.0.0:50051",
            "--allow-remote",
            "--token",
            "s3cret",
        ])
        .expect("remote serve");
        match cli.command {
            Command::Serve(args) => {
                assert!(args.allow_remote);
                assert_eq!(args.token.as_deref(), Some("s3cret"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
        let cli = Cli::try_parse_from(["impresario-connector", "ping", "ws://host:9222/x"])
            .expect("ping");
        match cli.command {
            Command::Ping(args) => assert_eq!(args.ws_url, "ws://host:9222/x"),
            other => panic!("unexpected command: {other:?}"),
        }
        let cli =
            Cli::try_parse_from(["impresario-connector", "pages", "host:9222"]).expect("pages");
        match cli.command {
            Command::Pages(args) => assert_eq!(args.address, "host:9222"),
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
