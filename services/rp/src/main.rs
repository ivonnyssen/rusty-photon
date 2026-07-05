use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rusty_photon_service_lifecycle::{
    init_tracing, report_from_boxed, ServiceResult, ServiceRunner, Shutdown,
};
use tracing::{debug, Level};

#[derive(Parser)]
#[command(name = "rp", about = "Rusty Photon - equipment gateway and event bus")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to configuration file (shorthand for `rp serve --config`)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info", value_parser = clap::value_parser!(Level))]
    log_level: Level,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the rp server
    Serve {
        /// Path to configuration file. Defaults to the per-user platform
        /// config directory (e.g. `~/.config/rusty-photon/rp.json` on
        /// Linux); created with a minimal scaffold on first start if absent.
        #[arg(long)]
        config: Option<PathBuf>,

        /// Log level (trace, debug, info, warn, error)
        #[arg(long, default_value = "info", value_parser = clap::value_parser!(Level))]
        log_level: Level,
    },
    /// Hash a password for use in service auth configuration
    HashPassword {
        /// Log level (trace, debug, info, warn, error)
        #[arg(long, default_value = "info", value_parser = clap::value_parser!(Level))]
        log_level: Level,

        /// Read password from stdin (no prompt, no confirmation)
        #[arg(long)]
        stdin: bool,
    },
    /// Generate TLS certificates for all services
    InitTls {
        /// Output directory (default: ~/.rusty-photon/pki)
        #[arg(long)]
        output_dir: Option<String>,

        /// Services to generate certs for (default: all known services)
        #[arg(long)]
        services: Option<Vec<String>>,

        /// Additional SANs (hostnames or IPs) to include in certificates
        #[arg(long)]
        extra_san: Option<Vec<String>>,

        /// Use ACME/Let's Encrypt instead of self-signed CA
        #[arg(long)]
        acme: bool,

        /// Domain for ACME wildcard certificate (requires --acme)
        #[arg(long, requires = "acme")]
        domain: Option<String>,

        /// DNS provider for ACME challenge (e.g., "cloudflare") (requires --acme)
        #[arg(long, requires = "acme")]
        dns_provider: Option<String>,

        /// DNS provider API token (requires --acme)
        #[arg(long, requires = "acme")]
        dns_token: Option<String>,

        /// Email for ACME account registration (requires --acme)
        #[arg(long, requires = "acme")]
        email: Option<String>,

        /// Use Let's Encrypt staging environment (requires --acme)
        #[arg(long, requires = "acme")]
        staging: bool,

        /// Log level (trace, debug, info, warn, error)
        #[arg(long, default_value = "info", value_parser = clap::value_parser!(Level))]
        log_level: Level,
    },
}

fn main() -> ServiceResult {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve { config, log_level }) => {
            init_tracing(log_level);
            run_serve(config)
        }
        Some(Commands::HashPassword { log_level, stdin }) => {
            init_tracing(log_level);
            rp::hash_password_cmd::run(stdin).map_err(report_from_boxed)
        }
        Some(Commands::InitTls {
            output_dir,
            services,
            extra_san,
            acme,
            domain,
            dns_provider,
            dns_token,
            email,
            staging,
            log_level,
        }) => {
            init_tracing(log_level);
            if acme {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(rp::tls_cmd::run_acme(
                    output_dir.as_deref(),
                    domain.as_deref(),
                    dns_provider.as_deref(),
                    dns_token.as_deref(),
                    email.as_deref(),
                    staging,
                ))
                .map_err(report_from_boxed)
            } else {
                rp::tls_cmd::run(
                    output_dir.as_deref(),
                    services.as_deref(),
                    &extra_san.unwrap_or_default(),
                )
                .map_err(report_from_boxed)
            }
        }
        None => {
            // No subcommand serves (packaged units run a bare
            // `/usr/bin/rusty-photon-rp`); `rp --config <path>` still works
            // as a shorthand for `rp serve --config <path>`.
            init_tracing(cli.log_level);
            run_serve(cli.config)
        }
    }
}

fn run_serve(config: Option<PathBuf>) -> ServiceResult {
    // Minimal runnable scaffold (no equipment, default port), written on
    // first start at the XDG default path only — an explicit `--config`
    // naming a missing file stays a hard error. `session.data_directory`
    // matches the packaged unit's StateDirectory.
    let default_config = serde_json::json!({
        "session": { "data_directory": "/var/lib/rusty-photon/rp/data" },
        "equipment": {},
        "server": {}
    });
    let config_path = rusty_photon_config::resolve_and_init("rp", config, &default_config)?;
    ServiceRunner::new("rp").run(move |shutdown: Shutdown| async move {
        debug!(config_path = %config_path.display(), "loading configuration");
        let config = rp::config::load_config(&config_path)?;

        rp::ServerBuilder::new()
            .with_config(config)
            .build()
            .await?
            .start(shutdown.cancelled())
            .await?;

        Ok(())
    })
}
