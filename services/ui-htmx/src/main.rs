//! `ui-htmx` — CLI entry point for the web configuration UI (BFF).

use std::path::PathBuf;

use clap::Parser;
use rusty_photon_service_lifecycle::{report_from_boxed, ServiceResult, ServiceRunner};
use tracing::{debug, info, Level};
use ui_htmx::{build_router, load_config, AppState, Config};

#[derive(Parser)]
#[command(name = "ui-htmx")]
#[command(about = "Server-rendered web configuration UI (BFF) for rusty-photon")]
#[command(version)]
struct Args {
    /// Path to the BFF configuration file. Defaults to the per-user platform
    /// config directory (e.g. `~/.config/rusty-photon/ui-htmx.json` on
    /// Linux); created with defaults on first start if absent (binds
    /// 127.0.0.1:11120, targets dsd-fp2 at http://127.0.0.1:11119).
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// BFF listen port (overrides server.port).
    #[arg(long)]
    port: Option<u16>,

    /// Log level
    #[arg(short, long, default_value = "info", value_parser = parse_log_level)]
    log_level: Level,
}

fn parse_log_level(s: &str) -> Result<Level, String> {
    s.parse()
        .map_err(|_| format!("Invalid log level: {s}. Use: trace, debug, info, warn, error"))
}

fn main() -> ServiceResult {
    let args = Args::parse();

    rusty_photon_service_lifecycle::init_tracing(args.log_level);

    let config_path = rusty_photon_config::resolve_and_init(
        "ui-htmx",
        args.config,
        &serde_json::to_value(Config::default())?,
    )?;
    debug!("Loading configuration from {config_path:?}");
    let mut config = load_config(&config_path).map_err(report_from_boxed)?;
    if let Some(port) = args.port {
        config.server.port = port;
    }

    info!("Starting ui-htmx configuration UI");

    ServiceRunner::new("ui-htmx").run(move |shutdown| async move {
        let state = AppState::from_config(&config)?;
        let app = build_router(state);

        let addr = format!("{}:{}", config.server.bind, config.server.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        let bound = listener.local_addr()?;
        // Print to stdout (not just `info!`) so port discovery works regardless
        // of log level/output — matching the drivers' `bound_addr=` line.
        println!("Bound ui-htmx server bound_addr={bound}");
        info!("ui-htmx listening; bound_addr={bound}");

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled())
            .await?;
        Ok(())
    })
}
