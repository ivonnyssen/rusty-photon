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

    /// Run as a Windows service (used by the service control manager).
    /// No-op on non-Windows targets.
    #[arg(long, hide = true)]
    service: bool,
}

fn main() -> ServiceResult {
    let args = Args::parse();

    // In Windows SCM service mode logs go to the rolling file under
    // %PROGRAMDATA%\rusty-photon\logs\; hold the guard until process exit so
    // the final lines flush on SCM Stop. Console mode logs to stderr as before.
    let _tracing_guard = rusty_photon_service_lifecycle::init_service_tracing(
        "sentinel",
        args.log_level,
        args.service,
    );

    tracing::debug!(
        "Parsed command line arguments: config={:?}, dashboard_port={:?}, log_level={:?}",
        args.config,
        args.dashboard_port,
        args.log_level
    );

    let config_path = rusty_photon_config::resolve_and_init(
        "sentinel",
        args.config,
        &serde_json::to_value(Config::default())?,
    )?;
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

    ServiceRunner::new("sentinel")
        .scm_mode(args.service)
        .run(move |shutdown| async move {
            SentinelBuilder::new(config)
                .with_cancellation_token(shutdown.token())
                .build()
                .await?
                .start()
                .await?;
            Ok(())
        })
}
