//! plate-solver service binary.
//!
//! Reads a JSON config file passed via `--config`, validates it, builds
//! the HTTP server, prints `bound_addr=<host>:<port>` to stdout (so
//! `bdd-infra::ServiceHandle` can discover the bound port), and serves
//! until SIGTERM / Ctrl-C.

use clap::Parser;
use plate_solver::{load_config, ServerBuilder};
use rusty_photon_service_lifecycle::ServiceRunner;
use std::path::PathBuf;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "plate-solver",
    version,
    about = "rp-managed plate solver service"
)]
struct Cli {
    /// Path to the JSON config file.
    #[arg(short, long)]
    config: PathBuf,
}

fn main() -> ExitCode {
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
            eprintln!("plate-solver: {e}");
            return ExitCode::from(2);
        }
    };

    let result = ServiceRunner::new("plate-solver").run(move |shutdown| async move {
        let server = ServerBuilder::new()
            .with_config(config)
            .build()
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { Box::from(format!("build: {e}")) })?;

        let addr = server.listen_addr();
        // `bound_addr=` is parsed by `bdd-infra::parse_bound_port` to
        // discover the test-spawned service's port. Must be on stdout.
        println!("bound_addr={addr}");
        tracing::info!(%addr, "plate-solver listening");

        server
            .start(shutdown.cancelled())
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { Box::from(format!("server: {e}")) })?;
        Ok(())
    });

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let msg = e.to_string();
            eprintln!("plate-solver: {msg}");
            // Preserve the prior split: config / build failures returned
            // 2, runtime errors returned 1. After the migration the
            // closure surfaces both via Box<dyn Error>; we recover the
            // distinction by tagging build failures with a "build: "
            // prefix in the closure above.
            if msg.starts_with("build: ") {
                ExitCode::from(2)
            } else {
                ExitCode::from(1)
            }
        }
    }
}
