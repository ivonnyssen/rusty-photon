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
    /// Path to configuration file
    #[arg(long)]
    config: PathBuf,

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

    ServiceRunner::new("calibrator-flats").run(move |shutdown| async move {
        debug!(config_path = %cli.config.display(), "loading configuration");
        let plan = calibrator_flats::config::load_config(&cli.config)?;

        calibrator_flats::ServerBuilder::new()
            .with_plan(plan)
            .with_port(cli.port)
            .with_bind_address(cli.bind_address)
            .build()
            .await?
            .start(shutdown.cancelled())
            .await?;

        Ok(())
    })
}
