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
    if probes.is_empty() {
        return Vec::new();
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            return vec![Check::warn(
                "service.doctor-probe",
                None,
                format!("could not start the probe runtime: {e}"),
                None,
            )];
        }
    };
    runtime.block_on(async {
        let mut checks = Vec::new();
        for probe in probes {
            match probe {
                Probe::Devices(scan) => checks.push(probe_devices(ctx, scan).await),
                Probe::ShellOut(scan, unit) => {
                    checks.extend(probe_shell_out(ctx, scan, unit).await);
                }
            }
        }
        checks
    })
}

/// The subset of an Alpaca management response the inventory reads.
#[derive(Debug, Deserialize)]
struct ManagementResponse {
    #[serde(rename = "Value", default)]
    value: Vec<ConfiguredDevice>,
}

#[derive(Debug, Deserialize)]
struct ConfiguredDevice {
    #[serde(rename = "DeviceName", default)]
    name: String,
    #[serde(rename = "DeviceType", default)]
    device_type: String,
    #[serde(rename = "DeviceNumber", default)]
    number: u32,
}

/// Ask an active Alpaca service for its configured devices, following the
/// service's own config: HTTPS when its `server.tls` is set (trusting
/// doctor's CA), the observatory credential when its `server.auth` is on.
async fn probe_devices(ctx: &Context, scan: &ServiceScan) -> Check {
    let service = Some(scan.entry.name.to_string());
    let port = scan.effective_port();
    let tls_on = scan.server().is_some_and(|s| s.tls.is_some());
    let auth_on = scan.server().is_some_and(|s| s.auth.is_some());
    let scheme = if tls_on { "https" } else { "http" };
    let url = format!("{scheme}://localhost:{port}/management/v1/configureddevices");
    debug!(service = scan.entry.name, url, "probing the active service");

    let ca_path = tls_on.then(|| {
        rusty_photon_tls::config::ca_cert_path(&crate::provision::pki_dir(&ctx.config_dir))
    });
    let client = match rusty_photon_tls::client::build_reqwest_client(ca_path.as_deref()) {
        Ok(client) => client,
        Err(e) => {
            return Check::warn(
                "service.devices",
                service,
                format!(
                    "the service serves TLS but doctor could not load its trust root: {e} \
                     — the probe was skipped"
                ),
                Some("run `rusty-photon-doctor tls issue` to (re)create the pki tree".to_string()),
            );
        }
    };
    let credential = if auth_on {
        crate::provision::read_credential(&ctx.config_dir)
    } else {
        None
    };
    let mut request = client.get(&url).timeout(HTTP_TIMEOUT);
    if let Some(password) = &credential {
        request = request.basic_auth(crate::provision::CREDENTIAL_USERNAME, Some(password));
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(e) => {
            return Check::fail(
                "service.devices",
                service,
                format!("the unit is active but {url} does not answer: {e}"),
                Some(
                    "an active service that cannot answer its own port fails at night \
                     — restart it and check its logs"
                        .to_string(),
                ),
            );
        }
    };
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        let held = if credential.is_some() {
            "the observatory credential was rejected"
        } else {
            "doctor holds no credential for it (no pki/credential)"
        };
        return Check::warn(
            "service.devices",
            service,
            format!(
                "the service is alive but its management API answered {status} — {held}, \
                 so liveness is proven but the device inventory is not"
            ),
            Some("run `rusty-photon-doctor --fix` to align the observatory credential".to_string()),
        );
    }
    if !status.is_success() {
        return Check::fail(
            "service.devices",
            service,
            format!("the management API answered HTTP {status}"),
            Some("check the service's logs".to_string()),
        );
    }
    match response.json::<ManagementResponse>().await {
        Ok(management) => Check::ok(
            "service.devices",
            service,
            describe_devices(&management.value),
        ),
        Err(e) => Check::fail(
            "service.devices",
            service,
            format!("the management API answered but its payload did not parse: {e}"),
            Some("check the service's logs".to_string()),
        ),
    }
}

fn describe_devices(devices: &[ConfiguredDevice]) -> String {
    if devices.is_empty() {
        return "the service reports 0 configured devices".to_string();
    }
    let listed: Vec<String> = devices
        .iter()
        .map(|d| format!("{} \"{}\" (#{})", d.device_type, d.name, d.number))
        .collect();
    format!(
        "the service reports {} configured device(s): {}",
        devices.len(),
        listed.join(", ")
    )
}

/// Run an inactive unit's own binary as `doctor --json --config <file>` and
/// merge the returned checks. Every way the probe itself can go wrong is a
/// `warn` under `service.doctor-probe` — most of them are the version-skew
/// signature (a binary from before D5 does not know the subcommand), and an
/// old binary is not a broken rig.
async fn probe_shell_out(ctx: &Context, scan: &ServiceScan, unit: &UnitFacts) -> Vec<Check> {
    let name = scan.entry.name;
    let service = Some(name.to_string());
    let Some(binary) = &unit.binary_path else {
        return vec![Check::warn(
            "service.doctor-probe",
            service,
            "the unit is installed but its service manager entry records no binary path, \
             so its own doctor could not be asked",
            None,
        )];
    };
    let config = ctx.config_dir.join(scan.entry.config_file());
    debug!(service = name, binary = %binary.display(), "running the per-service doctor");

    let mut command = tokio::process::Command::new(binary);
    command
        .arg("doctor")
        .arg("--json")
        .arg("--config")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = match tokio::time::timeout(SHELL_OUT_TIMEOUT, command.output()).await {
        Err(_elapsed) => {
            return vec![Check::warn(
                "service.doctor-probe",
                service,
                format!(
                    "{} doctor did not answer within {}s and was stopped",
                    binary.display(),
                    SHELL_OUT_TIMEOUT.as_secs()
                ),
                None,
            )];
        }
        Ok(Err(e)) => {
            return vec![Check::warn(
                "service.doctor-probe",
                service,
                format!("could not run {} doctor: {e}", binary.display()),
                None,
            )];
        }
        Ok(Ok(output)) => output,
    };

    match serde_json::from_slice::<Report>(&output.stdout) {
        Ok(child) if !child.checks.is_empty() => merge_child_checks(child, name),
        Ok(_) => vec![Check::warn(
            "service.doctor-probe",
            service,
            "the per-service doctor returned a report with no checks",
            None,
        )],
        Err(_) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let trail = stderr.lines().next().unwrap_or("").trim();
            vec![Check::warn(
                "service.doctor-probe",
                service,
                format!(
                    "{} did not produce a doctor report (exit {}{}{}) — a binary older \
                     than the doctor subcommand predates it; update the service package \
                     to restore per-service diagnosis",
                    binary.display(),
                    output
                        .status
                        .code()
                        .map_or_else(|| "?".to_string(), |c| c.to_string()),
                    if trail.is_empty() { "" } else { "; stderr: " },
                    trail,
                ),
                None,
            )]
        }
    }
}

/// The merge itself: the child's checks join the aggregate report scoped to
/// the emitting service (the child self-scopes at the report level, not per
/// check). Statuses — including `Unknown` from a newer binary — carry over
/// untouched, so the child's failures fail the aggregate exit code.
fn merge_child_checks(child: Report, service: &str) -> Vec<Check> {
    child
        .checks
        .into_iter()
        .map(|mut check| {
            check.service.get_or_insert_with(|| service.to_string());
            check
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::report::Status;

    #[test]
    fn test_merge_scopes_unscoped_child_checks_to_the_service() {
        let child: Report = serde_json::from_str(
            r#"{
                "mode": "service",
                "service": "ppba-driver",
                "checks": [
                    { "name": "config.full-shape", "status": "fail", "detail": "unknown key" },
                    { "name": "already.scoped", "service": "other", "status": "ok" }
                ]
            }"#,
        )
        .unwrap();
        let merged = merge_child_checks(child, "ppba-driver");
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].service.as_deref(), Some("ppba-driver"));
        assert_eq!(merged[0].status, Status::Fail);
        assert_eq!(
            merged[1].service.as_deref(),
            Some("other"),
            "a check the child already scoped keeps its scope"
        );
    }

    #[test]
    fn test_merge_preserves_unknown_statuses_from_newer_binaries() {
        let child: Report = serde_json::from_str(
            r#"{ "checks": [ { "name": "novel.check", "status": "degraded" } ] }"#,
        )
        .unwrap();
        let merged = merge_child_checks(child, "rp");
        assert_eq!(merged[0].status, Status::Unknown);
    }

    #[test]
    fn test_describe_devices_lists_type_name_and_number() {
        assert_eq!(
            describe_devices(&[]),
            "the service reports 0 configured devices"
        );
        let devices = vec![
            ConfiguredDevice {
                name: "QHY178M".to_string(),
                device_type: "Camera".to_string(),
                number: 0,
            },
            ConfiguredDevice {
                name: "EAF".to_string(),
                device_type: "Focuser".to_string(),
                number: 1,
            },
        ];
        let text = describe_devices(&devices);
        assert!(
            text.contains("2 configured device(s)")
                && text.contains("Camera \"QHY178M\" (#0)")
                && text.contains("Focuser \"EAF\" (#1)"),
            "{text}"
        );
    }

    #[test]
    fn test_management_response_parses_permissively() {
        let m: ManagementResponse = serde_json::from_str(
            r#"{ "Value": [ { "DeviceName": "x", "DeviceType": "Camera",
                              "DeviceNumber": 0, "UniqueID": "u" } ],
                 "ClientTransactionID": 7, "ServerTransactionID": 9 }"#,
        )
        .unwrap();
        assert_eq!(m.value.len(), 1);
        let empty: ManagementResponse = serde_json::from_str("{}").unwrap();
        assert!(empty.value.is_empty());
    }
}
