//! QHY Q-Focuser Driver CLI
//!
//! Command-line interface for the QHY Q-Focuser ASCOM Alpaca driver.

use std::path::PathBuf;

use clap::Parser;
use tracing::{debug, info, Level};

use qhy_focuser::SerialPortFactory;
#[cfg(feature = "mock")]
use qhy_focuser::{load_config, Config, MockSerialPortFactory, ServerBuilder};
#[cfg(not(feature = "mock"))]
use qhy_focuser::{load_config, Config, ServerBuilder};

#[derive(Parser)]
#[command(name = "qhy-focuser")]
#[command(about = "ASCOM Alpaca driver for QHY Q-Focuser (EAF)")]
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
        () = ctrl_c => debug!("received Ctrl+C"),
        () = terminate => debug!("received SIGTERM"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .init();

    debug!(
        "Parsed command line arguments: config={:?}, port={:?}, server_port={:?}, log_level={:?}",
        args.config, args.port, args.server_port, args.log_level
    );

    let mut config = if let Some(config_path) = &args.config {
        debug!("Loading configuration from {:?}", config_path);
        load_config(config_path)?
    } else {
        debug!("Using default configuration");
        Config::default()
    };

    if let Some(port) = args.port {
        config.serial.port = port;
    }
    if let Some(server_port) = args.server_port {
        config.server.port = server_port;
    }

    info!("Starting QHY Q-Focuser driver");
    #[cfg(feature = "mock")]
    info!("Running in MOCK MODE - no real hardware");
    #[cfg(not(feature = "mock"))]
    info!("Serial port: {}", config.serial.port);
    info!("Baud rate: {}", config.serial.baud_rate);
    info!("Server port: {}", config.server.port);

    #[cfg(feature = "mock")]
    let factory: std::sync::Arc<dyn SerialPortFactory> =
        std::sync::Arc::new(MockSerialPortFactory::default());
    #[cfg(not(feature = "mock"))]
    let factory: std::sync::Arc<dyn SerialPortFactory> =
        std::sync::Arc::new(qhy_focuser::serial::TokioSerialPortFactory::new());

    if let Some(config_path) = &args.config {
        qhy_focuser::run_server_loop(
            config_path.as_ref(),
            factory,
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
        let bound = ServerBuilder::new()
            .with_config(config)
            .with_factory(factory)
            .build()
            .await?;
        tokio::select! {
            result = bound.start() => { result?; }
            () = shutdown_signal() => { info!("Shutting down"); }
        }
    }

    Ok(())
}
