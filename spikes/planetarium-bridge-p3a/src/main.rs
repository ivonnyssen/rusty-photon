//! P3a verification spike: a virtual ASCOM Alpaca Telescope that answers a
//! planetarium client plausibly while logging everything the client sends.
//!
//! Throwaway observation harness — it exists to answer the P3a questions in
//! docs/plans/planetarium-target-import.md against a real SkySafari install:
//! discovery/connection lifecycle, declared-epoch honoring (J2000 vs JNow),
//! which slew/sync verbs the client uses, the position-report cadence needed
//! to look connected, and whether arbitrary-point GoTo is possible at all.
//! Findings land in docs/services/planetarium-bridge.md; this crate is then
//! deleted. It is NOT the planetarium-bridge service and never touches
//! hardware or rp.

mod discovery;
mod epoch;
mod telescope;
mod wirelog;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::telescope::EquatorialCoordinateType;
use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use axum::middleware;
use clap::{Parser, ValueEnum};
use rp_ephemeris::{Ephemeris, ErfarsEphemeris, Site};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

use crate::epoch::EpochInference;
use crate::telescope::SpikeTelescope;
use crate::wirelog::WireLog;

/// Coordinate system the virtual device declares via `EquatorialSystem`.
/// A CLI flag so the J2000-honoring experiment can be re-run with a
/// different declaration without editing code.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum EquatorialSystemArg {
    /// Declare J2000 (the planetarium-bridge design assumption).
    J2000,
    /// Declare topocentric / equinox-of-date.
    Topocentric,
    /// Declare "other/unknown".
    Other,
}

impl From<EquatorialSystemArg> for EquatorialCoordinateType {
    fn from(value: EquatorialSystemArg) -> Self {
        match value {
            EquatorialSystemArg::J2000 => Self::J2000,
            EquatorialSystemArg::Topocentric => Self::Topocentric,
            EquatorialSystemArg::Other => Self::Other,
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "P3a spike: virtual Alpaca Telescope that logs planetarium client behavior")]
struct Args {
    /// Address to serve the Alpaca HTTP API on.
    #[arg(long, default_value = "0.0.0.0:11126")]
    listen: SocketAddr,

    /// UDP port for the Alpaca discovery responder.
    #[arg(long, default_value_t = 32227)]
    discovery_port: u16,

    /// Disable the discovery responder (manual IP:port entry only).
    #[arg(long)]
    no_discovery: bool,

    /// Site latitude in degrees, north positive. Defaults to Greenwich —
    /// pass your real (city-level) coordinates so SkySafari's alt-az /
    /// horizon display matches what it expects.
    #[arg(long, default_value_t = 51.4769, allow_hyphen_values = true)]
    latitude: f64,

    /// Site longitude in degrees, east positive.
    #[arg(long, default_value_t = -0.0005, allow_hyphen_values = true)]
    longitude: f64,

    /// Site elevation in meters (reported via SiteElevation only).
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    elevation: f64,

    /// Simulated slew convergence time in seconds. The spike reports
    /// Slewing=true and interpolates the position report for this long
    /// after every GoTo.
    #[arg(long, default_value_t = 3.0)]
    slew_secs: f64,

    /// Coordinate system the device declares via `EquatorialSystem`.
    #[arg(long, value_enum, default_value_t = EquatorialSystemArg::J2000)]
    equatorial_system: EquatorialSystemArg,

    /// Path of the JSONL wire log (every HTTP request/response and every
    /// discovery packet, timestamped to the millisecond).
    #[arg(long, default_value = "p3a-wire.jsonl")]
    log_file: PathBuf,

    /// Extra epoch-inference probe object names (repeatable). Resolved
    /// against the embedded rp-catalog; added to the built-in bright-object
    /// list. GoTo one of these in the planetarium for a clean J2000-vs-JNow
    /// verdict.
    #[arg(long = "probe")]
    probes: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,planetarium_bridge_p3a=debug")),
        )
        .init();

    let args = Args::parse();

    let site = Site::new(args.latitude, args.longitude)?;
    if (args.latitude - 51.4769).abs() < f64::EPSILON {
        warn!(
            "using the Greenwich default site — pass --latitude/--longitude (city-level \
             is fine) so the alt-az the device reports matches the planetarium's sky"
        );
    }

    // Warm up ERFA's lazily-initialized leap-second table single-threaded,
    // before concurrent Alpaca polls and epoch inference can race its first
    // initialization.
    let ephemeris = ErfarsEphemeris::new();
    let lst = ephemeris.sidereal_time(&site, chrono::Utc::now());
    debug!(lst_hours = lst.lst_hours, "ERFA warmed up; LST computed");

    let inference = EpochInference::with_default_and_extra_probes(&args.probes);

    let wire_log = Arc::new(WireLog::create(&args.log_file)?);
    info!("wire log: {}", args.log_file.display());

    let telescope = SpikeTelescope::new(
        site,
        args.elevation,
        ephemeris,
        inference,
        Duration::from_secs_f64(args.slew_secs.max(0.1)),
        args.equatorial_system.into(),
    );

    let mut server = Server::new(CargoServerInfo!());
    server.devices.register(telescope);

    let app = axum::Router::new()
        .fallback_service(server.into_service())
        .layer(middleware::from_fn_with_state(
            Arc::clone(&wire_log),
            wirelog::wire_middleware,
        ));

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let local_addr = listener.local_addr()?;
    info!(
        "Alpaca Telescope serving on http://{local_addr} — point SkySafari at this \
         machine's LAN IP, port {}, device number 0",
        local_addr.port()
    );

    if args.no_discovery {
        info!("discovery responder disabled (--no-discovery)");
    } else {
        tokio::spawn(discovery::run(
            args.discovery_port,
            local_addr.port(),
            Arc::clone(&wire_log),
        ));
    }

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!("ctrl-c handler failed: {e}");
        }
        info!("shutting down");
    })
    .await?;

    wire_log.flush();
    info!(
        "wire log written to {} — analyze cadence with e.g. \
         `jq -r '.t' {} | uniq -c`",
        args.log_file.display(),
        args.log_file.display()
    );
    Ok(())
}
