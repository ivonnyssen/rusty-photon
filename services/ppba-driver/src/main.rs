//! PPBA Switch Driver CLI
//!
//! Command-line interface for the Pegasus Astro Pocket Powerbox Advance Gen2 Switch driver.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Parser;
use rust_embed::RustEmbed;
use rusty_photon_i18n::{fl, fluent_language_loader, LocalizedParser};
use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
use tracing::Level;

use ppba_driver::config::{load_effective_config, CliOverrides};
#[cfg(feature = "mock")]
use ppba_driver::{Config, MockPpbaTransportFactory, ServerBuilder};
#[cfg(not(feature = "mock"))]
use ppba_driver::{Config, ServerBuilder};

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

#[derive(Parser, LocalizedParser)]
#[command(name = "ppba-driver")]
#[command(version)]
#[localized(about = "cli-about")]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    #[localized(help = "cli-help-config")]
    config: Option<PathBuf>,

    /// Serial port path (overrides config file)
    #[arg(long)]
    #[localized(help = "cli-help-port")]
    port: Option<String>,

    /// Server port (overrides config file)
    #[arg(long)]
    #[localized(help = "cli-help-server-port")]
    server_port: Option<u16>,

    /// Enable/disable Switch device
    #[arg(long)]
    #[localized(help = "cli-help-enable-switch")]
    enable_switch: Option<bool>,

    /// Enable/disable ObservingConditions device
    #[arg(long)]
    #[localized(help = "cli-help-enable-observingconditions")]
    enable_observingconditions: Option<bool>,

    /// Log level
    #[arg(short, long, default_value = "info", value_parser = parse_log_level)]
    #[localized(help = "cli-help-log-level")]
    log_level: Level,

    /// Run as a Windows service (used by the service control manager).
    /// No-op on non-Windows targets. Hidden, so deliberately not localized.
    #[arg(long, hide = true)]
    service: bool,
}

fn parse_log_level(s: &str) -> Result<Level, String> {
    s.parse().map_err(|_| {
        rusty_photon_i18n::fl_active(|loader| fl!(loader, "error-invalid-log-level", value = s))
            .unwrap_or_else(|| {
                format!(
                    "Invalid log level: {}. Use: trace, debug, info, warn, error",
                    s
                )
            })
    })
}

fn main() -> ServiceResult {
    let (loader, i18n_status) = rusty_photon_i18n::init(fluent_language_loader!(), &Localizations);
    let args = Args::parse_localized(&loader);

    // Setup tracing. In Windows SCM service mode logs go to the rolling file
    // under %PROGRAMDATA%\rusty-photon\logs\; hold the guard until process exit
    // so the final lines flush on SCM Stop. Console mode logs to stderr as before.
    let _tracing_guard = rusty_photon_service_lifecycle::init_service_tracing(
        "ppba-driver",
        args.log_level,
        args.service,
    );

    match i18n_status {
        Ok(()) => {}
        Err(rusty_photon_i18n::LoadError::Available { reason }) => {
            tracing::warn!(
                %reason,
                "i18n: failed to enumerate embedded locales; running with English fallback"
            );
        }
        Err(rusty_photon_i18n::LoadError::Load { reason }) => {
            tracing::warn!(
                %reason,
                "i18n: failed to load negotiated locale bundle; running with English fallback"
            );
        }
        Err(rusty_photon_i18n::LoadError::AlreadyInitialized) => {
            // Distinct from the load-failure cases: the loader is *not*
            // English-fallback-only, it's just whatever the first init
            // populated. Surfaces the most likely cause (refactor or test
            // artefact) so it's visible without misrepresenting the locale.
            tracing::warn!(
                "i18n: rusty_photon_i18n::init was called more than once on this thread; \
                 second call's loader was discarded, active locale unchanged"
            );
        }
    }

    tracing::debug!(
        "Parsed command line arguments: config={:?}, port={:?}, server_port={:?}, log_level={:?}",
        args.config,
        args.port,
        args.server_port,
        args.log_level
    );

    // Resolve the config path (explicit `--config`, else the per-user platform
    // config directory) and mint a UUIDv4 `UniqueID` for each device on first
    // run. `materialize_identity` is idempotent: it only fills empty/absent ids,
    // never overwrites an existing one, and persists atomically. When the file
    // is absent it writes the default scaffold (with freshly-minted ids), so the
    // subsequent load always succeeds.
    let config_path = rusty_photon_config::resolve_config_path("ppba-driver", args.config.clone())?;
    tracing::debug!("Resolved configuration path: {:?}", config_path);

    let outcome = rusty_photon_config::materialize_identity(
        &config_path,
        &serde_json::to_value(Config::default())?,
        &["/switch/unique_id", "/observingconditions/unique_id"],
    )?;
    if outcome.wrote {
        tracing::debug!(
            "Minted and persisted device identities at {:?}: {:?}",
            config_path,
            outcome.filled
        );
    } else {
        tracing::debug!("Device identities already present at {:?}", config_path);
    }

    // CLI overrides are tracked (not just applied) so config.apply keeps them
    // out of the persisted file — a transient `--port` / `--enable-switch` is
    // never baked in.
    let overrides = CliOverrides {
        serial_port: args.port.clone(),
        server_port: args.server_port,
        enable_switch: args.enable_switch,
        enable_observingconditions: args.enable_observingconditions,
    };

    tracing::info!("Starting PPBA driver");
    #[cfg(feature = "mock")]
    tracing::info!("Running in MOCK MODE - no real hardware");

    // Reload loop: a `config.apply` that changes a field fires the reload signal
    // (from either device); the loop re-reads the freshly-persisted config and
    // rebuilds the server, awaiting `start()` to completion so the old server
    // drains HTTP and releases the serial port before the rebuilt one binds.
    ServiceRunner::new("ppba-driver")
        .with_reload()
        .scm_mode(args.service)
        .run_with_reload(move |shutdown, reload| async move {
            loop {
                let config = load_effective_config(&config_path, &overrides)?;
                #[cfg(not(feature = "mock"))]
                tracing::info!("Serial port: {}", config.serial.port);
                tracing::info!("Server port: {}", config.server.port);

                #[cfg(feature = "mock")]
                let bound = {
                    let factory = Arc::new(MockPpbaTransportFactory::default());
                    ServerBuilder::new(config)
                        .with_factory(factory)
                        .with_config_source(config_path.clone(), overrides.clone())
                        .with_reload_signal(reload.clone())
                        .build()
                        .await?
                };
                #[cfg(not(feature = "mock"))]
                let bound = ServerBuilder::new(config)
                    .with_config_source(config_path.clone(), overrides.clone())
                    .with_reload_signal(reload.clone())
                    .build()
                    .await?;

                let reloaded = Arc::new(AtomicBool::new(false));
                let stop = {
                    let reloaded = Arc::clone(&reloaded);
                    let shutdown = shutdown.cancelled();
                    let reload = reload.clone();
                    async move {
                        tokio::select! {
                            () = shutdown => {}
                            () = reload.recv() => reloaded.store(true, Ordering::SeqCst),
                        }
                    }
                };
                bound.start(stop).await?;

                if reloaded.load(Ordering::SeqCst) {
                    tracing::debug!("reloading ppba-driver configuration");
                    continue;
                }
                return Ok(());
            }
        })
}
