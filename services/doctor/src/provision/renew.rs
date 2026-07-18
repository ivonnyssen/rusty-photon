//! One-shot certificate renewal (docs/services/doctor.md §Renewal).
//!
//! `doctor tls renew` is what a platform scheduler runs daily: the
//! self-signed leg re-issues, from the existing CA, every service pair in
//! the pki tree inside its 30-day window; the ACME leg — only when
//! `<config-root>/acme.json` exists — replays the persisted settings
//! through a fresh order when the wildcard pair is missing or inside
//! `renewal_days_before_expiry`. Running services pick a renewed pair up
//! in-process through `rusty-photon-tls`'s reloading resolver.

use std::path::Path;
use std::time::Duration;

use tracing::debug;

use super::{acme, acme_config, cert, dns, expiry};
use crate::report::{AppliedFix, FixOp};

/// Self-signed material renews inside this window. The ACME window is
/// `renewal_days_before_expiry` from `acme.json`.
const SELF_SIGNED_RENEWAL_WINDOW_DAYS: i64 = 30;

/// A failed order is retried with fresh orders up to this many attempts —
/// a failed DNS-01 authorization is dead, so re-polling the old order
/// would never recover.
const ACME_ORDER_ATTEMPTS: u32 = 3;

/// Backoff between order attempts.
const ACME_RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// A renewal failure, carrying whatever was renewed before the failure so
/// the caller still reports it — a hook failing after a successful order
/// must not hide that the pair on disk is new.
#[derive(Debug)]
pub struct RenewError {
    pub message: String,
    pub applied: Vec<AppliedFix>,
    pub warnings: Vec<String>,
}

/// Run both renewal legs against the resolved config root. Returns the
/// provisioning actions performed (empty = nothing was due) plus operator
/// warnings (a CA inside its window). `force` ignores the windows and
/// renews everything both legs own — never the CA.
pub async fn renew(
    config_dir: &Path,
    force: bool,
) -> Result<(Vec<AppliedFix>, Vec<String>), RenewError> {
    let mut applied = Vec::new();
    let mut warnings = Vec::new();

    renew_self_signed(config_dir, force, &mut applied, &mut warnings).map_err(|message| {
        RenewError {
            message,
            applied: applied.clone(),
            warnings: warnings.clone(),
        }
    })?;
    renew_acme(config_dir, force, &mut applied)
        .await
        .map_err(|message| RenewError {
            message,
            applied: applied.clone(),
            warnings: warnings.clone(),
        })?;

    Ok((applied, warnings))
}

/// Whether a certificate with this `not_after` is due inside `window_days`.
fn due_within(not_after: time::OffsetDateTime, window_days: i64) -> bool {
    not_after - time::OffsetDateTime::now_utc() <= time::Duration::days(window_days)
}

/// The self-signed leg: re-issue every due `<svc>.pem`/`<svc>-key.pem`
/// pair from the existing CA, preserving the old certificate's DNS SANs
/// (unioned with the hostname defaults by `cert::generate_service_cert`).
/// The CA is never re-issued — one inside its window only earns a warning.
fn renew_self_signed(
    config_dir: &Path,
    force: bool,
    applied: &mut Vec<AppliedFix>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let pki = super::pki_dir(config_dir);
    if !pki.is_dir() {
        debug!("no pki tree; nothing self-signed to renew");
        return Ok(());
    }
    let ca_cert_path = rusty_photon_tls::config::ca_cert_path(&pki);
    if let Ok(ca_pem) = std::fs::read_to_string(&ca_cert_path) {
        if let Ok(not_after) = expiry::not_after(&ca_pem) {
            if due_within(not_after, SELF_SIGNED_RENEWAL_WINDOW_DAYS) {
                warnings.push(format!(
                    "{} expires {not_after} and is never auto-renewed — replacing \
                     the CA invalidates every distributed trust anchor, so delete \
                     ca.pem, re-run `doctor tls issue`, and redistribute the new \
                     anchor deliberately",
                    ca_cert_path.display()
                ));
            }
        }
    }

    let mut due: Vec<(String, Vec<String>)> = Vec::new();
    for entry in std::fs::read_dir(&pki)
        .map_err(|e| format!("could not read {}: {e}", pki.display()))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(service) = name.strip_suffix(".pem") else {
            continue;
        };
        if service.ends_with("-key") || name == "ca.pem" || name == "acme-cert.pem" {
            continue;
        }
        if !pki.join(format!("{service}-key.pem")).is_file() {
            debug!(
                service,
                "certificate without a key file; not a renewable pair"
            );
            continue;
        }
        let Ok(pem) = std::fs::read_to_string(entry.path()) else {
            debug!(service, "certificate unreadable; treating the pair as due");
            due.push((service.to_string(), Vec::new()));
            continue;
        };
        match expiry::not_after(&pem) {
            Ok(not_after) if !force && !due_within(not_after, SELF_SIGNED_RENEWAL_WINDOW_DAYS) => {
                debug!(service, %not_after, "outside the renewal window");
            }
            Ok(_) => due.push((service.to_string(), expiry::dns_sans(&pem))),
            Err(e) => {
                // An unparseable pair is exactly what `tls.expiry` sends
                // operators here to fix.
                debug!(service, "certificate unparseable ({e}); re-issuing");
                due.push((service.to_string(), Vec::new()));
            }
        }
    }
    if due.is_empty() {
        debug!("no self-signed pair is due");
        return Ok(());
    }

    let ca_key_path = rusty_photon_tls::config::ca_key_path(&pki);
    let ca_cert_pem = std::fs::read_to_string(&ca_cert_path).map_err(|e| {
        format!(
            "cannot renew: could not read {}: {e}",
            ca_cert_path.display()
        )
    })?;
    let ca_key_pem = std::fs::read_to_string(&ca_key_path).map_err(|e| {
        format!(
            "cannot renew: could not read {}: {e}",
            ca_key_path.display()
        )
    })?;

    for (service, extra_sans) in due {
        cert::generate_service_cert(&ca_cert_pem, &ca_key_pem, &service, &extra_sans, &pki)
            .map_err(|e| format!("could not re-issue the certificate for {service}: {e}"))?;
        debug!(service, "re-issued from the existing CA");
        applied.push(AppliedFix {
            check: "provisioning".to_string(),
            op: FixOp::GenerateCert { service },
        });
    }
    Ok(())
}

/// The ACME leg: when `acme.json` exists and the wildcard pair is missing
/// or due, replay the persisted settings through a fresh order (this is
/// also the recovery path for a first order that failed after `tls issue
/// --acme` persisted the settings), then run the post-renewal hooks.
async fn renew_acme(
    config_dir: &Path,
    force: bool,
    applied: &mut Vec<AppliedFix>,
) -> Result<(), String> {
    let config_path = config_dir.join("acme.json");
    if !config_path.is_file() {
        debug!("no acme.json; skipping the ACME leg");
        return Ok(());
    }
    let config = acme_config::load_acme_config(&config_path)
        .map_err(|e| format!("could not load {}: {e}", config_path.display()))?;
    let pki = super::pki_dir(config_dir);
    let cert_path = acme_config::acme_cert_path(&pki);

    let due = force
        || match std::fs::read_to_string(&cert_path) {
            Err(_) => {
                debug!(cert = %cert_path.display(), "wildcard certificate missing; due");
                true
            }
            Ok(pem) => match expiry::not_after(&pem) {
                Ok(not_after) => {
                    due_within(not_after, i64::from(config.renewal_days_before_expiry))
                }
                Err(e) => {
                    debug!("wildcard certificate unparseable ({e}); due");
                    true
                }
            },
        };
    if !due {
        debug!("the ACME wildcard pair is outside its renewal window");
        return Ok(());
    }

    let resolved = acme_config::resolve_credentials(&config.dns_credentials)
        .map_err(|e| format!("could not resolve DNS credentials: {e}"))?;
    let dns_provider = dns::build_dns_provider(&config.dns_provider, &resolved, &config.domain)
        .await
        .map_err(|e| e.to_string())?;
    let client = acme::RealAcmeClient::new(
        dns_provider.as_ref(),
        config.acme_root.as_ref().map(std::path::PathBuf::from),
        Duration::from_secs(config.dns_propagation_seconds),
    );

    order_with_retry(&config, &pki, &client).await?;
    applied.push(AppliedFix {
        check: "provisioning".to_string(),
        op: FixOp::RenewAcme {
            domain: config.domain.clone(),
        },
    });

    run_hooks(&config.post_renewal_hooks)
}

/// Drive `issue_certificate` with fresh orders until one succeeds, up to
/// [`ACME_ORDER_ATTEMPTS`] attempts with a short backoff between them.
async fn order_with_retry(
    config: &acme_config::AcmeConfig,
    pki: &Path,
    client: &dyn acme::AcmeClient,
) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 1..=ACME_ORDER_ATTEMPTS {
        match acme::issue_certificate(config, pki, client).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                debug!(attempt, "ACME order failed: {e}");
                last_error = e.to_string();
                if attempt < ACME_ORDER_ATTEMPTS {
                    tokio::time::sleep(ACME_RETRY_BACKOFF).await;
                }
            }
        }
    }
    Err(format!(
        "the ACME order failed {ACME_ORDER_ATTEMPTS} times; last error: {last_error}"
    ))
}

/// Run every post-renewal hook in order, even after one fails — a skipped
/// hook is a remote machine keeping its old certificate. Any failure is an
/// overall error (exit 2) naming the hook. Hook output is captured, never
/// inherited: doctor's stdout is reserved for its own report (`--json`
/// consumers parse it), so a chatty hook must not write through to it.
fn run_hooks(hooks: &[String]) -> Result<(), String> {
    let mut failed: Vec<String> = Vec::new();
    for hook in hooks {
        debug!(hook, "running post-renewal hook");
        let output = shell_command(hook).output();
        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                debug!(hook, output = %stdout.trim(), "post-renewal hook succeeded");
            }
            Ok(output) => {
                let status = output.status;
                let stderr = String::from_utf8_lossy(&output.stderr);
                let snippet = stderr.trim().chars().take(200).collect::<String>();
                debug!(hook, %status, stderr = %snippet, "post-renewal hook failed");
                if snippet.is_empty() {
                    failed.push(format!("`{hook}` ({status})"));
                } else {
                    failed.push(format!("`{hook}` ({status}: {snippet})"));
                }
            }
            Err(e) => {
                debug!(hook, "post-renewal hook could not run: {e}");
                failed.push(format!("`{hook}` (could not run: {e})"));
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "post-renewal hook failure — a silently-failed hook means a remote \
             machine keeps its old certificate: {}",
            failed.join(", ")
        ))
    }
}

#[cfg(unix)]
fn shell_command(hook: &str) -> std::process::Command {
    let mut command = std::process::Command::new("sh");
    command.arg("-c").arg(hook);
    command
}

#[cfg(windows)]
fn shell_command(hook: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new("cmd");
    // raw_arg: std's argument quoting wraps the hook in escaped quotes,
    // which cmd.exe does not unescape — a hook with a quoted path (or any
    // redirect) reaches cmd mangled and silently does nothing. cmd wants
    // the line verbatim after /C.
    command.arg("/C").raw_arg(hook);
    command
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    /// Sign a service pair from the staged CA with an arbitrary window.
    fn stage_pair(pki: &Path, service: &str, not_after: time::OffsetDateTime, sans: &[&str]) {
        let ca_cert = std::fs::read_to_string(pki.join("ca.pem")).unwrap();
        let ca_key_pem = std::fs::read_to_string(pki.join("ca-key.pem")).unwrap();
        let ca_key = rcgen::KeyPair::from_pem(&ca_key_pem).unwrap();
        let issuer = rcgen::Issuer::from_ca_cert_pem(&ca_cert, &ca_key).unwrap();
        let mut params =
            rcgen::CertificateParams::new(sans.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                .unwrap();
        params.not_before = not_after - time::Duration::days(365);
        params.not_after = not_after;
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.signed_by(&key, &issuer).unwrap();
        std::fs::write(pki.join(format!("{service}.pem")), cert.pem()).unwrap();
        std::fs::write(pki.join(format!("{service}-key.pem")), key.serialize_pem()).unwrap();
    }

    fn stage_tree() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let pki = super::super::pki_dir(dir.path());
        cert::generate_ca(&pki).unwrap();
        (dir, pki)
    }

    #[tokio::test]
    async fn test_renew_is_a_no_op_on_a_healthy_tree() {
        let (dir, pki) = stage_tree();
        let healthy = time::OffsetDateTime::now_utc() + time::Duration::days(300);
        stage_pair(&pki, "qhy-focuser", healthy, &["localhost"]);
        let before = std::fs::read(pki.join("qhy-focuser.pem")).unwrap();

        let (applied, warnings) = renew(dir.path(), false).await.unwrap();
        assert!(applied.is_empty(), "{applied:?}");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(std::fs::read(pki.join("qhy-focuser.pem")).unwrap(), before);
    }

    #[tokio::test]
    async fn test_renew_reissues_a_due_pair_preserving_sans() {
        let (dir, pki) = stage_tree();
        let due = time::OffsetDateTime::now_utc() + time::Duration::days(5);
        stage_pair(
            &pki,
            "qhy-focuser",
            due,
            &["localhost", "observatory.local"],
        );

        let (applied, _) = renew(dir.path(), false).await.unwrap();
        assert_eq!(applied.len(), 1);
        assert!(
            matches!(&applied[0].op, FixOp::GenerateCert { service } if service == "qhy-focuser")
        );
        let renewed = std::fs::read_to_string(pki.join("qhy-focuser.pem")).unwrap();
        let not_after = expiry::not_after(&renewed).unwrap();
        assert!(
            !due_within(not_after, SELF_SIGNED_RENEWAL_WINDOW_DAYS),
            "the re-issued pair must leave the window"
        );
        let sans = expiry::dns_sans(&renewed);
        assert!(sans.contains(&"observatory.local".to_string()), "{sans:?}");
        assert!(sans.contains(&"localhost".to_string()), "{sans:?}");
    }

    #[tokio::test]
    async fn test_renew_warns_about_a_due_ca_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let pki = super::super::pki_dir(dir.path());
        std::fs::create_dir_all(&pki).unwrap();
        // A CA expiring inside the window, built directly.
        let mut params = rcgen::CertificateParams::default();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(3640);
        params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(10);
        let key = rcgen::KeyPair::generate().unwrap();
        let ca = params.self_signed(&key).unwrap();
        std::fs::write(pki.join("ca.pem"), ca.pem()).unwrap();
        std::fs::write(pki.join("ca-key.pem"), key.serialize_pem()).unwrap();
        let ca_before = std::fs::read(pki.join("ca.pem")).unwrap();

        let (applied, warnings) = renew(dir.path(), false).await.unwrap();
        assert!(applied.is_empty(), "{applied:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("ca.pem"), "{warnings:?}");
        assert_eq!(std::fs::read(pki.join("ca.pem")).unwrap(), ca_before);
    }

    #[tokio::test]
    async fn test_renew_fails_naming_the_missing_ca_key() {
        let (dir, pki) = stage_tree();
        let due = time::OffsetDateTime::now_utc() + time::Duration::days(5);
        stage_pair(&pki, "qhy-focuser", due, &["localhost"]);
        std::fs::remove_file(pki.join("ca-key.pem")).unwrap();

        let err = renew(dir.path(), false).await.unwrap_err();
        assert!(err.message.contains("ca-key.pem"), "{}", err.message);
    }

    #[tokio::test]
    async fn test_force_reissues_a_healthy_pair_but_never_the_ca() {
        let (dir, pki) = stage_tree();
        let healthy = time::OffsetDateTime::now_utc() + time::Duration::days(300);
        stage_pair(&pki, "ppba-driver", healthy, &["localhost"]);
        let cert_before = std::fs::read(pki.join("ppba-driver.pem")).unwrap();
        let ca_before = std::fs::read(pki.join("ca.pem")).unwrap();

        let (applied, _) = renew(dir.path(), true).await.unwrap();
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert_ne!(
            std::fs::read(pki.join("ppba-driver.pem")).unwrap(),
            cert_before
        );
        assert_eq!(std::fs::read(pki.join("ca.pem")).unwrap(), ca_before);
    }

    #[test]
    fn test_run_hooks_runs_all_and_names_the_failure() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let write_marker = if cfg!(windows) {
            format!("echo ran > \"{}\"", marker.display())
        } else {
            format!("echo ran > '{}'", marker.display())
        };
        let hooks = vec!["echo scp-failed >&2 && exit 7".to_string(), write_marker];
        let err = run_hooks(&hooks).unwrap_err();
        assert!(err.contains("exit 7"), "{err}");
        assert!(
            err.contains("scp-failed"),
            "the failure must carry the hook's stderr: {err}"
        );
        assert!(
            marker.is_file(),
            "the hook after the failing one must still run"
        );
    }

    #[test]
    fn test_run_hooks_succeeds_when_all_pass() {
        run_hooks(&["exit 0".to_string()]).unwrap();
        run_hooks(&[]).unwrap();
    }

    fn acme_test_config() -> acme_config::AcmeConfig {
        serde_json::from_value(serde_json::json!({
            "email": "ops@example.com",
            "domain": "observatory.test",
            "dns_provider": "cloudflare",
            "dns_credentials": { "api_token": "tok" },
        }))
        .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn test_order_retry_recovers_from_transient_failures() {
        let dir = tempfile::tempdir().unwrap();
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let seen = attempts.clone();
        let mut mock = acme::MockAcmeClient::new();
        mock.expect_create_or_load_account()
            .returning(|_, _, _| Ok(None));
        mock.expect_order_certificate().returning(move |_| {
            // Each attempt is a fresh order; the first two die.
            if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                Err(rusty_photon_tls::error::TlsError::Acme(
                    "authorization dead".to_string(),
                ))
            } else {
                Ok(("CERT".to_string(), "KEY".to_string()))
            }
        });

        order_with_retry(&acme_test_config(), dir.path(), &mock)
            .await
            .unwrap();
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert!(dir.path().join("acme-cert.pem").is_file());
    }

    #[tokio::test(start_paused = true)]
    async fn test_order_retry_gives_up_after_three_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let seen = attempts.clone();
        let mut mock = acme::MockAcmeClient::new();
        mock.expect_create_or_load_account()
            .returning(|_, _, _| Ok(None));
        mock.expect_order_certificate().returning(move |_| {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(rusty_photon_tls::error::TlsError::Acme(
                "authorization dead".to_string(),
            ))
        });

        let err = order_with_retry(&acme_test_config(), dir.path(), &mock)
            .await
            .unwrap_err();
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert!(err.contains("authorization dead"), "{err}");
    }
}
