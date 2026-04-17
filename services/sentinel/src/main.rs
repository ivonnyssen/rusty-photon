//! Sentinel CLI
//!
//! Command-line interface for the observatory monitoring and notification service.

use std::path::PathBuf;

use clap::Parser;
use sentinel::{Config, SentinelBuilder};
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

    if let Some(config_path) = &args.config {
        sentinel::run_server_loop(
            config_path.as_ref(),
            || {
                Box::pin(async {
                    let ctrl_c = async {
                        tokio::signal::ctrl_c()
                            .await
                            .expect("Failed to listen for ctrl-c");
                    };

                    #[cfg(unix)]
                    let terminate = async {
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .expect("Failed to install SIGTERM handler")
                            .recv()
                            .await;
                    };

                    #[cfg(not(unix))]
                    let terminate = std::future::pending::<()>();

                    tokio::select! {
                        () = ctrl_c => tracing::info!("Received Ctrl+C"),
                        () = terminate => tracing::info!("Received SIGTERM"),
                    }
                })
            },
            || {
                #[cfg(unix)]
                {
                    Box::pin(async {
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                            .expect("Failed to register SIGHUP handler")
                            .recv()
                            .await;
                    })
                }
                #[cfg(not(unix))]
                {
                    Box::pin(std::future::pending())
                }
            },
        )
        .await?;
    } else {
        // No config file — single run with default config (no reload support).
        let mut config = Config::default();
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
    }

    Ok(())
}
