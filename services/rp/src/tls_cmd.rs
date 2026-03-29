use std::fs;
use std::path::Path;

use rp_tls::cert::{self, DEFAULT_SERVICES};
use rp_tls::config;
use tracing::{debug, info};

/// Run the `init-tls` command: generate CA and per-service certificates.
pub fn run(
    output_dir: Option<&str>,
    services: Option<&[String]>,
    extra_sans: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let pki_dir = match output_dir {
        Some(dir) => config::expand_tilde(dir),
        None => config::default_pki_dir(),
    };
    let certs_dir = pki_dir.join("certs");

    debug!("PKI directory: {}", pki_dir.display());
    debug!("Certs directory: {}", certs_dir.display());

    // Generate or reuse CA
    let ca_cert_path = config::ca_cert_path(&pki_dir);
    let ca_key_path = config::ca_key_path(&pki_dir);

    if ca_cert_path.exists() && ca_key_path.exists() {
        info!(
            "CA certificate already exists at {}, skipping generation",
            ca_cert_path.display()
        );
    } else {
        cert::generate_ca(&pki_dir)?;
        info!("Generated CA certificate: {}", ca_cert_path.display());
        info!("Generated CA private key: {}", ca_key_path.display());
    }

    // Load CA for signing
    let ca_cert_pem = fs::read_to_string(&ca_cert_path)?;
    let ca_key_pem = fs::read_to_string(&ca_key_path)?;

    // Determine which services to generate certs for
    let service_list: Vec<&str> = match services {
        Some(names) => names.iter().map(|s| s.as_str()).collect(),
        None => DEFAULT_SERVICES.to_vec(),
    };

    // Generate per-service certs
    for service_name in &service_list {
        cert::generate_service_cert(
            &ca_cert_pem,
            &ca_key_pem,
            service_name,
            extra_sans,
            &certs_dir,
        )?;
        info!(
            "Generated certificate for '{}': {}",
            service_name,
            certs_dir.join(format!("{service_name}.pem")).display()
        );
    }

    // Print summary
    println!("\nTLS certificates generated successfully:");
    println!("  CA cert:    {}", ca_cert_path.display());
    println!("  CA key:     {}", ca_key_path.display());
    println!("  Certs dir:  {}", certs_dir.display());
    println!("\n  Services:");
    for name in &service_list {
        println!("    - {name}.pem / {name}-key.pem");
    }

    print_config_hint(&certs_dir, &ca_cert_path, &service_list);

    Ok(())
}

/// Print a hint showing how to configure services to use the generated certs.
fn print_config_hint(certs_dir: &Path, ca_cert_path: &Path, services: &[&str]) {
    if let Some(first) = services.first() {
        let cert = certs_dir.join(format!("{first}.pem"));
        let key = certs_dir.join(format!("{first}-key.pem"));
        println!("\nAdd to each service's config.json:");
        println!(
            r#"  "server": {{
    "tls": {{
      "cert": "{}",
      "key": "{}"
    }}
  }}"#,
            cert.display(),
            key.display()
        );
        println!(
            "\nFor clients (sentinel), add:\n  \"ca_cert\": \"{}\"",
            ca_cert_path.display()
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn run_generates_all_default_certs() {
        let dir = tempfile::tempdir().unwrap();
        run(Some(dir.path().to_str().unwrap()), None, &[]).unwrap();

        // CA files
        assert!(dir.path().join("ca.pem").exists());
        assert!(dir.path().join("ca-key.pem").exists());

        // Service cert files
        for svc in DEFAULT_SERVICES {
            assert!(
                dir.path().join("certs").join(format!("{svc}.pem")).exists(),
                "missing cert for {svc}"
            );
            assert!(
                dir.path()
                    .join("certs")
                    .join(format!("{svc}-key.pem"))
                    .exists(),
                "missing key for {svc}"
            );
        }
    }

    #[test]
    fn run_is_idempotent_preserves_ca() {
        let dir = tempfile::tempdir().unwrap();

        // First run
        run(Some(dir.path().to_str().unwrap()), None, &[]).unwrap();
        let ca_contents_1 = fs::read_to_string(dir.path().join("ca.pem")).unwrap();

        // Second run — CA should be preserved
        run(Some(dir.path().to_str().unwrap()), None, &[]).unwrap();
        let ca_contents_2 = fs::read_to_string(dir.path().join("ca.pem")).unwrap();

        assert_eq!(
            ca_contents_1, ca_contents_2,
            "CA cert should be preserved on re-run"
        );
    }

    #[test]
    fn run_with_custom_services() {
        let dir = tempfile::tempdir().unwrap();
        let services = vec!["my-service".to_string()];
        run(Some(dir.path().to_str().unwrap()), Some(&services), &[]).unwrap();

        assert!(dir.path().join("certs/my-service.pem").exists());
        assert!(dir.path().join("certs/my-service-key.pem").exists());

        // Default services should NOT exist
        assert!(!dir.path().join("certs/filemonitor.pem").exists());
    }

    #[test]
    fn run_with_extra_sans() {
        let dir = tempfile::tempdir().unwrap();
        let services = vec!["test-svc".to_string()];
        let extra = vec!["observatory.local".to_string()];
        run(Some(dir.path().to_str().unwrap()), Some(&services), &extra).unwrap();

        assert!(dir.path().join("certs/test-svc.pem").exists());
    }
}
