use std::collections::HashMap;
use std::fmt::Debug;

use async_trait::async_trait;
use tracing::debug;

use crate::error::{Result, TlsError};

/// Trait for DNS providers that can create and delete TXT records
/// for ACME DNS-01 challenge validation.
#[async_trait]
pub trait DnsProvider: Send + Sync + Debug {
    /// Create a TXT record for the ACME challenge.
    async fn create_txt_record(&self, fqdn: &str, value: &str) -> Result<()>;

    /// Remove the TXT record after validation completes.
    async fn delete_txt_record(&self, fqdn: &str) -> Result<()>;
}

/// Cloudflare DNS provider using the official `cloudflare` crate.
///
/// The zone ID is resolved once at construction time by looking up
/// the zone matching the provided domain.
pub struct CloudflareDnsProvider {
    client: cloudflare::framework::client::async_api::Client,
    zone_id: String,
}

impl Debug for CloudflareDnsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudflareDnsProvider")
            .field("zone_id", &self.zone_id)
            .finish_non_exhaustive()
    }
}

impl CloudflareDnsProvider {
    /// Create a new Cloudflare DNS provider.
    ///
    /// Connects to the Cloudflare API with the given token and resolves
    /// the zone ID for the specified domain.
    pub async fn new(api_token: &str, domain: &str) -> Result<Self> {
        use cloudflare::endpoints::zones::zone::{ListZones, ListZonesParams};
        use cloudflare::framework::auth::Credentials;
        use cloudflare::framework::client::ClientConfig;
        use cloudflare::framework::Environment;

        let credentials = Credentials::UserAuthToken {
            token: api_token.to_string(),
        };
        let client = cloudflare::framework::client::async_api::Client::new(
            credentials,
            ClientConfig::default(),
            Environment::Production,
        )
        .map_err(|e| TlsError::DnsProvider(format!("failed to create Cloudflare client: {e}")))?;

        // Look up zone ID by domain name
        let zones_response = client
            .request(&ListZones {
                params: ListZonesParams {
                    name: Some(domain.to_string()),
                    ..Default::default()
                },
            })
            .await
            .map_err(|e| TlsError::DnsProvider(format!("failed to list zones: {e}")))?;

        let zone = zones_response.result.first().ok_or_else(|| {
            TlsError::DnsProvider(format!("no Cloudflare zone found for domain '{domain}'"))
        })?;

        debug!(
            "Resolved Cloudflare zone ID '{}' for domain '{}'",
            zone.id, domain
        );

        Ok(Self {
            client,
            zone_id: zone.id.clone(),
        })
    }
}

#[async_trait]
impl DnsProvider for CloudflareDnsProvider {
    async fn create_txt_record(&self, fqdn: &str, value: &str) -> Result<()> {
        use cloudflare::endpoints::dns::dns::{CreateDnsRecord, CreateDnsRecordParams, DnsContent};

        debug!("Creating TXT record: {} = {}", fqdn, value);

        self.client
            .request(&CreateDnsRecord {
                zone_identifier: &self.zone_id,
                params: CreateDnsRecordParams {
                    name: fqdn,
                    content: DnsContent::TXT {
                        content: value.to_string(),
                    },
                    ttl: Some(60),
                    priority: None,
                    proxied: Some(false),
                },
            })
            .await
            .map_err(|e| TlsError::DnsProvider(format!("failed to create TXT record: {e}")))?;

        debug!("Created TXT record for {}", fqdn);
        Ok(())
    }

    async fn delete_txt_record(&self, fqdn: &str) -> Result<()> {
        use cloudflare::endpoints::dns::dns::{
            DeleteDnsRecord, DnsContent, ListDnsRecords, ListDnsRecordsParams,
        };

        debug!("Deleting TXT records for {}", fqdn);

        // List TXT records matching the FQDN
        let records_response = self
            .client
            .request(&ListDnsRecords {
                zone_identifier: &self.zone_id,
                params: ListDnsRecordsParams {
                    name: Some(fqdn.to_string()),
                    record_type: Some(DnsContent::TXT {
                        content: String::new(),
                    }),
                    ..Default::default()
                },
            })
            .await
            .map_err(|e| TlsError::DnsProvider(format!("failed to list TXT records: {e}")))?;

        let records = &records_response.result;
        if records.is_empty() {
            debug!("No TXT records found for {} (already cleaned up)", fqdn);
            return Ok(());
        }

        // Delete each matching record
        for record in records {
            self.client
                .request(&DeleteDnsRecord {
                    zone_identifier: &self.zone_id,
                    identifier: &record.id,
                })
                .await
                .map_err(|e| {
                    TlsError::DnsProvider(format!("failed to delete TXT record {}: {e}", record.id))
                })?;
            debug!("Deleted TXT record {} for {}", record.id, fqdn);
        }

        Ok(())
    }
}

/// Build a DNS provider from a provider name and credentials.
///
/// Currently supports:
/// - `"cloudflare"` — requires `api_token` in credentials
pub async fn build_dns_provider(
    provider_name: &str,
    credentials: &HashMap<String, String>,
    domain: &str,
) -> Result<Box<dyn DnsProvider>> {
    match provider_name {
        "cloudflare" => {
            let api_token = credentials.get("api_token").ok_or_else(|| {
                TlsError::Config(
                    "Cloudflare DNS provider requires 'api_token' in dns_credentials".to_string(),
                )
            })?;
            let provider = CloudflareDnsProvider::new(api_token, domain).await?;
            Ok(Box::new(provider))
        }
        other => Err(TlsError::Config(format!(
            "unsupported DNS provider: '{other}'. Supported providers: cloudflare"
        ))),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn trait_object_safety() {
        // Verify DnsProvider is object-safe (compiles if so)
        fn _assert_object_safe(_: &dyn DnsProvider) {}
    }

    #[tokio::test]
    async fn build_dns_provider_unknown_provider_returns_error() {
        let creds = HashMap::from([("api_token".to_string(), "tok".to_string())]);
        let err = build_dns_provider("unknown", &creds, "example.com")
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported DNS provider"),
            "error should mention unsupported provider: {msg}"
        );
    }

    #[tokio::test]
    async fn build_dns_provider_cloudflare_missing_token_returns_error() {
        let creds = HashMap::new();
        let err = build_dns_provider("cloudflare", &creds, "example.com")
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("api_token"),
            "error should mention missing api_token: {msg}"
        );
    }
}
