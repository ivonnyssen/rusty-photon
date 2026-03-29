use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::acme_config::{self, AcmeConfig};
use crate::dns::DnsProvider;
use crate::error::{Result, TlsError};
use crate::permissions::set_restricted_permissions;

/// Trait abstracting the ACME protocol operations.
///
/// This allows mocking the ACME client in tests without making real
/// HTTP calls to Let's Encrypt. Uses owned types to be mockall-compatible.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AcmeClient: Send + Sync {
    /// Create or load an ACME account.
    ///
    /// If `existing_credentials_json` is `Some`, the account is restored
    /// from those credentials. Otherwise a new account is created.
    ///
    /// Returns `Some(credentials_json)` if a new account was created
    /// (caller should persist it), or `None` if an existing account was loaded.
    async fn create_or_load_account(
        &self,
        email: String,
        directory_url: String,
        existing_credentials_json: Option<String>,
    ) -> Result<Option<String>>;

    /// Run the full ACME order flow for a wildcard domain:
    /// create order, solve DNS-01 challenges, finalize, return (cert_pem, key_pem).
    ///
    /// Must be called after `create_or_load_account`.
    async fn order_certificate(&self, domain: String) -> Result<(String, String)>;
}

/// Real ACME client using `instant-acme`.
///
/// Stores the account handle after `create_or_load_account` so that
/// `order_certificate` can use it. Holds a DNS provider reference
/// for solving DNS-01 challenges.
pub struct RealAcmeClient<'a> {
    dns_provider: &'a dyn DnsProvider,
    account: Arc<Mutex<Option<instant_acme::Account>>>,
}

impl<'a> RealAcmeClient<'a> {
    pub fn new(dns_provider: &'a dyn DnsProvider) -> Self {
        Self {
            dns_provider,
            account: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl AcmeClient for RealAcmeClient<'_> {
    async fn create_or_load_account(
        &self,
        email: String,
        directory_url: String,
        existing_credentials_json: Option<String>,
    ) -> Result<Option<String>> {
        use instant_acme::{Account, AccountCredentials, NewAccount};

        if let Some(json) = existing_credentials_json {
            debug!("Loading ACME account from credentials");
            let credentials: AccountCredentials = serde_json::from_str(&json)
                .map_err(|e| TlsError::Acme(format!("failed to parse account credentials: {e}")))?;
            let account = Account::builder()
                .map_err(|e| TlsError::Acme(format!("failed to create account builder: {e}")))?
                .from_credentials(credentials)
                .await
                .map_err(|e| TlsError::Acme(format!("failed to load account: {e}")))?;
            *self.account.lock().await = Some(account);
            debug!("Loaded existing ACME account");
            Ok(None)
        } else {
            debug!("Creating new ACME account at {}", directory_url);
            let contact = format!("mailto:{email}");
            let new_account = NewAccount {
                contact: &[&contact],
                terms_of_service_agreed: true,
                only_return_existing: false,
            };

            let (account, credentials) = Account::builder()
                .map_err(|e| TlsError::Acme(format!("failed to create account builder: {e}")))?
                .create(&new_account, directory_url, None)
                .await
                .map_err(|e| TlsError::Acme(format!("failed to create ACME account: {e}")))?;

            *self.account.lock().await = Some(account);

            let json = serde_json::to_string_pretty(&credentials)
                .map_err(|e| TlsError::Acme(format!("failed to serialize credentials: {e}")))?;
            info!("Created new ACME account");
            Ok(Some(json))
        }
    }

    async fn order_certificate(&self, domain: String) -> Result<(String, String)> {
        use instant_acme::{ChallengeType, Identifier, NewOrder, RetryPolicy};

        let mut account_guard = self.account.lock().await;
        let account = account_guard.as_mut().ok_or_else(|| {
            TlsError::Acme(
                "account not initialized — call create_or_load_account first".to_string(),
            )
        })?;

        // Create order for wildcard domain
        let wildcard = format!("*.{domain}");
        let identifiers = [Identifier::Dns(wildcard.clone())];
        let new_order = NewOrder::new(&identifiers);

        debug!("Creating ACME order for {}", wildcard);
        let mut order = account
            .new_order(&new_order)
            .await
            .map_err(|e| TlsError::Acme(format!("failed to create order: {e}")))?;

        // Solve DNS-01 challenges
        let challenge_fqdn = format!("_acme-challenge.{domain}");
        let mut auths = order.authorizations();
        while let Some(auth_result) = auths.next().await {
            let mut auth = auth_result
                .map_err(|e| TlsError::Acme(format!("failed to get authorization: {e}")))?;

            let mut challenge = auth.challenge(ChallengeType::Dns01).ok_or_else(|| {
                TlsError::Acme("no DNS-01 challenge offered by server".to_string())
            })?;

            let key_auth = challenge.key_authorization();
            let dns_value = key_auth.dns_value();

            debug!(
                "Setting up DNS-01 challenge for {} with value {}",
                challenge_fqdn, dns_value
            );

            self.dns_provider
                .create_txt_record(&challenge_fqdn, &dns_value)
                .await?;

            debug!("Waiting 15 seconds for DNS propagation");
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;

            challenge
                .set_ready()
                .await
                .map_err(|e| TlsError::Acme(format!("failed to set challenge ready: {e}")))?;

            debug!("Challenge marked as ready");
        }

        debug!("Polling order until ready");
        order
            .poll_ready(&RetryPolicy::default())
            .await
            .map_err(|e| TlsError::Acme(format!("order did not become ready: {e}")))?;

        debug!("Cleaning up DNS challenge record");
        if let Err(e) = self.dns_provider.delete_txt_record(&challenge_fqdn).await {
            debug!("Warning: failed to clean up DNS record: {e}");
        }

        // Finalize
        debug!("Finalizing order (generating CSR)");
        let private_key_pem = order
            .finalize()
            .await
            .map_err(|e| TlsError::Acme(format!("failed to finalize order: {e}")))?;

        debug!("Polling for certificate");
        let cert_chain_pem = order
            .poll_certificate(&RetryPolicy::default())
            .await
            .map_err(|e| TlsError::Acme(format!("failed to retrieve certificate: {e}")))?;

        Ok((cert_chain_pem, private_key_pem))
    }
}

/// Issue a wildcard certificate via ACME.
///
/// This is the main entry point for certificate issuance:
/// 1. Creates or loads an ACME account (via `AcmeClient`)
/// 2. Persists new account credentials if created
/// 3. Orders the certificate (via `AcmeClient`)
/// 4. Writes the certificate and private key to disk
pub async fn issue_certificate(
    config: &AcmeConfig,
    pki_dir: &Path,
    acme_client: &dyn AcmeClient,
) -> Result<()> {
    let account_path = acme_config::acme_account_path(pki_dir);
    let directory_url = acme_config::directory_url(config.staging).to_string();

    // Load existing credentials if available
    let existing_creds = if account_path.exists() {
        debug!("Loading ACME account from {}", account_path.display());
        Some(std::fs::read_to_string(&account_path)?)
    } else {
        None
    };

    // Create or load account
    let new_creds = acme_client
        .create_or_load_account(config.email.clone(), directory_url, existing_creds)
        .await?;

    // Persist new credentials if account was just created
    if let Some(creds_json) = new_creds {
        std::fs::create_dir_all(pki_dir)?;
        std::fs::write(&account_path, &creds_json)?;
        set_restricted_permissions(&account_path)?;
        info!(
            "Saved ACME account credentials to {}",
            account_path.display()
        );
    }

    // Order certificate
    let (cert_chain_pem, private_key_pem) =
        acme_client.order_certificate(config.domain.clone()).await?;

    // Write certificate and key
    let certs_dir = pki_dir.join("certs");
    std::fs::create_dir_all(&certs_dir)?;

    let cert_path = acme_config::acme_cert_path(pki_dir);
    let key_path = acme_config::acme_key_path(pki_dir);

    std::fs::write(&cert_path, &cert_chain_pem)?;
    std::fs::write(&key_path, &private_key_pem)?;
    set_restricted_permissions(&key_path)?;

    info!("Certificate written to {}", cert_path.display());
    info!("Private key written to {}", key_path.display());

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn test_config() -> AcmeConfig {
        AcmeConfig {
            email: "test@example.com".to_string(),
            domain: "example.com".to_string(),
            dns_provider: "cloudflare".to_string(),
            dns_credentials: std::collections::HashMap::new(),
            staging: true,
            renewal_days_before_expiry: 30,
            post_renewal_hooks: vec![],
        }
    }

    #[tokio::test]
    async fn issue_certificate_happy_path_new_account() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config();

        let mut mock_acme = MockAcmeClient::new();
        mock_acme
            .expect_create_or_load_account()
            .returning(|_, _, _| Ok(Some(r#"{"fake":"credentials"}"#.to_string())));
        mock_acme
            .expect_order_certificate()
            .returning(|_| Ok(("CERT-PEM-DATA".to_string(), "KEY-PEM-DATA".to_string())));

        issue_certificate(&config, dir.path(), &mock_acme)
            .await
            .unwrap();

        // Verify account credentials were saved
        let account_path = acme_config::acme_account_path(dir.path());
        assert!(account_path.exists(), "account credentials should be saved");
        let saved = std::fs::read_to_string(&account_path).unwrap();
        assert_eq!(saved, r#"{"fake":"credentials"}"#);

        // Verify cert and key were written
        let cert_path = acme_config::acme_cert_path(dir.path());
        let key_path = acme_config::acme_key_path(dir.path());
        assert_eq!(std::fs::read_to_string(cert_path).unwrap(), "CERT-PEM-DATA");
        assert_eq!(std::fs::read_to_string(key_path).unwrap(), "KEY-PEM-DATA");
    }

    #[tokio::test]
    async fn issue_certificate_loads_existing_account() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config();

        // Pre-create account credentials file
        let account_path = acme_config::acme_account_path(dir.path());
        std::fs::create_dir_all(account_path.parent().unwrap()).unwrap();
        std::fs::write(&account_path, r#"{"existing":"creds"}"#).unwrap();

        let mut mock_acme = MockAcmeClient::new();
        mock_acme
            .expect_create_or_load_account()
            .withf(|_, _, existing| existing.as_deref() == Some(r#"{"existing":"creds"}"#))
            .returning(|_, _, _| Ok(None));
        mock_acme
            .expect_order_certificate()
            .returning(|_| Ok(("CERT".to_string(), "KEY".to_string())));

        issue_certificate(&config, dir.path(), &mock_acme)
            .await
            .unwrap();

        // Verify cert was still written
        let cert_path = acme_config::acme_cert_path(dir.path());
        assert!(cert_path.exists());
    }

    #[tokio::test]
    async fn issue_certificate_acme_account_error_propagates() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config();

        let mut mock_acme = MockAcmeClient::new();
        mock_acme
            .expect_create_or_load_account()
            .returning(|_, _, _| Err(TlsError::Acme("account creation failed".to_string())));

        let err = issue_certificate(&config, dir.path(), &mock_acme)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("account creation failed"),
            "error: {err}"
        );
    }

    #[tokio::test]
    async fn issue_certificate_order_error_propagates() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config();

        let mut mock_acme = MockAcmeClient::new();
        mock_acme
            .expect_create_or_load_account()
            .returning(|_, _, _| Ok(None));
        mock_acme
            .expect_order_certificate()
            .returning(|_| Err(TlsError::Acme("order failed".to_string())));

        let err = issue_certificate(&config, dir.path(), &mock_acme)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("order failed"), "error: {err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn issue_certificate_sets_key_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let config = test_config();

        let mut mock_acme = MockAcmeClient::new();
        mock_acme
            .expect_create_or_load_account()
            .returning(|_, _, _| Ok(None));
        mock_acme
            .expect_order_certificate()
            .returning(|_| Ok(("CERT".to_string(), "KEY".to_string())));

        issue_certificate(&config, dir.path(), &mock_acme)
            .await
            .unwrap();

        let key_path = acme_config::acme_key_path(dir.path());
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "key should have 0600 permissions, got {mode:o}"
        );
    }

    #[tokio::test]
    async fn real_acme_client_invalid_credentials_returns_parse_error() {
        let dns = crate::dns::MockDnsProvider::new();
        let client = RealAcmeClient::new(&dns);
        let result = client
            .create_or_load_account(
                "test@example.com".to_string(),
                "https://acme-staging-v02.api.letsencrypt.org/directory".to_string(),
                Some("not valid json".to_string()),
            )
            .await;
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("parse"),
            "error should mention parse failure: {msg}"
        );
    }
}
