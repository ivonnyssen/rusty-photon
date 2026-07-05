use std::path::PathBuf;

use clap::Parser;
use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
use tracing::{debug, Level};

#[derive(Parser)]
#[command(
    name = "calibrator-flats",
    about = "Calibrator flat field orchestrator - iterative exposure optimization"
)]
struct Cli {
    /// Path to the flat-plan configuration file. Defaults to the per-user
    /// platform config directory (e.g.
    /// `~/.config/rusty-photon/calibrator-flats.json` on Linux). There is
    /// no built-in default plan: the file must exist.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Port to listen on
    #[arg(long, default_value = "11170")]
    port: u16,

    /// Bind address
    #[arg(long, default_value = "127.0.0.1")]
    bind_address: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info", value_parser = clap::value_parser!(Level))]
    log_level: Level,
}

fn main() -> ServiceResult {
    let cli = Cli::parse();

    rusty_photon_service_lifecycle::init_tracing(cli.log_level);

    let config_path = rusty_photon_config::resolve_config_path("calibrator-flats", cli.config)?;
    let (port, bind_address) = (cli.port, cli.bind_address);

    ServiceRunner::new("calibrator-flats").run(move |shutdown| async move {
        debug!(config_path = %config_path.display(), "loading configuration");
        let plan = calibrator_flats::config::load_config(&config_path)?;

        calibrator_flats::ServerBuilder::new()
            .with_plan(plan)
            .with_port(port)
            .with_bind_address(bind_address)
            .build()
            .await?
            .start(shutdown.cancelled())
            .await?;

        Ok(())
    })
}
