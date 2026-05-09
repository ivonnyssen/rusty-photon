//! PPBA Switch Driver CLI
//!
//! Command-line interface for the Pegasus Astro Pocket Powerbox Advance Gen2 Switch driver.

use std::cell::OnceCell;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{CommandFactory, FromArgMatches, Parser};
use i18n_embed::fluent::FluentLanguageLoader;
use rp_i18n::{fl, fluent_language_loader};
use rust_embed::RustEmbed;
use tracing::Level;

#[cfg(feature = "mock")]
use ppba_driver::{load_config, Config, MockSerialPortFactory, ServerBuilder};
#[cfg(not(feature = "mock"))]
use ppba_driver::{load_config, Config, ServerBuilder};

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

thread_local! {
    static LOADER: OnceCell<Arc<FluentLanguageLoader>> = const { OnceCell::new() };
}

#[derive(Parser)]
#[command(name = "ppba-driver")]
#[command(version)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Serial port path (overrides config file)
    #[arg(long)]
    port: Option<String>,

    /// Server port (overrides config file)
    #[arg(long)]
    server_port: Option<u16>,

    /// Enable/disable Switch device
    #[arg(long)]
    enable_switch: Option<bool>,

    /// Enable/disable ObservingConditions device
    #[arg(long)]
    enable_observingconditions: Option<bool>,

    /// Log level
    #[arg(short, long, default_value = "info", value_parser = parse_log_level)]
    log_level: Level,
}

fn parse_log_level(s: &str) -> Result<Level, String> {
    s.parse().map_err(|_| {
        LOADER.with(|cell| match cell.get() {
            Some(loader) => fl!(loader, "error-invalid-log-level", value = s),
            None => format!(
                "Invalid log level: {}. Use: trace, debug, info, warn, error",
                s
            ),
        })
    })
}

fn build_loader() -> Arc<FluentLanguageLoader> {
    let loader = fluent_language_loader!();
    let requested = rp_i18n::resolve_locale();
    rp_i18n::select_best(&loader, &Localizations, &requested);
    Arc::new(loader)
}

fn parse_args() -> Result<Args, clap::Error> {
    let loader = build_loader();
    LOADER.with(|cell| {
        let _ = cell.set(loader.clone());
    });

    let cmd = Args::command()
        .about(fl!(loader, "cli-about"))
        .mut_arg("config", |a| a.help(fl!(loader, "cli-help-config")))
        .mut_arg("port", |a| a.help(fl!(loader, "cli-help-port")))
        .mut_arg("server_port", |a| {
            a.help(fl!(loader, "cli-help-server-port"))
        })
        .mut_arg("enable_switch", |a| {
            a.help(fl!(loader, "cli-help-enable-switch"))
        })
        .mut_arg("enable_observingconditions", |a| {
            a.help(fl!(loader, "cli-help-enable-observingconditions"))
        })
        .mut_arg("log_level", |a| a.help(fl!(loader, "cli-help-log-level")));

    let matches = cmd.get_matches();
    Args::from_arg_matches(&matches)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => e.exit(),
    };

    // Setup tracing
    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .init();

    tracing::debug!(
        "Parsed command line arguments: config={:?}, port={:?}, server_port={:?}, log_level={:?}",
        args.config,
        args.port,
        args.server_port,
        args.log_level
    );

    // Load configuration
    let mut config = if let Some(config_path) = &args.config {
        tracing::debug!("Loading configuration from {:?}", config_path);
        load_config(config_path)?
    } else {
        tracing::debug!("Using default configuration");
        Config::default()
    };

    // Apply CLI overrides
    if let Some(port) = args.port {
        config.serial.port = port;
    }
    if let Some(server_port) = args.server_port {
        config.server.port = server_port;
    }
    if let Some(enable) = args.enable_switch {
        config.switch.enabled = enable;
    }
    if let Some(enable) = args.enable_observingconditions {
        config.observingconditions.enabled = enable;
    }

    tracing::info!("Starting PPBA driver");
    #[cfg(feature = "mock")]
    tracing::info!("Running in MOCK MODE - no real hardware");
    #[cfg(not(feature = "mock"))]
    tracing::info!("Serial port: {}", config.serial.port);
    tracing::info!("Baud rate: {}", config.serial.baud_rate);
    tracing::info!("Server port: {}", config.server.port);

    #[cfg(feature = "mock")]
    let bound = {
        let factory = std::sync::Arc::new(MockSerialPortFactory::default());
        ServerBuilder::new(config)
            .with_factory(factory)
            .build()
            .await?
    };

    #[cfg(not(feature = "mock"))]
    let bound = ServerBuilder::new(config).build().await?;

    // Race the server with a shutdown signal so SIGTERM triggers a clean
    // exit, allowing llvm-cov profraw data to be flushed.
    tokio::select! {
        result = bound.start() => { result?; },
        () = shutdown_signal() => {
            tracing::debug!("shutting down");
        }
    }

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::debug!("received Ctrl+C"),
        () = terminate => tracing::debug!("received SIGTERM"),
    }
}
