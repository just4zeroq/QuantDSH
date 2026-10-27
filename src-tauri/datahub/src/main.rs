//! Daemon entry point.
//!
//!   quantdsh-datahub [--data-dir DIR] [--port N] [--token T]
//!
//! Writes a port file (`datahub.json`) next to the databases so the desktop app
//! can find the running instance. If a healthy daemon already answers, this
//! process prints that endpoint and exits — it never opens the database twice.

use std::sync::Arc;

use clap::Parser;

use quantdsh_datahub::api::{self, AppState};
use quantdsh_datahub::daemon::{self, Acquired, DEFAULT_PORT};
use quantdsh_datahub::service::DataHub;

#[derive(Parser, Debug)]
#[command(name = "quantdsh-datahub", about = "QuantDSH market data daemon")]
struct Cli {
    /// Where `market.duckdb` / `meta.sqlite` / `datahub.json` live.
    #[arg(long)]
    data_dir: Option<std::path::PathBuf>,

    /// Preferred loopback port; scans upward when taken.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// Fixed token. Generated (and stored in the port file) when omitted.
    #[arg(long)]
    token: Option<String>,

    /// Exit after printing the endpoint instead of serving.
    #[arg(long)]
    resolve_only: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let data_dir = cli.data_dir.unwrap_or_else(daemon::default_data_dir);

    let acquired = daemon::acquire(&data_dir, cli.port, cli.token).await?;

    let endpoint = match acquired {
        Acquired::Existing(ep) => {
            // Another instance owns the databases. Reuse it rather than
            // contending for the DuckDB file.
            println!("{}", serde_json::to_string(&ep)?);
            tracing::info!(
                port = ep.port,
                "reusing the running datahub; exiting to keep a single writer"
            );
            return Ok(());
        }
        Acquired::Fresh(ep) => ep,
    };

    println!("{}", serde_json::to_string(&endpoint)?);
    if cli.resolve_only {
        daemon::release(&data_dir);
        return Ok(());
    }

    let hub = Arc::new(DataHub::open(&data_dir)?);
    let state = Arc::new(AppState {
        hub: hub.clone(),
        token: endpoint.token.clone(),
    });
    let router = api::build_router(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", endpoint.port)).await?;
    tracing::info!(
        port = endpoint.port,
        data_dir = %data_dir.display(),
        duckdb = quantdsh_datahub::DUCKDB_ENGINE,
        "quantdsh-datahub listening on 127.0.0.1"
    );

    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutdown signal received");
    };

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;

    daemon::release(&data_dir);
    Ok(())
}
