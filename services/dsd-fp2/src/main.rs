//! Deep Sky Dad FP2 ASCOM Alpaca driver — CLI entry point.

use std::path::PathBuf;

use ascom_alpaca::api::Device;
use clap::Parser;
use dsd_fp2::{load_effective_config, resolve_config_path, CliOverrides, ServerBuilder};
use rusty_photon_service_lifecycle::ServiceRunner;
use tracing::{debug, info, Level};

#[cfg(feature = "mock")]
use dsd_fp2::MockTransportFactory;
#[cfg(feature = "mock")]
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "dsd-fp2")]
#[command(about = "ASCOM Alpaca CoverCalibrator driver for the Deep Sky Dad FP2")]
#[command(version)]
struct Args {
    /// Path to configuration file. Defaults to the XDG path
    /// `~/.config/rusty-photon/dsd-fp2.json` (read if present, created by config.apply).
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .init();

    debug!(
        "Parsed command line arguments: config={:?}, port={:?}, server_port={:?}, log_level={:?}",
        args.config, args.port, args.server_port, args.log_level
    );

    // A config path is always resolvable (explicit --config or the XDG default),
    // so config editing is never disabled for lack of one.
    let config_path = resolve_config_path(args.config)?;
    let overrides = CliOverrides {
        serial_port: args.port,
        server_port: args.server_port,
    };

    info!("Starting Deep Sky Dad FP2 driver");
    #[cfg(feature = "mock")]
    info!("Running in MOCK MODE - no real hardware");
    info!("Configuration path: {}", config_path.display());

    // `config.apply` triggers an in-process reload rather than a process bounce:
    // each loop iteration re-reads the effective config and rebuilds the server.
    ServiceRunner::new("dsd-fp2").with_reload().run_with_reload(
        move |shutdown, reload| async move {
            loop {
                let config = load_effective_config(&config_path, &overrides)?;
                debug!(
                    "Loaded effective configuration: serial.port={}, server.port={}",
                    config.serial.port, config.server.port
                );

                #[cfg(feature = "mock")]
                let bound = {
                    let factory = Arc::new(MockTransportFactory::default());
                    ServerBuilder::new()
                        .with_config(config)
                        .with_factory(factory)
                        .with_config_source(config_path.clone(), overrides.clone())
                        .with_reload_signal(reload.clone())
                        .build()
                        .await?
                };

                #[cfg(not(feature = "mock"))]
                let bound = ServerBuilder::new()
                    .with_config(config)
                    .with_config_source(config_path.clone(), overrides.clone())
                    .with_reload_signal(reload.clone())
                    .build()
                    .await?;

                let device = bound.device();
                tokio::select! {
                    result = bound.start(shutdown.cancelled()) => return result,
                    () = reload.recv() => {
                        debug!("Reload signalled; tearing down and rebuilding dsd-fp2");
                        if let Some(device) = device {
                            // Inline, awaited transport close so the serial port is
                            // released before a client reconnects to the rebuilt server.
                            let _ = device.set_connected(false).await;
                        }
                        continue;
                    }
                }
            }
        },
    )
}
