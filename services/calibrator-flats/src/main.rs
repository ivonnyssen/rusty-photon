use std::path::PathBuf;

use clap::Parser;
use tracing::debug;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "calibrator-flats",
    about = "Calibrator flat field orchestrator - iterative exposure optimization"
)]
struct Cli {
    /// Path to configuration file
    #[arg(long)]
    config: PathBuf,

    /// Port to listen on
    #[arg(long, default_value = "11170")]
    port: u16,

    /// Bind address
    #[arg(long, default_value = "127.0.0.1")]
    bind_address: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cli.log_level)),
        )
        .init();

    debug!(config_path = %cli.config.display(), "loading configuration");
    let plan = calibrator_flats::config::load_config(&cli.config)?;

    calibrator_flats::ServerBuilder::new()
        .with_plan(plan)
        .with_port(cli.port)
        .with_bind_address(cli.bind_address)
        .build()
        .await?
        .start()
        .await?;

    Ok(())
}
