//! plate-solver service binary.
//!
//! Reads a JSON config file (`--config`, or when omitted the
//! platform config path — `~/.config/rusty-photon/plate-solver.json` on
//! Linux), validates it, builds the HTTP server, prints
//! `bound_addr=<host>:<port>` to stdout (so `bdd-infra::ServiceHandle`
//! can discover the bound port), and serves until SIGTERM / Ctrl-C.

use clap::Parser;
use plate_solver::{load_config, ServerBuilder};
use rusty_photon_service_lifecycle::{init_service_tracing, ServiceRunner};
use std::path::PathBuf;
use std::process::ExitCode;
use tracing::Level;

#[derive(Parser, Debug)]
#[command(
    name = "plate-solver",
    version,
    about = "rp-managed plate solver service"
)]
struct Cli {
    /// Path to the JSON config file. Defaults to the platform
    /// config directory (e.g. `~/.config/rusty-photon/plate-solver.json`
    /// on Linux). There is no built-in default config: the file must
    /// exist (`astap_binary_path` / `astap_db_directory` are mandatory).
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info", value_parser = clap::value_parser!(Level))]
    log_level: Level,

    /// Run as a Windows service (used by the service control manager).
    /// No-op on non-Windows targets.
    #[arg(long, hide = true)]
    service: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // In Windows SCM service mode logs go to the rolling file under
    // %PROGRAMDATA%\rusty-photon\logs\; hold the guard until process exit so
    // the final lines flush on SCM Stop. Console mode logs to stderr as before.
    let _tracing_guard = init_service_tracing("plate-solver", cli.log_level, cli.service);

    // Path resolution and config load share one failure arm: both are
    // startup config errors (exit 2). `resolve_config_path` fails only
    // when no home directory is resolvable at all (HOME unset/empty and
    // no passwd entry for the uid — a stripped-container state tests
    // can't reproduce), so a dedicated arm would stay uncovered.
    let config = match rusty_photon_config::resolve_config_path("plate-solver", cli.config)
        .map_err(Box::<dyn std::error::Error>::from)
        .and_then(|path| load_config(&path).map_err(Box::from))
    {
        Ok(c) => c,
        Err(e) => {
            // Through tracing, not eprintln!: in console mode the subscriber
            // writes to stderr anyway, and in Windows SCM service mode stderr
            // is a dead handle — tracing is what lands in the rolling log file.
            tracing::error!("plate-solver: {e}");
            return ExitCode::from(2);
        }
    };

    let result = ServiceRunner::new("plate-solver")
        .scm_mode(cli.service)
        .run(move |shutdown| async move {
            let server = ServerBuilder::new()
                .with_config(config)
                .build()
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::from(format!("build: {e}"))
                })?;

            let addr = server.listen_addr();
            // `bound_addr=` is parsed by `bdd-infra::parse_bound_port` to
            // discover the test-spawned service's port. Must be on stdout.
            // Console mode only: stdout is a dead handle under the Windows SCM,
            // and the only stdout consumer (bdd-infra's port parser) never runs
            // services with --service.
            if !rusty_photon_service_lifecycle::is_scm_service() {
                println!("bound_addr={addr}");
            }
            tracing::info!(%addr, "plate-solver listening");

            server.start(shutdown.cancelled()).await.map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::from(format!("server: {e}"))
                },
            )?;
            Ok(())
        });

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let msg = e.to_string();
            // `{e:?}` on the runner's `Report` prints the full multi-line
            // error chain (ADR-011), matching what the other services get
            // by returning `ServiceResult` from `main`. Through tracing, not
            // eprintln!: console mode reaches stderr via the subscriber, and
            // in SCM service mode this lands in the rolling log file instead
            // of a dead handle.
            tracing::error!("plate-solver: {e:?}");
            // Preserve the prior split: config / build failures returned
            // 2, runtime errors returned 1. The closure surfaces both via
            // the runner's `Report`; we recover the distinction by tagging
            // build failures with a "build: " prefix in the closure above.
            if msg.starts_with("build: ") {
                ExitCode::from(2)
            } else {
                ExitCode::from(1)
            }
        }
    }
}
