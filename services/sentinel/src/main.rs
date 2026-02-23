//! Sentinel CLI
//!
//! Command-line interface for the observatory monitoring and notification service.

use std::path::PathBuf;

use clap::Parser;
use sentinel::{load_config, Config, SentinelBuilder};
use tracing::Level;

#[derive(Parser)]
#[command(name = "sentinel")]
#[command(about = "Observatory monitoring and notification service")]
#[command(version)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Dashboard port (overrides config file)
    #[arg(long)]
    dashboard_port: Option<u16>,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: Level,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .init();

    tracing::debug!(
        "Parsed command line arguments: config={:?}, dashboard_port={:?}, log_level={:?}",
        args.config,
        args.dashboard_port,
        args.log_level
    );

    let mut config = if let Some(config_path) = &args.config {
        tracing::debug!("Loading configuration from {:?}", config_path);
        load_config(config_path)?
    } else {
        tracing::debug!("Using default configuration");
        Config::default()
    };

    config.resolve_secrets()?;

    if let Some(dashboard_port) = args.dashboard_port {
        config.dashboard.port = dashboard_port;
    }

    tracing::info!("Starting sentinel service");
    tracing::debug!(
        "Monitors: {}, Notifiers: {}, Transitions: {}",
        config.monitors.len(),
        config.notifiers.len(),
        config.transitions.len()
    );

    SentinelBuilder::new(config).build().await?.start().await?;

    Ok(())
}
