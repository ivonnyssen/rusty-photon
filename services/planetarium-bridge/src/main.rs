//! planetarium-bridge — a virtual ASCOM Alpaca Telescope that turns
//! planetarium Align gestures into paused rusty-photon targets
//! (docs/services/planetarium-bridge.md).
//!
//! Phase-2 scaffold (docs/skills/development-workflow.md): the crate, its BDD
//! suite, and the service lifecycle land ahead of the device. The binary
//! starts, logs, and waits for shutdown; the virtual Telescope, import
//! pipeline, spool, and `/health` are specified by the `@wip` feature files
//! in `tests/features/` and arrive in the implementation phase.

use std::path::PathBuf;

use clap::Parser;
use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
use tracing::{debug, Level};

#[derive(Parser)]
#[command(name = "planetarium-bridge")]
#[command(
    about = "Virtual ASCOM Alpaca Telescope: planetarium Align gestures become paused rusty-photon targets"
)]
#[command(version)]
struct Args {
    /// Path to the JSON config file. When omitted, resolves to the platform
    /// config path (e.g. `~/.config/rusty-photon/planetarium-bridge.json` on
    /// Linux).
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Log level: trace, debug, info, warn, error.
    #[arg(short, long, default_value = "info", value_parser = parse_log_level)]
    log_level: Level,

    /// Run as a Windows service (used by the service control manager).
    /// No-op on non-Windows targets.
    #[arg(long, hide = true)]
    service: bool,
}

fn parse_log_level(s: &str) -> Result<Level, String> {
    s.parse()
        .map_err(|_| format!("invalid log level: {s} (use trace, debug, info, warn, error)"))
}

fn main() -> ServiceResult {
    let args = Args::parse();

    // In Windows SCM service mode logs go to the rolling file under
    // %PROGRAMDATA%\rusty-photon\logs\; hold the guard until process exit so
    // the final lines flush on SCM Stop. Console mode logs to stderr.
    let _tracing_guard = rusty_photon_service_lifecycle::init_service_tracing(
        "planetarium-bridge",
        args.log_level,
        args.service,
    );

    debug!(config = ?args.config, "starting planetarium-bridge (phase-2 scaffold)");

    ServiceRunner::new("planetarium-bridge")
        .scm_mode(args.service)
        .run(move |shutdown| async move {
            // Scaffold only: no Alpaca listener yet. The virtual Telescope,
            // import pipeline, and spool are the implementation phase.
            shutdown.cancelled().await;
            Ok(())
        })
}
