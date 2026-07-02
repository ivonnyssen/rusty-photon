use clap::Parser;
use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
use sky_survey_camera::run_reloadable;
use std::path::PathBuf;
use tracing::{debug, Level};

#[derive(Parser)]
#[command(name = "sky-survey-camera")]
#[command(about = "ASCOM Alpaca Camera simulator backed by NASA SkyView")]
struct Args {
    /// Path to the JSON config file. When omitted, resolves to the
    /// per-user XDG config path (`~/.config/rusty-photon/sky-survey-camera.json`
    /// on Linux) via `rusty_photon_config::resolve_config_path`. Pass
    /// `--config config.json` to keep the previous CWD-relative default.
    #[arg(short, long)]
    config: Option<PathBuf>,

    #[arg(short, long, default_value = "info", value_parser = clap::value_parser!(Level))]
    log_level: Level,
}

fn main() -> ServiceResult {
    let args = Args::parse();

    rusty_photon_service_lifecycle::init_tracing(args.log_level);

    let config_path = rusty_photon_config::resolve_config_path("sky-survey-camera", args.config)?;
    debug!(config = ?config_path, "starting sky-survey-camera");

    ServiceRunner::new("sky-survey-camera")
        .with_reload()
        .run_with_reload(move |shutdown, reload| async move {
            run_reloadable(&config_path, shutdown, reload)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
        })
}
