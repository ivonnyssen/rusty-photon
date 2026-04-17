//! PPBA Switch Driver CLI
//!
//! Command-line interface for the Pegasus Astro Pocket Powerbox Advance Gen2 Switch driver.

use std::path::PathBuf;

use clap::Parser;
use tracing::Level;

use ppba_driver::SerialPortFactory;
#[cfg(feature = "mock")]
use ppba_driver::{Config, MockSerialPortFactory, ServerBuilder};
#[cfg(not(feature = "mock"))]
use ppba_driver::{Config, ServerBuilder};

#[derive(Parser)]
#[command(name = "ppba-driver")]
#[command(about = "ASCOM Alpaca driver for Pegasus Astro PPBA Gen2")]
#[command(version)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Serial port path (overrides config file)
    #[arg(long)]
    port: Option<String>,

    /// Server port (overrides config file)
    #[arg(long)]
    server_port: Option<u16>,

    /// Enable/disable Switch device
    #[arg(long)]
    enable_switch: Option<bool>,

    /// Enable/disable ObservingConditions device
    #[arg(long)]
    enable_observingconditions: Option<bool>,

    /// Log level
    #[arg(short, long, default_value = "info", value_parser = parse_log_level)]
    log_level: Level,
}

fn parse_log_level(s: &str) -> Result<Level, String> {
    s.parse().map_err(|_| {
        format!(
            "Invalid log level: {}. Use: trace, debug, info, warn, error",
            s
        )
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Setup tracing
    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .init();

    tracing::debug!(
        "Parsed command line arguments: config={:?}, port={:?}, server_port={:?}, log_level={:?}",
        args.config,
        args.port,
        args.server_port,
        args.log_level
    );

    tracing::info!("Starting PPBA driver");
    #[cfg(feature = "mock")]
    tracing::info!("Running in MOCK MODE - no real hardware");

    #[cfg(feature = "mock")]
    let factory: std::sync::Arc<dyn SerialPortFactory> =
        std::sync::Arc::new(MockSerialPortFactory::default());
    #[cfg(not(feature = "mock"))]
    let factory: std::sync::Arc<dyn SerialPortFactory> =
        std::sync::Arc::new(ppba_driver::serial::TokioSerialPortFactory::new());

    if let Some(config_path) = args.config.clone() {
        tracing::debug!("Loading configuration from {:?}", config_path);
        // Capture CLI overrides so they are re-applied after each reload.
        let override_serial_port = args.port.clone();
        let override_server_port = args.server_port;
        let override_enable_switch = args.enable_switch;
        let override_enable_obs = args.enable_observingconditions;
        ppba_driver::run_server_loop(
            config_path.as_ref(),
            factory,
            move |cfg: &mut Config| {
                if let Some(p) = &override_serial_port {
                    cfg.serial.port = p.clone();
                }
                if let Some(p) = override_server_port {
                    cfg.server.port = p;
                }
                if let Some(v) = override_enable_switch {
                    cfg.switch.enabled = v;
                }
                if let Some(v) = override_enable_obs {
                    cfg.observingconditions.enabled = v;
                }
            },
            || {
                Box::pin(async {
                    shutdown_signal().await;
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
        // No config file — single run with the CLI-assembled config (no reload support).
        tracing::debug!("Using default configuration");
        let mut config = Config::default();
        if let Some(port) = args.port {
            config.serial.port = port;
        }
        if let Some(server_port) = args.server_port {
            config.server.port = server_port;
        }
        if let Some(enable) = args.enable_switch {
            config.switch.enabled = enable;
        }
        if let Some(enable) = args.enable_observingconditions {
            config.observingconditions.enabled = enable;
        }

        #[cfg(not(feature = "mock"))]
        tracing::info!("Serial port: {}", config.serial.port);
        tracing::info!("Baud rate: {}", config.serial.baud_rate);
        tracing::info!("Server port: {}", config.server.port);

        let bound = ServerBuilder::new(config)
            .with_factory(factory)
            .build()
            .await?;
        tokio::select! {
            result = bound.start() => { result?; },
            () = shutdown_signal() => {
                tracing::debug!("shutting down");
            }
        }
    }

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::debug!("received Ctrl+C"),
        () = terminate => tracing::debug!("received SIGTERM"),
    }
}
