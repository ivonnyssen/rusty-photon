use clap::Parser;
use filemonitor::{run_server_loop, Config};
use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
use std::path::PathBuf;
use tracing::{debug, Level};

#[derive(Parser)]
#[command(name = "filemonitor")]
#[command(about = "ASCOM Alpaca SafetyMonitor that monitors file content")]
struct Args {
    /// Path to configuration file. Defaults to the per-user platform config
    /// directory (e.g. `~/.config/rusty-photon/filemonitor.json` on Linux);
    /// created with defaults on first start if absent.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Log level
    #[arg(short, long, default_value = "info", value_parser = clap::value_parser!(Level))]
    log_level: Level,

    /// Run as a Windows service (used by the service control manager).
    /// No-op on non-Windows targets.
    #[arg(long, hide = true)]
    service: bool,
}

fn main() -> ServiceResult {
    let args = Args::parse();

    rusty_photon_service_lifecycle::init_tracing(args.log_level);

    debug!(
        "Parsed command line arguments: config={:?}, log_level={:?}, service={}",
        args.config, args.log_level, args.service
    );

    let config_path = rusty_photon_config::resolve_and_init(
        "filemonitor",
        args.config,
        &serde_json::to_value(Config::default())?,
    )?;
    ServiceRunner::new("filemonitor")
        .with_reload()
        .scm_mode(args.service)
        .run_with_reload(move |shutdown, reload| async move {
            run_server_loop(&config_path, shutdown.token(), reload).await
        })
}
