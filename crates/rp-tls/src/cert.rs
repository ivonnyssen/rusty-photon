use std::fs;
use std::path::Path;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use tracing::debug;

use crate::error::{Result, TlsError};

/// Duration for CA certificate validity (10 years in days).
const CA_VALIDITY_DAYS: i64 = 3650;

/// Duration for service certificate validity (10 years in days).
const SERVICE_VALIDITY_DAYS: i64 = 3650;

/// Generate a self-signed root CA certificate and key.
///
/// Writes `ca.pem` and `ca-key.pem` to `output_dir`.
/// Creates `output_dir` if it does not exist.
pub fn generate_ca(output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "Rusty Photon Observatory CA");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Rusty Photon");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = time::OffsetDateTime::now_utc();
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(CA_VALIDITY_DAYS);

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    let cert_path = output_dir.join("ca.pem");
    let key_path = output_dir.join("ca-key.pem");

    fs::write(&cert_path, cert.pem())?;
    fs::write(&key_path, key_pair.serialize_pem())?;

    debug!("Generated CA certificate: {}", cert_path.display());
    debug!("Generated CA private key: {}", key_path.display());

    Ok(())
}

/// Generate a service certificate signed by the CA.
///
/// The certificate includes SANs for `localhost`, `127.0.0.1`, the system
/// hostname, and any additional SANs provided.
///
/// Writes `{service_name}.pem` and `{service_name}-key.pem` to `output_dir`.
/// Creates `output_dir` if it does not exist.
pub fn generate_service_cert(
    ca_cert_pem: &str,
    ca_key_pem: &str,
    service_name: &str,
    extra_sans: &[String],
    output_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    // Load CA
    let ca_key = KeyPair::from_pem(ca_key_pem)?;
    let ca_params = CertificateParams::from_ca_cert_pem(ca_cert_pem)?;
    let ca_cert = ca_params.self_signed(&ca_key)?;

    // Build service cert params
    let mut params = CertificateParams::new(build_dns_sans(extra_sans))
        .map_err(|e| TlsError::Other(format!("invalid SAN: {e}")))?;

    params
        .distinguished_name
        .push(DnType::CommonName, service_name);
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Rusty Photon");
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = time::OffsetDateTime::now_utc();
    params.not_after =
        time::OffsetDateTime::now_utc() + time::Duration::days(SERVICE_VALIDITY_DAYS);

    // Add IP SANs
    params
        .subject_alt_names
        .push(SanType::IpAddress(std::net::IpAddr::V4(
            std::net::Ipv4Addr::LOCALHOST,
        )));
    params
        .subject_alt_names
        .push(SanType::IpAddress(std::net::IpAddr::V6(
            std::net::Ipv6Addr::LOCALHOST,
        )));

    let service_key = KeyPair::generate()?;
    let service_cert = params.signed_by(&service_key, &ca_cert, &ca_key)?;

    let cert_path = output_dir.join(format!("{service_name}.pem"));
    let key_path = output_dir.join(format!("{service_name}-key.pem"));

    fs::write(&cert_path, service_cert.pem())?;
    fs::write(&key_path, service_key.serialize_pem())?;

    debug!(
        "Generated service certificate for '{}': {}",
        service_name,
        cert_path.display()
    );

    Ok(())
}

/// Build the list of DNS SANs for a service certificate.
fn build_dns_sans(extra_sans: &[String]) -> Vec<String> {
    let mut sans = vec!["localhost".to_string()];

    // Add system hostname
    if let Some(hostname) = get_hostname() {
        if hostname != "localhost" {
            sans.push(hostname);
        }
    }

    // Add user-provided extra SANs
    for san in extra_sans {
        if !sans.contains(san) {
            sans.push(san.clone());
        }
    }

    sans
}

/// Get the system hostname, or `None` if it cannot be determined.
fn get_hostname() -> Option<String> {
    hostname::get().ok().and_then(|h| h.into_string().ok())
}

/// List of default services for certificate generation.
pub const DEFAULT_SERVICES: &[&str] = &[
    "filemonitor",
    "ppba-driver",
    "qhy-focuser",
    "rp",
    "sentinel",
];

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn generate_ca_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        generate_ca(dir.path()).unwrap();

        assert!(dir.path().join("ca.pem").exists());
        assert!(dir.path().join("ca-key.pem").exists());

        // Verify PEM contents are non-empty and look like PEM
        let cert_pem = fs::read_to_string(dir.path().join("ca.pem")).unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));

        let key_pem = fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
        assert!(key_pem.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn generate_service_cert_creates_files() {
        let ca_dir = tempfile::tempdir().unwrap();
        generate_ca(ca_dir.path()).unwrap();

        let ca_cert_pem = fs::read_to_string(ca_dir.path().join("ca.pem")).unwrap();
        let ca_key_pem = fs::read_to_string(ca_dir.path().join("ca-key.pem")).unwrap();

        let certs_dir = tempfile::tempdir().unwrap();
        generate_service_cert(
            &ca_cert_pem,
            &ca_key_pem,
            "test-service",
            &[],
            certs_dir.path(),
        )
        .unwrap();

        assert!(certs_dir.path().join("test-service.pem").exists());
        assert!(certs_dir.path().join("test-service-key.pem").exists());

        let cert_pem = fs::read_to_string(certs_dir.path().join("test-service.pem")).unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn generate_service_cert_with_extra_sans() {
        let ca_dir = tempfile::tempdir().unwrap();
        generate_ca(ca_dir.path()).unwrap();

        let ca_cert_pem = fs::read_to_string(ca_dir.path().join("ca.pem")).unwrap();
        let ca_key_pem = fs::read_to_string(ca_dir.path().join("ca-key.pem")).unwrap();

        let certs_dir = tempfile::tempdir().unwrap();
        generate_service_cert(
            &ca_cert_pem,
            &ca_key_pem,
            "test-service",
            &["observatory.local".to_string()],
            certs_dir.path(),
        )
        .unwrap();

        assert!(certs_dir.path().join("test-service.pem").exists());
    }

    #[test]
    fn build_dns_sans_includes_localhost() {
        let sans = build_dns_sans(&[]);
        assert!(sans.contains(&"localhost".to_string()));
    }

    #[test]
    fn build_dns_sans_deduplicates() {
        let sans = build_dns_sans(&["localhost".to_string()]);
        assert_eq!(
            sans.iter().filter(|s| *s == "localhost").count(),
            1,
            "localhost should appear exactly once"
        );
    }

    #[test]
    fn generated_cert_chain_validates() {
        use rustls::pki_types::CertificateDer;

        let ca_dir = tempfile::tempdir().unwrap();
        generate_ca(ca_dir.path()).unwrap();

        let ca_cert_pem = fs::read_to_string(ca_dir.path().join("ca.pem")).unwrap();
        let ca_key_pem = fs::read_to_string(ca_dir.path().join("ca-key.pem")).unwrap();

        let certs_dir = tempfile::tempdir().unwrap();
        generate_service_cert(
            &ca_cert_pem,
            &ca_key_pem,
            "test-service",
            &[],
            certs_dir.path(),
        )
        .unwrap();

        // Load and parse the service cert + key to verify they are valid
        let cert_pem = fs::read_to_string(certs_dir.path().join("test-service.pem")).unwrap();
        let key_pem = fs::read_to_string(certs_dir.path().join("test-service-key.pem")).unwrap();

        let certs: Vec<CertificateDer> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(certs.len(), 1, "should have exactly one certificate");

        let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
            .unwrap()
            .expect("should have a private key");

        // Verify we can build a rustls ServerConfig with these certs
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key);

        assert!(
            server_config.is_ok(),
            "should build valid rustls config: {:?}",
            server_config.err()
        );
    }
}
