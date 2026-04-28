use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use tracing::debug;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "rp", about = "Rusty Photon - equipment gateway and event bus")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to configuration file (shorthand for `rp serve --config`)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the rp server
    Serve {
        /// Path to configuration file
        #[arg(long)]
        config: PathBuf,

        /// Log level (trace, debug, info, warn, error)
        #[arg(long, default_value = "info")]
        log_level: String,
    },
    /// Hash a password for use in service auth configuration
    HashPassword {
        /// Log level (trace, debug, info, warn, error)
        #[arg(long, default_value = "info")]
        log_level: String,

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
        #[arg(long, default_value = "info")]
        log_level: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve { config, log_level }) => {
            init_tracing(&log_level);
            run_serve(&config).await
        }
        Some(Commands::HashPassword { log_level, stdin }) => {
            init_tracing(&log_level);
            rp::hash_password_cmd::run(stdin)
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
            init_tracing(&log_level);
            if acme {
                rp::tls_cmd::run_acme(
                    output_dir.as_deref(),
                    domain.as_deref(),
                    dns_provider.as_deref(),
                    dns_token.as_deref(),
                    email.as_deref(),
                    staging,
                )
                .await
            } else {
                rp::tls_cmd::run(
                    output_dir.as_deref(),
                    services.as_deref(),
                    &extra_san.unwrap_or_default(),
                )
            }
        }
        None => {
            // Backward compat: `rp --config <path>` works without a subcommand
            let config = cli.config.ok_or(
                "no subcommand given. Use `rp serve --config <path>` or `rp --config <path>`",
            )?;
            init_tracing(&cli.log_level);
            run_serve(&config).await
        }
    }
}

fn init_tracing(log_level: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level)),
        )
        .init();
}

async fn run_serve(config_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    debug!(config_path = %config_path.display(), "loading configuration");
    let config = rp::config::load_config(config_path)?;

    rp::ServerBuilder::new()
        .with_config(config)
        .build()
        .await?
        .start()
        .await?;

    Ok(())
}
