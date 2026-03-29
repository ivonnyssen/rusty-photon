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

/// Run the ACME certificate issuance flow.
///
/// Creates an ACME account, requests a wildcard certificate via DNS-01
/// challenge, and writes the certificate and key to the PKI directory.
pub async fn run_acme(
    output_dir: Option<&str>,
    domain: Option<&str>,
    dns_provider_name: Option<&str>,
    dns_token: Option<&str>,
    email: Option<&str>,
    staging: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let domain = domain.ok_or("--domain is required with --acme")?;
    let dns_provider_name = dns_provider_name.ok_or("--dns-provider is required with --acme")?;
    let dns_token = dns_token.ok_or("--dns-token is required with --acme")?;
    let email = email.ok_or("--email is required with --acme")?;

    let pki_dir = match output_dir {
        Some(dir) => config::expand_tilde(dir),
        None => config::default_pki_dir(),
    };

    // Build and save ACME config
    let mut dns_credentials = std::collections::HashMap::new();
    dns_credentials.insert("api_token".to_string(), dns_token.to_string());

    let acme_config = rp_tls::acme_config::AcmeConfig {
        email: email.to_string(),
        domain: domain.to_string(),
        dns_provider: dns_provider_name.to_string(),
        dns_credentials,
        staging,
        renewal_days_before_expiry: 30,
        post_renewal_hooks: vec![],
    };

    let config_path = match output_dir {
        Some(dir) => config::expand_tilde(dir).join("acme.json"),
        None => rp_tls::acme_config::default_acme_config_path(),
    };
    rp_tls::acme_config::save_acme_config(&acme_config, &config_path)?;
    info!("Saved ACME configuration to {}", config_path.display());

    // Build DNS provider and issue certificate
    let resolved_creds = rp_tls::acme_config::resolve_credentials(&acme_config.dns_credentials)?;
    let dns_provider =
        rp_tls::dns::build_dns_provider(&acme_config.dns_provider, &resolved_creds, domain).await?;
    let acme_client = rp_tls::acme::RealAcmeClient::new(dns_provider.as_ref());

    run_acme_issue(&acme_config, &pki_dir, &acme_client).await
}

/// Inner function that issues the certificate and prints the summary.
///
/// Separated from `run_acme` so it can be tested with mock dependencies.
async fn run_acme_issue(
    acme_config: &rp_tls::acme_config::AcmeConfig,
    pki_dir: &Path,
    acme_client: &dyn rp_tls::acme::AcmeClient,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        "Requesting wildcard certificate for *.{}",
        acme_config.domain
    );
    if acme_config.staging {
        info!("Using Let's Encrypt STAGING environment");
    }

    rp_tls::acme::issue_certificate(acme_config, pki_dir, acme_client).await?;

    // Print summary
    let cert_path = rp_tls::acme_config::acme_cert_path(pki_dir);
    let key_path = rp_tls::acme_config::acme_key_path(pki_dir);
    println!("\nACME certificate issued successfully:");
    println!("  Certificate: {}", cert_path.display());
    println!("  Private key: {}", key_path.display());
    println!("  Domain:      *.{}", acme_config.domain);
    if acme_config.staging {
        println!("  Environment: STAGING (not trusted by browsers)");
    }

    print_acme_config_hint(pki_dir);

    Ok(())
}

/// Print a hint showing how to configure services to use ACME certs.
fn print_acme_config_hint(pki_dir: &Path) {
    let cert = rp_tls::acme_config::acme_cert_path(pki_dir);
    let key = rp_tls::acme_config::acme_key_path(pki_dir);
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
    println!("\nNo CA configuration needed for clients -- Let's Encrypt is publicly trusted.");
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
    use rp_tls::acme::AcmeClient;
    use rp_tls::error::TlsError;

    // Local mock of AcmeClient for testing run_acme_issue.
    // The rp-tls crate's MockAcmeClient is only available in its own test builds,
    // so we define one here via mockall::mock!.
    mockall::mock! {
        Acme {}
        #[async_trait::async_trait]
        impl AcmeClient for Acme {
            async fn create_or_load_account(
                &self,
                email: String,
                directory_url: String,
                existing_credentials_json: Option<String>,
            ) -> rp_tls::error::Result<Option<String>>;
            async fn order_certificate(
                &self,
                domain: String,
            ) -> rp_tls::error::Result<(String, String)>;
        }
    }

    #[tokio::test]
    async fn run_acme_issue_happy_path() {
        let dir = tempfile::tempdir().unwrap();
        let acme_config = rp_tls::acme_config::AcmeConfig {
            email: "test@example.com".to_string(),
            domain: "observatory.example.com".to_string(),
            dns_provider: "cloudflare".to_string(),
            dns_credentials: std::collections::HashMap::new(),
            staging: false,
            renewal_days_before_expiry: 30,
            post_renewal_hooks: vec![],
        };

        let mut mock = MockAcme::new();
        mock.expect_create_or_load_account()
            .returning(|_, _, _| Ok(Some(r#"{"account":"creds"}"#.to_string())));
        mock.expect_order_certificate()
            .returning(|_| Ok(("CERT-CHAIN".to_string(), "PRIV-KEY".to_string())));

        run_acme_issue(&acme_config, dir.path(), &mock)
            .await
            .unwrap();

        // Verify cert and key files written
        let cert_path = rp_tls::acme_config::acme_cert_path(dir.path());
        let key_path = rp_tls::acme_config::acme_key_path(dir.path());
        assert_eq!(std::fs::read_to_string(cert_path).unwrap(), "CERT-CHAIN");
        assert_eq!(std::fs::read_to_string(key_path).unwrap(), "PRIV-KEY");
    }

    #[tokio::test]
    async fn run_acme_issue_staging_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let acme_config = rp_tls::acme_config::AcmeConfig {
            email: "test@example.com".to_string(),
            domain: "observatory.example.com".to_string(),
            dns_provider: "cloudflare".to_string(),
            dns_credentials: std::collections::HashMap::new(),
            staging: true,
            renewal_days_before_expiry: 30,
            post_renewal_hooks: vec![],
        };

        let mut mock = MockAcme::new();
        mock.expect_create_or_load_account()
            .returning(|_, _, _| Ok(None));
        mock.expect_order_certificate()
            .returning(|_| Ok(("CERT".to_string(), "KEY".to_string())));

        run_acme_issue(&acme_config, dir.path(), &mock)
            .await
            .unwrap();

        assert!(rp_tls::acme_config::acme_cert_path(dir.path()).exists());
    }

    #[tokio::test]
    async fn run_acme_issue_error_propagates() {
        let dir = tempfile::tempdir().unwrap();
        let acme_config = rp_tls::acme_config::AcmeConfig {
            email: "test@example.com".to_string(),
            domain: "example.com".to_string(),
            dns_provider: "cloudflare".to_string(),
            dns_credentials: std::collections::HashMap::new(),
            staging: true,
            renewal_days_before_expiry: 30,
            post_renewal_hooks: vec![],
        };

        let mut mock = MockAcme::new();
        mock.expect_create_or_load_account()
            .returning(|_, _, _| Err(TlsError::Acme("test error".to_string())));

        let err = run_acme_issue(&acme_config, dir.path(), &mock)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("test error"), "error: {err}");
    }

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
