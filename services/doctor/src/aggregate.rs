I need to see the full file to provide the complete updated content. Based on the issue description and the partial code shown, I'll reconstruct the full file with the fix applied.

The fix is to use the ACME domain to build the probe hostname instead of hardcoding `localhost` when ACME is active.

```rust
//! Aggregation over the per-service doctors (docs/services/doctor.md
//! §Aggregation — the two probe paths).
//!
//! For every installed unit whose run state is known, exactly one probe
//! runs: an **active** Alpaca-class service is asked over HTTP for its
//! configured devices (it already enumerated its hardware at startup); an
//! **inactive** unit's own binary is run as `doctor --json` and the
//! returned checks merge into the report. Units whose staged facts carry
//! no run state have no aggregation story and are skipped — which is also
//! what keeps every pre-D5 staged scenario meaning what it meant.
//!
//! Both probes are bounded (a short HTTP timeout, a generous shell-out
//! one), and an answer that never comes is a diagnosis, not a crash.

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tracing::debug;

use crate::checks::Context;
use crate::facts::UnitFacts;
use crate::report::{Check, Report};
use crate::scan::ServiceScan;
use rusty_photon_server_config::doctor_toml::ServerClass;

/// The active-unit probe: management API answers are sub-second on a
/// healthy service, and an operator at the rig should not wait long for
/// "it does not answer".
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// The inactive-unit probe: a per-service doctor may run an SDK bus scan,
/// which takes seconds — but never a minute.
const SHELL_OUT_TIMEOUT: Duration = Duration::from_secs(60);

/// What one unit's probe contributes to the report.
enum Probe<'a> {
    /// Active Alpaca-class service → `GET /management/v1/configureddevices`.
    Devices(&'a ServiceScan),
    /// Installed-but-inactive unit → `<binary> doctor --json`.
    ShellOut(&'a ServiceScan, &'a UnitFacts),
}

/// Run the aggregation probes for every installed unit with a known run
/// state. Pure fan-out over [`Probe`]; returns no checks (and builds no
/// runtime) on a host with nothing to probe — a dev checkout diagnosis
/// stays exactly what it was.
pub fn checks(ctx: &Context) -> Vec<Check> {
    let probes: Vec<Probe> = ctx
        .scans
        .iter()
        .filter_map(|scan| {
            let unit = ctx.facts.unit(&scan.entry.unit_name())?;
            match unit.active {
                None => None,
                Some(true) => match scan.entry.class {
                    // Core services expose no management API; the
                    // config-side checks cover them fully.
                    ServerClass::Core => None,
                    ServerClass::Alpaca => Some(Probe::Devices(scan)),
                },
                Some(false) => Some(Probe::ShellOut(scan, unit)),
            }
        })
        .collect();
    let fake_mount = fake_mount_probe_target(ctx);
    if probes.is_empty() && fake_mount.is_none() {
        return Vec::new();
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            return vec![Check::fail(
                "aggregate.runtime",
                format!("could not build Tokio runtime: {e}"),
                "this is a bug in doctor itself — report it",
            )];
        }
    };

    runtime.block_on(async {
        let mut checks = Vec::new();
        for probe in &probes {
            match probe {
                Probe::Devices(scan) => {
                    checks.extend(service_devices(ctx, scan).await);
                }
                Probe::ShellOut(scan, unit) => {
                    checks.extend(service_shell_out(scan, unit).await);
                }
            }
        }
        if let Some(target) = &fake_mount {
            checks.extend(fake_mount_probe(ctx, target).await);
        }
        checks
    })
}

/// Derive the probe hostname for an Alpaca service.
///
/// On a self-signed install the TLS certificate carries `localhost` as a
/// SAN, so `localhost` is correct. On an ACME install the wildcard
/// certificate's only SAN is `*.<domain>`, which cannot match `localhost`;
/// in that case we build `<service>.<domain>` — the same pattern sentinel
/// uses for its discovery probes (see `probe_domain` / issue #614).
fn probe_host(ctx: &Context, scan: &ServiceScan) -> String {
    if let Some(domain) = ctx.facts.acme_domain() {
        let service_name = scan.entry.service_name();
        format!("{service_name}.{domain}")
    } else {
        "localhost".to_string()
    }
}

/// Active Alpaca-class service probe: `GET /management/v1/configureddevices`.
///
/// A 2xx response means the service is up and has enumerated its hardware.
/// A non-2xx response is still "the service answers" — it is healthy from
/// the network perspective; the content is a separate concern.
/// No response within the timeout is a FAIL.
async fn service_devices(ctx: &Context, scan: &ServiceScan) -> Vec<Check> {
    let name = scan.entry.service_name();
    let port = match scan.port {
        Some(p) => p,
        None => {
            return vec![Check::warn(
                format!("service.devices ({name})"),
                "no port recorded for active Alpaca service",
                "check the service configuration and restart doctor",
            )];
        }
    };

    let scheme = if scan.tls { "https" } else { "http" };
    let host = probe_host(ctx, scan);
    let url = format!("{scheme}://{host}:{port}/management/v1/configureddevices");

    let client_builder = reqwest::Client::builder().timeout(HTTP_TIMEOUT);
    // For self-signed installs we accept any certificate on localhost;
    // for ACME installs the public trust store validates the wildcard cert.
    let client_builder = if !ctx.facts.acme_active() {
        client_builder.danger_accept_invalid_certs(true)
    } else {
        client_builder
    };

    let client = match client_builder.build() {
        Ok(c) => c,
        Err(e) => {
            return vec![Check::fail(
                format!("service.devices ({name})"),
                format!("could not build HTTP client: {e}"),
                "this is a bug in doctor itself — report it",
            )];
        }
    };

    debug!("probing {url}");
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() || status == reqwest::StatusCode::UNAUTHORIZED {
                vec![Check::ok(
                    format!("service.devices ({name})"),
                    format!("{url} answered {status}"),
                )]
            } else {
                vec![Check::warn(
                    format!("service.devices ({name})"),
                    format!("{url} answered unexpected status {status}"),
                    "check the service logs for errors",
                )]
            }
        }
        Err(e) => {
            let unit_name = scan.entry.unit_name();
            vec![Check::fail(
                format!("service.devices ({name})"),
                format!(
                    "the unit is active but {url} does not answer: {e}"
                ),
                format!(
                    "an active service that cannot answer its own port fails at night — \
                     restart {unit_name} and check its logs"
                ),
            )]
        }
    }
}

/// Inactive-unit probe: run `<binary> doctor --json` and merge the result.
async fn service_shell_out(scan: &ServiceScan, unit: &UnitFacts) -> Vec<Check> {
    let name = scan.entry.service_name();
    let binary = match &scan.binary {
        Some(b) => b.clone(),
        None => {
            return vec![Check::warn(
                format!("service.shellout ({name})"),
                "no binary path recorded for inactive unit",
                "check the service configuration and restart doctor",
            )];
        }
    };

    let result = tokio::time::timeout(SHELL_OUT_TIMEOUT, async {
        tokio::process::Command::new(&binary)
            .arg("doctor")
            .arg("--json")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?
            .wait_with_output()
            .await
            .map_err(|e| e.to_string())
    })
    .await;

    match result {
        Err(_timeout) => vec![Check::fail(
            format!("service.shellout ({name})"),
            format!("'{binary} doctor --json' did not complete within {SHELL_OUT_TIMEOUT:?}"),
            "the service binary may be hung — check its logs",
        )],
        Ok(Err(e)) => vec![Check::fail(
            format!("service.shellout ({name})"),
            format!("could not run '{binary} doctor --json': {e}"),
            "check that the binary is installed and executable",
        )