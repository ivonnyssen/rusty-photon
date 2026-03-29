use std::path::Path;

use instant_acme::{
    Account, AccountCredentials, ChallengeType, Identifier, NewAccount, NewOrder, RetryPolicy,
};
use tracing::{debug, info};

use crate::acme_config::{self, AcmeConfig};
use crate::dns::DnsProvider;
use crate::error::{Result, TlsError};
use crate::permissions::set_restricted_permissions;

/// Create or load an ACME account.
///
/// If `acme-account.json` exists in the PKI directory, the account is
/// restored from the saved credentials. Otherwise, a new account is
/// created and the credentials are persisted.
pub async fn create_or_load_account(config: &AcmeConfig, pki_dir: &Path) -> Result<Account> {
    let account_path = acme_config::acme_account_path(pki_dir);
    let directory_url = acme_config::directory_url(config.staging).to_string();

    if account_path.exists() {
        debug!("Loading ACME account from {}", account_path.display());
        let json = std::fs::read_to_string(&account_path)?;
        let credentials: AccountCredentials = serde_json::from_str(&json)
            .map_err(|e| TlsError::Acme(format!("failed to parse account credentials: {e}")))?;
        let account = Account::builder()
            .map_err(|e| TlsError::Acme(format!("failed to create account builder: {e}")))?
            .from_credentials(credentials)
            .await
            .map_err(|e| TlsError::Acme(format!("failed to load account: {e}")))?;
        debug!("Loaded existing ACME account");
        Ok(account)
    } else {
        debug!("Creating new ACME account at {}", directory_url);
        let contact = format!("mailto:{}", config.email);
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

        // Persist account credentials
        std::fs::create_dir_all(pki_dir)?;
        let json = serde_json::to_string_pretty(&credentials)
            .map_err(|e| TlsError::Acme(format!("failed to serialize credentials: {e}")))?;
        std::fs::write(&account_path, json)?;
        set_restricted_permissions(&account_path)?;
        info!(
            "Created new ACME account, saved to {}",
            account_path.display()
        );

        Ok(account)
    }
}

/// Solve DNS-01 challenges for all authorizations in an order.
///
/// For each authorization:
/// 1. Creates a TXT record via the DNS provider
/// 2. Waits for DNS propagation
/// 3. Notifies the ACME server the challenge is ready
///
/// After all challenges are solved, polls the order until it is ready
/// and cleans up the DNS record.
pub async fn solve_dns01_challenges(
    order: &mut instant_acme::Order,
    dns_provider: &dyn DnsProvider,
    domain: &str,
) -> Result<()> {
    let challenge_fqdn = format!("_acme-challenge.{domain}");

    let mut auths = order.authorizations();
    while let Some(auth_result) = auths.next().await {
        let mut auth =
            auth_result.map_err(|e| TlsError::Acme(format!("failed to get authorization: {e}")))?;

        let mut challenge = auth
            .challenge(ChallengeType::Dns01)
            .ok_or_else(|| TlsError::Acme("no DNS-01 challenge offered by server".to_string()))?;

        let key_auth = challenge.key_authorization();
        let dns_value = key_auth.dns_value();

        debug!(
            "Setting up DNS-01 challenge for {} with value {}",
            challenge_fqdn, dns_value
        );

        // Create TXT record
        dns_provider
            .create_txt_record(&challenge_fqdn, &dns_value)
            .await?;

        // Wait for DNS propagation
        debug!("Waiting 15 seconds for DNS propagation");
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;

        // Notify the ACME server
        challenge
            .set_ready()
            .await
            .map_err(|e| TlsError::Acme(format!("failed to set challenge ready: {e}")))?;

        debug!("Challenge marked as ready");
    }

    // Poll until order is ready for finalization
    debug!("Polling order until ready");
    order
        .poll_ready(&RetryPolicy::default())
        .await
        .map_err(|e| TlsError::Acme(format!("order did not become ready: {e}")))?;

    // Clean up DNS record
    debug!("Cleaning up DNS challenge record");
    if let Err(e) = dns_provider.delete_txt_record(&challenge_fqdn).await {
        debug!("Warning: failed to clean up DNS record: {e}");
    }

    Ok(())
}

/// Issue a wildcard certificate via ACME.
///
/// This is the main entry point for certificate issuance:
/// 1. Creates or loads an ACME account
/// 2. Creates a new order for `*.<domain>`
/// 3. Solves DNS-01 challenges
/// 4. Finalizes the order (generates CSR internally)
/// 5. Retrieves and writes the certificate and private key
pub async fn issue_certificate(
    config: &AcmeConfig,
    pki_dir: &Path,
    dns_provider: &dyn DnsProvider,
) -> Result<()> {
    let account = create_or_load_account(config, pki_dir).await?;

    // Create order for wildcard domain
    let wildcard = format!("*.{}", config.domain);
    let identifiers = [Identifier::Dns(wildcard.clone())];
    let new_order = NewOrder::new(&identifiers);

    debug!("Creating ACME order for {}", wildcard);
    let mut order = account
        .new_order(&new_order)
        .await
        .map_err(|e| TlsError::Acme(format!("failed to create order: {e}")))?;

    // Solve challenges
    solve_dns01_challenges(&mut order, dns_provider, &config.domain).await?;

    // Finalize — generates CSR and returns private key PEM
    debug!("Finalizing order (generating CSR)");
    let private_key_pem = order
        .finalize()
        .await
        .map_err(|e| TlsError::Acme(format!("failed to finalize order: {e}")))?;

    // Poll for certificate
    debug!("Polling for certificate");
    let cert_chain_pem = order
        .poll_certificate(&RetryPolicy::default())
        .await
        .map_err(|e| TlsError::Acme(format!("failed to retrieve certificate: {e}")))?;

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

    #[test]
    fn invalid_account_json_returns_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let account_path = dir.path().join("acme-account.json");
            std::fs::write(&account_path, "not valid json").unwrap();

            let config = AcmeConfig {
                email: "test@example.com".to_string(),
                domain: "example.com".to_string(),
                dns_provider: "cloudflare".to_string(),
                dns_credentials: std::collections::HashMap::new(),
                staging: true,
                renewal_days_before_expiry: 30,
                post_renewal_hooks: vec![],
            };

            let result = create_or_load_account(&config, dir.path()).await;
            assert!(result.is_err(), "should fail with invalid JSON");
            let msg = result.err().unwrap().to_string();
            assert!(
                msg.contains("parse"),
                "error should mention parse failure: {msg}"
            );
        });
    }
}
