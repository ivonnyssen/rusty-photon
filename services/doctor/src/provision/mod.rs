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
pub mod expiry;
pub mod renew;

use std::path::{Path, PathBuf};

use rand::distr::{Alphanumeric, SampleString};
use rusty_photon_tls::permissions::write_restricted;
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

/// Align the pki tree (and `acme.json` beside the configs) with the config
/// root's owner.
///
/// Provisioning as root on a packaged host (`sudo rusty-photon-doctor
/// --fix`) creates key material root-owned; the services — and the renewal
/// timer, which runs as the service user — could then neither read nor
/// renew it. A fresh file has no original whose owner
/// `rusty_photon_config::save` could preserve, so the tree is aligned
/// wholesale: every entry whose owner differs from the config root's is
/// chowned to match. For an unprivileged caller on its own tree every
/// owner already matches and this is a no-op. Symlinks are skipped (doctor
/// never creates one there; following it would chown the target). A failed
/// chown is an error: a silently root-owned key breaks TLS at the next
/// service start.
#[cfg(unix)]
pub fn align_pki_ownership(config_dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let Ok(root_meta) = std::fs::metadata(config_dir) else {
        return Ok(());
    };
    let (uid, gid) = (root_meta.uid(), root_meta.gid());
    let mut paths = vec![config_dir.join("acme.json")];
    let pki = pki_dir(config_dir);
    if let Ok(entries) = std::fs::read_dir(&pki) {
        paths.push(pki);
        paths.extend(entries.flatten().map(|e| e.path()));
    }
    for path in paths {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || (meta.uid() == uid && meta.gid() == gid) {
            continue;
        }
        std::os::unix::fs::chown(&path, Some(uid), Some(gid)).map_err(|e| {
            format!(
                "could not chown {} to the config root's owner (uid {uid}, gid {gid}): {e} — \
                 the services and the renewal timer run as that user and need this material",
                path.display()
            )
        })?;
        debug!(path = %path.display(), uid, gid, "aligned ownership with the config root");
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn align_pki_ownership(_config_dir: &Path) -> Result<(), String> {
    Ok(())
}

/// Create the CA if absent and issue a certificate pair for every listed
/// service whose pair is missing. `force` re-issues service certificates
/// from the existing CA — never the CA itself: replacing it invalidates
/// every distributed trust anchor, so that is an explicit operator act
/// (delete `ca.pem` and `ca-key.pem`, re-run with `--force` so every
/// service pair chains to the new CA — without it existing pairs are
/// kept and still chain to the old one). Returns the provisioning
/// actions performed.
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
        align_pki_ownership(config_dir)?;
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
    align_pki_ownership(config_dir)?;
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
    write_restricted(&path, format!("{password}\n").as_bytes())
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    debug!(path = %path.display(), "wrote the observatory credential");
    align_pki_ownership(config_dir)?;
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

/// Everything `doctor tls issue --acme` collects from its flags. All of it
/// persists into `acme.json` — renewal must replay these settings
/// unattended.
#[derive(Debug, Clone)]
pub struct AcmeArgs {
    pub domain: String,
    pub dns_provider: String,
    pub dns_token: String,
    pub email: String,
    pub staging: bool,
    /// Overrides the Let's Encrypt endpoints entirely (an internal ACME CA,
    /// or Pebble in tests).
    pub directory_url: Option<String>,
    /// A PEM trust anchor for the ACME server's own TLS endpoint.
    pub acme_root: Option<PathBuf>,
    /// Wait between writing the TXT record and requesting validation;
    /// `None` keeps the 15s default.
    pub dns_propagation_seconds: Option<u64>,
}

/// Run the ACME issuance flow: persist `acme.json` beside the configs
/// **first** (that is the contract renewal picks up from, whether or not
/// the order succeeds), then build the DNS provider and order a wildcard
/// certificate into the flat pki tree.
pub async fn run_acme(config_dir: &Path, args: AcmeArgs) -> Result<(), String> {
    let pki = pki_dir(config_dir);

    // Persisted absolute: renewal replays acme.json from a scheduler whose
    // working directory is arbitrary, so a relative --acme-root (anchored
    // at the invoking shell's cwd, like any CLI path) must be resolved
    // now, not at 3am.
    let acme_root = args
        .acme_root
        .as_ref()
        .map(|p| {
            std::path::absolute(p)
                .map_err(|e| format!("could not resolve --acme-root {}: {e}", p.display()))
        })
        .transpose()?;

    let mut dns_credentials = std::collections::HashMap::new();
    dns_credentials.insert("api_token".to_string(), args.dns_token.clone());
    let config = acme_config::AcmeConfig {
        email: args.email.clone(),
        domain: args.domain.clone(),
        dns_provider: args.dns_provider.clone(),
        dns_credentials,
        staging: args.staging,
        renewal_days_before_expiry: 30,
        post_renewal_hooks: vec![],
        directory_url: args.directory_url.clone(),
        acme_root: acme_root.as_ref().map(|p| p.to_string_lossy().into_owned()),
        dns_propagation_seconds: args.dns_propagation_seconds.unwrap_or(15),
    };

    let config_path = config_dir.join("acme.json");
    acme_config::save_acme_config(&config, &config_path)
        .map_err(|e| format!("could not save {}: {e}", config_path.display()))?;
    debug!(path = %config_path.display(), "saved the ACME configuration");
    // Align before the order too: if it fails, acme.json is renewal's
    // recovery input, and the timer runs unprivileged.
    align_pki_ownership(config_dir)?;

    let resolved =
        acme_config::resolve_credentials(&config.dns_credentials).map_err(|e| e.to_string())?;
    let dns_provider = dns::build_dns_provider(&config.dns_provider, &resolved, &config.domain)
        .await
        .map_err(|e| e.to_string())?;
    let acme_client = acme::RealAcmeClient::new(
        dns_provider.as_ref(),
        acme_root,
        std::time::Duration::from_secs(config.dns_propagation_seconds),
    );

    acme::issue_certificate(&config, &pki, &acme_client)
        .await
        .map_err(|e| e.to_string())?;
    align_pki_ownership(config_dir)?;

    println!("ACME certificate issued for *.{}:", config.domain);
    println!("  cert: {}", acme_config::acme_cert_path(&pki).display());
    println!("  key:  {}", acme_config::acme_key_path(&pki).display());
    if config.staging {
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

    #[cfg(unix)]
    #[test]
    fn test_align_pki_ownership_is_a_noop_on_a_self_owned_tree() {
        let dir = tempfile::tempdir().unwrap();
        let pki = pki_dir(dir.path());
        std::fs::create_dir_all(&pki).unwrap();
        std::fs::write(pki.join("credential"), "secret\n").unwrap();
        align_pki_ownership(dir.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_align_pki_ownership_rehomes_a_foreign_owned_file() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let pki = pki_dir(dir.path());
        std::fs::create_dir_all(&pki).unwrap();
        let key = pki.join("ca-key.pem");
        std::fs::write(&key, "key material").unwrap();
        let acme = dir.path().join("acme.json");
        std::fs::write(&acme, "{}").unwrap();
        // Only a privileged run (a mapped-root userns: `unshare -r
        // --map-auto` around the test binary) can create the cross-owner
        // state; unprivileged, the chowns fail and the assertions reduce
        // to the no-op case.
        let cross_owner = std::os::unix::fs::chown(&key, Some(12345), Some(12345)).is_ok();
        let _ = std::os::unix::fs::chown(&acme, Some(12345), Some(12345));
        align_pki_ownership(dir.path()).unwrap();
        let root = std::fs::metadata(dir.path()).unwrap();
        for path in [&key, &acme] {
            let meta = std::fs::metadata(path).unwrap();
            assert_eq!(meta.uid(), root.uid(), "cross-owner run: {cross_owner}");
            assert_eq!(meta.gid(), root.gid(), "cross-owner run: {cross_owner}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_align_pki_ownership_skips_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let pki = pki_dir(dir.path());
        std::fs::create_dir_all(&pki).unwrap();
        // A dangling symlink: without the skip, the follow-the-link chown
        // would error on the missing target and fail the alignment.
        std::os::unix::fs::symlink("/nonexistent-target", pki.join("stray-link")).unwrap();
        align_pki_ownership(dir.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_align_pki_ownership_tolerates_a_missing_tree() {
        let dir = tempfile::tempdir().unwrap();
        align_pki_ownership(dir.path()).unwrap();
        align_pki_ownership(&dir.path().join("never-created")).unwrap();
    }

    #[test]
    fn test_plan_client_wiring_skips_a_missing_sentinel_config() {
        let dir = tempfile::tempdir().unwrap();
        assert!(plan_client_wiring(dir.path()).is_empty());
    }

    #[test]
    fn test_plan_client_wiring_skips_an_unparseable_sentinel_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sentinel.json"), "{ not json").unwrap();
        assert!(plan_client_wiring(dir.path()).is_empty());
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

    #[tokio::test]
    async fn test_run_acme_persists_a_relative_acme_root_as_absolute() {
        // A renewal timer runs with an arbitrary working directory, so the
        // persisted acme.json must carry an absolute trust-anchor path even
        // when the operator passed a relative one. acme.json is persisted
        // before the DNS provider is built, so a bogus provider name lets
        // this assert on the file without any network.
        let dir = tempfile::tempdir().unwrap();
        let err = run_acme(
            dir.path(),
            AcmeArgs {
                domain: "observatory.test".to_string(),
                dns_provider: "no-such-provider".to_string(),
                dns_token: "tok".to_string(),
                email: "t@observatory.test".to_string(),
                staging: false,
                directory_url: None,
                acme_root: Some(std::path::PathBuf::from("relative/pebble-ca.pem")),
                dns_propagation_seconds: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.contains("unsupported DNS provider"), "{err}");

        let saved = std::fs::read_to_string(dir.path().join("acme.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
        let root = value["acme_root"].as_str().unwrap();
        assert!(
            std::path::Path::new(root).is_absolute(),
            "persisted acme_root must be absolute: {root}"
        );
        assert!(root.ends_with("pebble-ca.pem"), "{root}");
    }
}
