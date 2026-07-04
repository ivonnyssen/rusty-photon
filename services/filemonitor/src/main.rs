use clap::Parser;
use filemonitor::{run_server_loop, Config};
use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
use std::path::PathBuf;
use tracing::{debug, info, Level};

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

    // Self-create defaults only for the XDG default path (packaged first
    // start); an explicit --config pointing at a missing file must stay a
    // hard error so a typo'd path never silently runs a default safety
    // monitor.
    let explicit = args.config.is_some();
    let config_path = rusty_photon_config::resolve_config_path("filemonitor", args.config)?;
    if !explicit
        && rusty_photon_config::init_file_if_absent(
            &config_path,
            &serde_json::to_value(Config::default())?,
        )?
    {
        info!("Created default config at {}", config_path.display());
    }
    ServiceRunner::new("filemonitor")
        .with_reload()
        .scm_mode(args.service)
        .run_with_reload(move |shutdown, reload| async move {
            run_server_loop(&config_path, shutdown.token(), reload).await
        })
}
