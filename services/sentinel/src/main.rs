//! Sentinel CLI
//!
//! Command-line interface for the observatory monitoring and notification service.

use std::path::PathBuf;

use clap::Parser;
use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
use sentinel::{load_config, Config, SentinelBuilder};
use tracing::Level;

#[derive(Parser)]
#[command(name = "sentinel")]
#[command(about = "Observatory monitoring and notification service")]
#[command(version)]
struct Args {
    /// Path to configuration file. Defaults to the per-user platform config
    /// directory (e.g. `~/.config/rusty-photon/sentinel.json` on Linux);
    /// created with defaults on first start if absent.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Dashboard port (overrides config file)
    #[arg(long)]
    dashboard_port: Option<u16>,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: Level,
}

fn main() -> ServiceResult {
    let args = Args::parse();

    rusty_photon_service_lifecycle::init_tracing(args.log_level);

    tracing::debug!(
        "Parsed command line arguments: config={:?}, dashboard_port={:?}, log_level={:?}",
        args.config,
        args.dashboard_port,
        args.log_level
    );

    // Self-create defaults only for the XDG default path (packaged first
    // start); an explicit --config pointing at a missing file must stay a
    // hard error so a typo'd path never silently drops monitors/notifiers.
    let explicit = args.config.is_some();
    let config_path = rusty_photon_config::resolve_config_path("sentinel", args.config)?;
    if !explicit
        && rusty_photon_config::init_file_if_absent(
            &config_path,
            &serde_json::to_value(Config::default())?,
        )?
    {
        tracing::info!("Created default config at {}", config_path.display());
    }
    tracing::debug!("Loading configuration from {:?}", config_path);
    let mut config = load_config(&config_path)?;

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

    ServiceRunner::new("sentinel").run(move |shutdown| async move {
        SentinelBuilder::new(config)
            .with_cancellation_token(shutdown.token())
            .build()
            .await?
            .start()
            .await?;
        Ok(())
    })
}
