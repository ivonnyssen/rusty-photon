use clap::Parser;
use tracing::debug;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "rp", about = "Rusty Photon - equipment gateway and event bus")]
struct Args {
    /// Path to configuration file
    #[arg(long)]
    config: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level)),
        )
        .init();

    debug!(config_path = %args.config, "loading configuration");
    let config = rp::config::load_config(&args.config)?;

    rp::start(config).await?;

    Ok(())
}
