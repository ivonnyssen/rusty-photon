//! qhy-camera ASCOM Alpaca driver — CLI entry point.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use qhy_camera::{load_effective_config, CliOverrides, ServerBuilder};
use rusty_photon_service_lifecycle::ServiceRunner;
use tracing::{debug, info, Level};

#[derive(Parser)]
#[command(name = "qhy-camera")]
#[command(about = "ASCOM Alpaca Camera (+ FilterWheel) driver for QHYCCD hardware")]
#[command(version)]
struct Args {
    /// Path to configuration file. Defaults to the per-user platform config
    /// directory (e.g. `~/.config/rusty-photon/qhy-camera.json` on Linux).
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Server port (overrides `server.port` in the config file).
    #[arg(long)]
    port: Option<u16>,

    /// Log level (trace, debug, info, warn, error).
    #[arg(short, long, default_value = "info", value_parser = parse_log_level)]
    log_level: Level,

    /// Test-only: start with an *empty* simulation backend (no cameras), to
    /// exercise the zero-camera startup path (contract C0). Only meaningful when
    /// built with `--features simulation`.
    #[cfg(feature = "simulation")]
    #[arg(long, hide = true)]
    simulation_empty: bool,
}

fn parse_log_level(s: &str) -> std::result::Result<Level, String> {
    s.parse()
        .map_err(|_| format!("invalid log level: {s}. Use: trace, debug, info, warn, error"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    rusty_photon_service_lifecycle::init_tracing(args.log_level);

    let config_path = rusty_photon_config::resolve_config_path("qhy-camera", args.config)?;
    let overrides = CliOverrides {
        server_port: args.port,
    };
    #[cfg(feature = "simulation")]
    let simulation_empty = args.simulation_empty;

    info!("Starting QHY camera driver");
    #[cfg(feature = "simulation")]
    info!("Using the qhyccd-rs SIMULATION backend");
    info!("Configuration path: {}", config_path.display());
    // No `materialize_identity`: ASCOM UniqueIDs are derived from the camera/CFW
    // SDK serials at enumeration (see docs/services/qhy-camera.md), not minted.

    ServiceRunner::new("qhy-camera")
        .with_reload()
        .run_with_reload(move |shutdown, reload| async move {
            loop {
                let config = load_effective_config(&config_path, &overrides)?;
                debug!(
                    "Loaded effective configuration: server.port={}",
                    config.server.port
                );

                let builder = ServerBuilder::new()
                    .with_config(config)
                    .with_config_source(config_path.clone(), overrides.clone())
                    .with_reload_signal(reload.clone());

                #[cfg(feature = "simulation")]
                let builder = if simulation_empty {
                    builder.with_sdk(qhyccd_rs::Sdk::new_simulated())
                } else {
                    builder
                };

                let bound = builder.build().await?;

                let reloaded = Arc::new(AtomicBool::new(false));
                let stop = {
                    let reloaded = Arc::clone(&reloaded);
                    let shutdown = shutdown.cancelled();
                    let reload = reload.clone();
                    async move {
                        tokio::select! {
                            () = shutdown => {}
                            () = reload.recv() => reloaded.store(true, Ordering::SeqCst),
                        }
                    }
                };
                bound.start(stop).await?;

                if reloaded.load(Ordering::SeqCst) {
                    debug!("Reload signalled; rebuilding qhy-camera from the new configuration");
                    continue;
                }
                return Ok(());
            }
        })
}
