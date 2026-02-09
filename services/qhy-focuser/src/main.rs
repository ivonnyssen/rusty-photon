//! QHY Q-Focuser Driver CLI
//!
//! Command-line interface for the QHY Q-Focuser ASCOM Alpaca driver.

use std::path::PathBuf;

use clap::Parser;
use tracing::Level;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

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

    let mut config = if let Some(config_path) = &args.config {
        tracing::debug!("Loading configuration from {:?}", config_path);
        load_config(config_path)?
    } else {
        tracing::debug!("Using default configuration");
        Config::default()
    };

    if let Some(port) = args.port {
        config.serial.port = port;
    }
    if let Some(server_port) = args.server_port {
        config.server.port = server_port;
    }

    tracing::info!("Starting QHY Q-Focuser driver");
    #[cfg(feature = "mock")]
    tracing::info!("Running in MOCK MODE - no real hardware");
    #[cfg(not(feature = "mock"))]
    tracing::info!("Serial port: {}", config.serial.port);
    tracing::info!("Baud rate: {}", config.serial.baud_rate);
    tracing::info!("Server port: {}", config.server.port);

    #[cfg(feature = "mock")]
    {
        let factory = std::sync::Arc::new(MockSerialPortFactory::default());
        ServerBuilder::new(config)
            .with_factory(factory)
            .build()
            .await?
            .start()
            .await?;
    }

    #[cfg(not(feature = "mock"))]
    {
        ServerBuilder::new(config).build().await?.start().await?;
    }

    Ok(())
}
