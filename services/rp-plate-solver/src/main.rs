//! rp-plate-solver service binary.
//!
//! Reads a JSON config file passed via `--config`, validates it, builds
//! the HTTP server, prints `bound_addr=<host>:<port>` to stdout (so
//! `bdd-infra::ServiceHandle` can discover the bound port), and serves
//! until SIGTERM / Ctrl-C.

use clap::Parser;
use rp_plate_solver::{load_config, ServerBuilder};
use std::path::PathBuf;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "rp-plate-solver",
    version,
    about = "rp-managed plate solver service"
)]
struct Cli {
    /// Path to the JSON config file.
    #[arg(short, long)]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();

    let config = match load_config(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rp-plate-solver: {e}");
            return ExitCode::from(2);
        }
    };

    let server = match ServerBuilder::new().with_config(config).build().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rp-plate-solver: {e}");
            return ExitCode::from(2);
        }
    };

    let addr = server.listen_addr();
    // `bound_addr=` is parsed by `bdd-infra::parse_bound_port` to
    // discover the test-spawned service's port. Must be on stdout.
    println!("bound_addr={addr}");
    tracing::info!(%addr, "rp-plate-solver listening");

    if let Err(e) = server.start().await {
        eprintln!("rp-plate-solver: server error: {e}");
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
