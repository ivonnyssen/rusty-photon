use clap::Parser;
use sky_survey_camera::run;
use std::path::PathBuf;
use tracing::{debug, Level};

#[derive(Parser)]
#[command(name = "sky-survey-camera")]
#[command(about = "ASCOM Alpaca Camera simulator backed by NASA SkyView")]
struct Args {
    #[arg(short, long, default_value = "config.json")]
    config: PathBuf,

    #[arg(short, long, default_value = "info", value_parser = clap::value_parser!(Level))]
    log_level: Level,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        tracing_subscriber::fmt()
            .with_max_level(args.log_level)
            .init();
        debug!(config = ?args.config, "starting sky-survey-camera");
        run(&args.config).await
    })?;
    Ok(())
}
