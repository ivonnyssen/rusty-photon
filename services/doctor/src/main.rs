//! rusty-photon-doctor CLI (docs/services/doctor.md §CLI contract).
//!
//! One-shot: diagnose (and repair, with --fix), print, exit. Exit 0 = no
//! failing check (warnings allowed; post-fix state on a --fix run), 1 = at
//! least one failure, 2 = doctor itself could not run.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use doctor::facts::PlatformFacts;
use tracing_subscriber::EnvFilter;

/// Diagnoses a multi-service rusty-photon install: config files, ports,
/// and cross-service wiring. Read-only.
#[derive(Debug, Parser)]
#[command(name = "rusty-photon-doctor", version)]
struct Cli {
    /// Config directory to diagnose. Default: /etc/rusty-photon when the
    /// packaged symlink exists (Unix), else the platform config directory
    /// the services themselves use.
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// Emit the DoctorReport JSON instead of the human-readable report.
    #[arg(long)]
    json: bool,
    /// Apply the machine-applicable fixes, re-diagnose, and report the
    /// post-fix state. Everything else stays read-only.
    #[arg(long)]
    fix: bool,
    /// Test affordance: read platform facts from a JSON file instead of
    /// querying the host's service manager.
    #[cfg(feature = "mock")]
    #[arg(long)]
    platform_facts: Option<PathBuf>,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();

    let facts = match gather_facts(&cli) {
        Ok(facts) => facts,
        Err(e) => {
            eprintln!("doctor: {e}");
            return ExitCode::from(2);
        }
    };
    let config_dir = match doctor::resolve_config_dir(cli.config_dir) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("doctor: {e}");
            return ExitCode::from(2);
        }
    };

    let report = if cli.fix {
        if !facts.units.is_empty() {
            // The canonical flow runs --fix with services live: atomic
            // renames make corruption impossible, but a driver's own
            // config.apply landing mid-fix loses one of the two writes.
            eprintln!(
                "doctor: services may be running while fixes are written — a \
                 concurrent config change can lose one write; re-run doctor to \
                 verify, and restart services to pick up fixed configs"
            );
        }
        match doctor::diagnose_and_fix(config_dir, facts) {
            Ok(report) => report,
            Err(e) => {
                eprintln!("doctor: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        doctor::diagnose(config_dir, facts)
    };
    if cli.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("doctor: could not serialize the report: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        print!("{}", doctor::render::render(&report));
    }
    if report.has_failures() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(feature = "mock")]
fn gather_facts(cli: &Cli) -> Result<PlatformFacts, String> {
    match &cli.platform_facts {
        Some(path) => PlatformFacts::load(path),
        None => Ok(PlatformFacts::gather()),
    }
}

#[cfg(not(feature = "mock"))]
fn gather_facts(_cli: &Cli) -> Result<PlatformFacts, String> {
    Ok(PlatformFacts::gather())
}
