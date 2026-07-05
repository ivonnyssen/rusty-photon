use std::path::PathBuf;

use clap::Parser;
use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
use tracing::{debug, Level};

#[derive(Parser)]
#[command(
    name = "session-runner",
    about = "Generic imaging-workflow orchestrator - executes declarative JSON workflow \
             documents against rp's tool catalog"
)]
struct Cli {
    /// Path to the configuration file. Defaults to the per-user platform
    /// config directory (e.g. `~/.config/rusty-photon/session-runner.json`
    /// on Linux). There are no usable built-in defaults for
    /// `workflows_dir` / `state_dir`: the file must exist.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Port to listen on (overrides the config file's `port`)
    #[arg(long)]
    port: Option<u16>,

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

    let config_path = rusty_photon_config::resolve_config_path("session-runner", cli.config)?;
    let (port, bind_address) = (cli.port, cli.bind_address);

    ServiceRunner::new("session-runner").run(move |shutdown| async move {
        debug!(config_path = %config_path.display(), "loading configuration");
        let mut config = session_runner::config::load_config(&config_path)?;
        if let Some(port) = port {
            config.port = port;
        }

        session_runner::ServerBuilder::new()
            .with_config(config)
            .with_bind_address(bind_address)
            .build()
            .await?
            .start(shutdown.cancelled())
            .await?;

        Ok(())
    })
}
