use clap::{Parser, ValueEnum};
use filemonitor::{load_config, start_server};
use std::path::PathBuf;
use tracing::{debug, info};

#[derive(Debug, Clone, ValueEnum)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<LogLevel> for tracing::Level {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Error => tracing::Level::ERROR,
            LogLevel::Warn => tracing::Level::WARN,
            LogLevel::Info => tracing::Level::INFO,
            LogLevel::Debug => tracing::Level::DEBUG,
            LogLevel::Trace => tracing::Level::TRACE,
        }
    }
}

#[derive(Parser)]
#[command(name = "filemonitor")]
#[command(about = "ASCOM Alpaca SafetyMonitor that monitors file content")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.json")]
    config: PathBuf,

    /// Log level
    #[arg(short, long, default_value = "info")]
    log_level: LogLevel,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Setup tracing with specified log level
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::from(args.log_level.clone()))
        .init();

    debug!(
        "Parsed command line arguments: config={:?}, log_level={:?}",
        args.config, args.log_level
    );

    let config = load_config(&args.config)?;

    info!("Starting filemonitor server on port {}", config.server.port);

    start_server(config).await?;

    Ok(())
}
