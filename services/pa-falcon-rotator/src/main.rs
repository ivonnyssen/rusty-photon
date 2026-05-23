#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Pegasus Falcon Rotator Driver CLI

use std::path::PathBuf;

use clap::Parser;
use rusty_photon_service_lifecycle::ServiceRunner;
use tracing::{debug, info, Level};

#[cfg(feature = "mock")]
use pa_falcon_rotator::{load_config, Config, MockFalconTransportFactory, ServerBuilder};
#[cfg(not(feature = "mock"))]
use pa_falcon_rotator::{load_config, Config, ServerBuilder};

#[derive(Parser)]
#[command(name = "pa-falcon-rotator")]
#[command(about = "ASCOM Alpaca driver for Pegasus Astro Falcon Rotator (firmware >= 1.3)")]
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

#[cfg_attr(coverage_nightly, coverage(off))]
fn parse_log_level(s: &str) -> Result<Level, String> {
    s.parse().map_err(|_| {
        format!(
            "Invalid log level: {}. Use: trace, debug, info, warn, error",
            s
        )
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    info!("Starting pa-falcon-rotator driver");
    #[cfg(feature = "mock")]
    info!("Running in MOCK MODE - no real hardware");
    #[cfg(not(feature = "mock"))]
    info!("Serial port: {}", config.serial.port);
    info!("Baud rate: {}", config.serial.baud_rate);
    info!("Server port: {}", config.server.port);

    ServiceRunner::new("pa-falcon-rotator").run(move |shutdown| async move {
        #[cfg(feature = "mock")]
        let bound = {
            let factory = std::sync::Arc::new(MockFalconTransportFactory::default());
            ServerBuilder::new()
                .with_config(config)
                .with_factory(factory)
                .build()
                .await?
        };

        #[cfg(not(feature = "mock"))]
        let bound = ServerBuilder::new().with_config(config).build().await?;

        bound.start(shutdown.cancelled()).await?;
        Ok(())
    })
}
