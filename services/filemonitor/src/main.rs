use clap::Parser;
use filemonitor::run_server_loop;
use std::path::PathBuf;
use tracing::{debug, Level};

#[cfg(windows)]
mod service;

#[derive(Parser)]
#[command(name = "filemonitor")]
#[command(about = "ASCOM Alpaca SafetyMonitor that monitors file content")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.json")]
    config: PathBuf,

    /// Log level
    #[arg(short, long, default_value = "info", value_parser = clap::value_parser!(Level))]
    log_level: Level,

    /// Run as a Windows service (used by the service control manager)
    #[cfg(windows)]
    #[arg(long, hide = true)]
    service: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    #[cfg(windows)]
    if args.service {
        return service::run(args.config, args.log_level);
    }

    debug!(
        "Parsed command line arguments: config={:?}, log_level={:?}",
        args.config, args.log_level
    );

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        tracing_subscriber::fmt()
            .with_max_level(args.log_level)
            .init();

        let config_path = args.config;
        run_with_reload(&config_path).await
    })
}

async fn run_with_reload(config_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    run_server_loop(
        config_path,
        || {
            Box::pin(async {
                let _ = tokio::signal::ctrl_c().await;
            })
        },
        || {
            // cfg(unix) covers both Linux and macOS — reload via SIGHUP
            // (e.g. `systemctl reload filemonitor` or `kill -HUP <pid>`)
            #[cfg(unix)]
            {
                Box::pin(async {
                    let mut sig =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                            .expect("Failed to register SIGHUP handler");
                    sig.recv().await;
                })
            }
            // Windows console mode has no reload signal — only Ctrl+C for shutdown.
            // In service mode, reload is handled via SCM ParamChange in service.rs.
            #[cfg(not(unix))]
            {
                Box::pin(std::future::pending())
            }
        },
    )
    .await
}
