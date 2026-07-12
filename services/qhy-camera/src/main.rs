//! qhy-camera ASCOM Alpaca driver — CLI entry point.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use qhy_camera::{load_effective_config, CliOverrides, ServerBuilder};
use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
use tracing::{debug, Level};

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

    /// Run as a Windows service (used by the service control manager).
    /// No-op on non-Windows targets.
    #[arg(long, hide = true)]
    service: bool,

    /// Test-only: start with an *empty* simulation backend (no cameras), to
    /// exercise the zero-camera startup path (contract C0). Only meaningful when
    /// built with `--features simulation`.
    #[cfg(feature = "simulation")]
    #[arg(long, hide = true)]
    simulation_empty: bool,

    /// Subcommand; running with none starts the ASCOM Alpaca driver.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Diagnose the QHYCCD Windows installation: qhyccd.dll resolution, the
    /// loaded SDK version vs. the pinned build-time version, and All-in-One
    /// driver-pack presence (see docs/services/qhy-camera.md § "Windows:
    /// qhyccd.dll resolution"). Windows-focused; on other platforms it only
    /// prints a note.
    Doctor,
}

fn parse_log_level(s: &str) -> std::result::Result<Level, String> {
    s.parse()
        .map_err(|_| format!("invalid log level: {s}. Use: trace, debug, info, warn, error"))
}

fn main() -> ServiceResult {
    let args = Args::parse();
    // In Windows SCM service mode logs go to the rolling file under
    // %PROGRAMDATA%\rusty-photon\logs\; hold the guard until process exit so
    // the final lines flush on SCM Stop. Console mode logs to stderr as before.
    let _tracing_guard = rusty_photon_service_lifecycle::init_service_tracing(
        "qhy-camera",
        args.log_level,
        args.service,
    );

    if let Some(Command::Doctor) = args.command {
        // Interactive diagnostic — never starts the server. The exit code
        // reflects overall health (DR3).
        std::process::exit(qhy_camera::doctor::run());
    }

    // Windows real-SDK builds delay-load qhyccd.dll (see build.rs): resolve it
    // BEFORE any SDK call and keep it resident, or exit non-zero with the one
    // distinctive, actionable error (PF1–PF4). Simulation builds skip this:
    // their real FFI is cfg'd out, so nothing would ever call into qhyccd.dll
    // and it is not required at runtime (PF5).
    #[cfg(all(windows, not(feature = "simulation")))]
    qhy_camera::preflight::ensure_qhyccd_dll()?;

    let config_path = rusty_photon_config::resolve_config_path("qhy-camera", args.config)?;
    let overrides = CliOverrides {
        server_port: args.port,
    };
    #[cfg(feature = "simulation")]
    let simulation_empty = args.simulation_empty;

    // Startup chatter stays at debug! per AGENTS.md Rule 9; the user-facing
    // "Service started successfully on <addr>" info! lives in lib.rs.
    debug!("Starting QHY camera driver");
    #[cfg(feature = "simulation")]
    debug!("Using the qhyccd-rs SIMULATION backend");
    debug!("Configuration path: {}", config_path.display());
    // No `materialize_identity`: ASCOM UniqueIDs are derived from the camera/CFW
    // SDK serials at enumeration (see docs/services/qhy-camera.md), not minted.

    ServiceRunner::new("qhy-camera")
        .with_reload()
        .scm_mode(args.service)
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
