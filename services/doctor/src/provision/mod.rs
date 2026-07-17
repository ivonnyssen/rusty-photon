//! TLS + credential provisioning (docs/services/doctor.md §Provisioning).
//!
//! Everything that *mints* material lives here: the self-signed CA and
//! per-service certificates, the ACME (DNS-01) path, and the observatory
//! credential. The serving half every service links is the
//! `rusty-photon-tls` crate; doctor is the only binary that writes the pki
//! tree. All material anchors at `<config-root>/pki` (flat — no `certs/`
//! subdirectory), with `acme.json` beside the service configs.

pub mod acme;
pub mod acme_config;
pub mod cert;
pub mod dns;

use std::path::{Path, PathBuf};

use rand::distr::{Alphanumeric, SampleString};
use rusty_photon_tls::permissions::set_restricted_permissions;
use serde_json::json;
use tracing::debug;

use crate::report::{AppliedFix, FixOp};

/// The one observatory username (ADR-016 decision 10(e)).
pub const CREDENTIAL_USERNAME: &str = "observatory";

/// 32 alphanumeric characters ≈ 190 bits of entropy — comfortably past the
/// ≥128-bit floor the design demands.
const CREDENTIAL_LENGTH: usize = 32;

/// The pki tree under the resolved config root.
pub fn pki_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("pki")
}

/// The canonical plaintext credential copy.
pub fn credential_path(config_dir: &Path) -> PathBuf {
    pki_dir(config_dir).join("credential")
}

fn service_cert_path(pki: &Path, service: &str) -> PathBuf {
    pki.join(format!("{service}.pem"))
}

fn service_key_path(pki: &Path, service: &str) -> PathBuf {
    pki.join(format!("{service}-key.pem"))
}

/// The `server.tls` block value pointing a service at its issued pair.
pub fn tls_block_value(config_dir: &Path, service: &str) -> serde_json::Value {
    let pki = absolute_pki_dir(config_dir);
    json!({
        "cert": service_cert_path(&pki, service).to_string_lossy(),
        "key": service_key_path(&pki, service).to_string_lossy(),
    })
}

/// The pki dir as an absolute path, so config-written paths stay valid
/// whatever directory a service later starts from.
pub fn absolute_pki_dir(config_dir: &Path) -> PathBuf {
    std::path::absolute(pki_dir(config_dir)).unwrap_or_else(|_| pki_dir(config_dir))
}

/// Create the CA if absent and issue a certificate pair for every listed
/// service whose pair is missing. `force` re-issues service certificates
/// from the existing CA — never the CA itself: replacing it invalidates
/// every distributed trust anchor, so that is an explicit operator act
/// (delete `ca.pem`, re-run). Returns the provisioning actions performed.
pub fn ensure_material(
    config_dir: &Path,
    services: &[String],
    extra_sans: &[String],
    force: bool,
) -> Result<Vec<AppliedFix>, String> {
    let pki = pki_dir(config_dir);
    let ca_cert = rusty_photon_tls::config::ca_cert_path(&pki);
    let ca_key = rusty_photon_tls::config::ca_key_path(&pki);
    let mut applied = Vec::new();

    if ca_cert.exists() && ca_key.exists() {
        debug!(ca = %ca_cert.display(), "CA exists; never regenerated");
    } else {
        cert::generate_ca(&pki).map_err(|e| format!("could not generate the CA: {e}"))?;
        applied.push(AppliedFix {
            check: "provisioning".to_string(),
            op: FixOp::GenerateCa,
        });
    }

    if services.is_empty() {
        return Ok(applied);
    }
    let ca_cert_pem = std::fs::read_to_string(&ca_cert)
        .map_err(|e| format!("could not read {}: {e}", ca_cert.display()))?;
    let ca_key_pem = std::fs::read_to_string(&ca_key)
        .map_err(|e| format!("could not read {}: {e}", ca_key.display()))?;

    for service in services {
        let cert_path = service_cert_path(&pki, service);
        let key_path = service_key_path(&pki, service);
        if !force && cert_path.is_file() && key_path.is_file() {
            debug!(service, "certificate pair exists; skipping");
            continue;
        }
        cert::generate_service_cert(&ca_cert_pem, &ca_key_pem, service, extra_sans, &pki)
            .map_err(|e| format!("could not generate a certificate for {service}: {e}"))?;
        applied.push(AppliedFix {
            check: "provisioning".to_string(),
            op: FixOp::GenerateCert {
                service: service.clone(),
            },
        });
    }
    Ok(applied)
}

/// The credential plaintext from the canonical pki copy, when present.
pub fn read_credential(config_dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(credential_path(config_dir)).ok()?;
    let trimmed = content.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Reuse `pki/credential` if present, else mint and write it — a service
/// installed after the first `--fix` run is wired with the *same*
/// credential on the next run.
pub fn ensure_credential(config_dir: &Path) -> Result<(String, Vec<AppliedFix>), String> {
    if let Some(existing) = read_credential(config_dir) {
        debug!("reusing the existing observatory credential");
        return Ok((existing, Vec::new()));
    }
    let password = mint_credential(config_dir)?;
    Ok((
        password,
        vec![AppliedFix {
            check: "provisioning".to_string(),
            op: FixOp::MintCredential,
        }],
    ))
}

/// Mint a fresh credential and (over)write the canonical 0600 copy —
/// `doctor auth rotate`'s first step, and the mint leg of
/// [`ensure_credential`].
pub fn mint_credential(config_dir: &Path) -> Result<String, String> {
    let password = Alphanumeric.sample_string(&mut rand::rng(), CREDENTIAL_LENGTH);
    let path = credential_path(config_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, format!("{password}\n"))
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    set_restricted_permissions(&path)
        .map_err(|e| format!("could not restrict {}: {e}", path.display()))?;
    debug!(path = %path.display(), "wrote the observatory credential");
    Ok(password)
}

/// The client-block wiring `--fix` distributes into sentinel.json once the
/// material exists: the plaintext credential into an absent `service_auth`,
/// the CA path into an absent `ca_cert`. Present (non-null) blocks are
/// operator intent and get no op. Empty when sentinel has no usable config
/// or the material is not there to point at.
pub fn plan_client_wiring(config_dir: &Path) -> Vec<(String, FixOp)> {
    let path = config_dir.join("sentinel.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        debug!(path = %path.display(), "sentinel.json is not valid JSON; no client wiring");
        return Vec::new();
    };
    let mut ops = Vec::new();
    if value
        .get("service_auth")
        .is_none_or(serde_json::Value::is_null)
    {
        if let Some(password) = read_credential(config_dir) {
            ops.push((
                "auth.absent".to_string(),
                FixOp::SetObject {
                    service: "sentinel".to_string(),
                    pointer: "/service_auth".to_string(),
                    value: json!({ "username": CREDENTIAL_USERNAME, "password": password }),
                },
            ));
        }
    }
    if value.get("ca_cert").is_none_or(serde_json::Value::is_null) {
        let ca = rusty_photon_tls::config::ca_cert_path(&absolute_pki_dir(config_dir));
        if ca.is_file() {
            ops.push((
                "tls.absent".to_string(),
                FixOp::SetString {
                    service: "sentinel".to_string(),
                    pointer: "/ca_cert".to_string(),
                    value: ca.to_string_lossy().into_owned(),
                },
            ));
        }
    }
    ops
}

/// Run the ACME issuance flow: persist `acme.json` beside the configs
/// **first** (that is the contract renewal picks up from, whether or not
/// the order succeeds), then build the DNS provider and order a wildcard
/// certificate into the flat pki tree.
#[allow(clippy::too_many_arguments)]
pub async fn run_acme(
    config_dir: &Path,
    domain: &str,
    dns_provider_name: &str,
    dns_token: &str,
    email: &str,
    staging: bool,
) -> Result<(), String> {
    let pki = pki_dir(config_dir);

    let mut dns_credentials = std::collections::HashMap::new();
    dns_credentials.insert("api_token".to_string(), dns_token.to_string());
    let config = acme_config::AcmeConfig {
        email: email.to_string(),
        domain: domain.to_string(),
        dns_provider: dns_provider_name.to_string(),
        dns_credentials,
        staging,
        renewal_days_before_expiry: 30,
        post_renewal_hooks: vec![],
    };

    let config_path = config_dir.join("acme.json");
    acme_config::save_acme_config(&config, &config_path)
        .map_err(|e| format!("could not save {}: {e}", config_path.display()))?;
    debug!(path = %config_path.display(), "saved the ACME configuration");

    let resolved =
        acme_config::resolve_credentials(&config.dns_credentials).map_err(|e| e.to_string())?;
    let dns_provider = dns::build_dns_provider(&config.dns_provider, &resolved, domain)
        .await
        .map_err(|e| e.to_string())?;
    let acme_client = acme::RealAcmeClient::new(dns_provider.as_ref());

    acme::issue_certificate(&config, &pki, &acme_client)
        .await
        .map_err(|e| e.to_string())?;

    println!("ACME certificate issued for *.{domain}:");
    println!("  cert: {}", acme_config::acme_cert_path(&pki).display());
    println!("  key:  {}", acme_config::acme_key_path(&pki).display());
    if staging {
        println!("  environment: STAGING (not trusted by browsers)");
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    fn services(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_ensure_material_creates_ca_and_service_pairs_flat() {
        let dir = tempfile::tempdir().unwrap();
        let applied = ensure_material(dir.path(), &services(&["ppba-driver"]), &[], false).unwrap();
        let ops: Vec<String> = applied.iter().map(|a| a.op.to_string()).collect();
        assert_eq!(applied.len(), 2, "{ops:?}");
        assert!(matches!(applied[0].op, FixOp::GenerateCa));
        assert!(
            matches!(&applied[1].op, FixOp::GenerateCert { service } if service == "ppba-driver")
        );
        let pki = pki_dir(dir.path());
        for name in [
            "ca.pem",
            "ca-key.pem",
            "ppba-driver.pem",
            "ppba-driver-key.pem",
        ] {
            assert!(pki.join(name).is_file(), "missing {name}");
        }
        assert!(
            !pki.join("certs").exists(),
            "the pki tree is flat — no certs/ subdirectory"
        );
    }

    #[test]
    fn test_ensure_material_is_idempotent_and_force_reissues_certs_only() {
        let dir = tempfile::tempdir().unwrap();
        ensure_material(dir.path(), &services(&["dsd-fp2"]), &[], false).unwrap();
        let pki = pki_dir(dir.path());
        let ca_before = std::fs::read(pki.join("ca.pem")).unwrap();
        let cert_before = std::fs::read(pki.join("dsd-fp2.pem")).unwrap();

        let applied = ensure_material(dir.path(), &services(&["dsd-fp2"]), &[], false).unwrap();
        assert!(applied.is_empty(), "second run generates nothing");
        assert_eq!(std::fs::read(pki.join("dsd-fp2.pem")).unwrap(), cert_before);

        let applied = ensure_material(dir.path(), &services(&["dsd-fp2"]), &[], true).unwrap();
        assert_eq!(applied.len(), 1, "--force re-issues the service cert");
        assert!(matches!(&applied[0].op, FixOp::GenerateCert { .. }));
        assert_ne!(std::fs::read(pki.join("dsd-fp2.pem")).unwrap(), cert_before);
        assert_eq!(
            std::fs::read(pki.join("ca.pem")).unwrap(),
            ca_before,
            "--force never touches the CA"
        );
    }

    #[test]
    fn test_ensure_credential_mints_once_and_reuses() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_credential(dir.path()).is_none());
        let (password, applied) = ensure_credential(dir.path()).unwrap();
        assert_eq!(password.len(), CREDENTIAL_LENGTH);
        assert!(password.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(applied.len(), 1);
        assert!(matches!(applied[0].op, FixOp::MintCredential));

        let (again, applied) = ensure_credential(dir.path()).unwrap();
        assert_eq!(again, password, "the canonical copy is reused");
        assert!(applied.is_empty());
        assert_eq!(read_credential(dir.path()).unwrap(), password);
    }

    #[cfg(unix)]
    #[test]
    fn test_credential_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        ensure_credential(dir.path()).unwrap();
        let mode = std::fs::metadata(credential_path(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "credential mode {mode:o}");
    }

    #[test]
    fn test_mint_credential_overwrites_for_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let first = mint_credential(dir.path()).unwrap();
        let second = mint_credential(dir.path()).unwrap();
        assert_ne!(first, second);
        assert_eq!(read_credential(dir.path()).unwrap(), second);
    }

    #[test]
    fn test_plan_client_wiring_wires_absent_blocks_only() {
        let dir = tempfile::tempdir().unwrap();
        // No sentinel.json: nothing to wire.
        assert!(plan_client_wiring(dir.path()).is_empty());

        std::fs::write(
            dir.path().join("sentinel.json"),
            r#"{ "server": { "port": 11114 }, "ca_cert": null }"#,
        )
        .unwrap();
        // Material absent: nothing to point at yet.
        assert!(plan_client_wiring(dir.path()).is_empty());

        ensure_material(dir.path(), &[], &[], false).unwrap();
        ensure_credential(dir.path()).unwrap();
        let ops = plan_client_wiring(dir.path());
        assert_eq!(ops.len(), 2, "{ops:?}");
        assert!(matches!(
            &ops[0].1,
            FixOp::SetObject { service, pointer, .. }
                if service == "sentinel" && pointer == "/service_auth"
        ));
        assert!(matches!(
            &ops[1].1,
            FixOp::SetString { service, pointer, .. }
                if service == "sentinel" && pointer == "/ca_cert"
        ));

        // Present blocks are never re-planned.
        std::fs::write(
            dir.path().join("sentinel.json"),
            r#"{ "service_auth": { "username": "u", "password": "p" }, "ca_cert": "/x/ca.pem" }"#,
        )
        .unwrap();
        assert!(plan_client_wiring(dir.path()).is_empty());
    }

    #[test]
    fn test_tls_block_value_points_at_the_flat_pki_pair() {
        let dir = tempfile::tempdir().unwrap();
        let value = tls_block_value(dir.path(), "qhy-focuser");
        let cert = value["cert"].as_str().unwrap();
        let key = value["key"].as_str().unwrap();
        assert!(cert.ends_with("qhy-focuser.pem"), "{cert}");
        assert!(key.ends_with("qhy-focuser-key.pem"), "{key}");
        assert!(std::path::Path::new(cert).is_absolute());
        assert!(!cert.contains("certs"), "flat pki: {cert}");
    }
}
