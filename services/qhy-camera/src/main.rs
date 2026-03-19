//! QHY Camera Driver CLI
//!
//! Command-line interface for the QHY Camera ASCOM Alpaca driver.

use std::path::PathBuf;

use clap::Parser;
use tracing::Level;

use qhy_camera::{load_config, Config};
#[cfg(feature = "mock")]
use qhy_camera::{MockSdkProvider, ServerBuilder};

#[derive(Parser)]
#[command(name = "qhy-camera")]
#[command(about = "ASCOM Alpaca driver for QHYCCD cameras and filter wheels")]
#[command(version)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

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
        "Parsed command line arguments: config={:?}, server_port={:?}, log_level={:?}",
        args.config,
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

    if let Some(server_port) = args.server_port {
        config.server.port = server_port;
    }

    tracing::info!("Starting QHY Camera driver");
    #[cfg(feature = "mock")]
    tracing::info!("Running in MOCK MODE - no real hardware");
    tracing::info!("Server port: {}", config.server.port);

    #[cfg(feature = "mock")]
    {
        let provider = Box::new(MockSdkProvider);
        // If no cameras configured, add a default mock camera and filter wheel
        if config.cameras.is_empty() {
            config.cameras.push(qhy_camera::CameraConfig {
                unique_id: "QHY600M-mock001".to_string(),
                name: "QHY600M Mock Camera".to_string(),
                description: "Mock QHYCCD camera for testing".to_string(),
                device_number: 0,
                enabled: true,
            });
        }
        if config.filter_wheels.is_empty() {
            config.filter_wheels.push(qhy_camera::FilterWheelConfig {
                unique_id: "CFW=QHY600M-mock001".to_string(),
                name: "Mock Filter Wheel".to_string(),
                description: "Mock QHYCCD filter wheel for testing".to_string(),
                device_number: 0,
                enabled: true,
                filter_names: vec![],
            });
        }
        ServerBuilder::new(config, provider)
            .build()
            .await?
            .start()
            .await?;
    }

    #[cfg(not(feature = "mock"))]
    {
        // Real SDK provider would be constructed here
        // For now, this requires the actual qhyccd-rs SDK
        let _ = config;
        panic!("Real SDK provider not yet implemented. Use --features mock for testing.");
    }

    #[allow(unreachable_code)]
    Ok(())
}
