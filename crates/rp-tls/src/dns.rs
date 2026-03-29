use std::collections::HashMap;
use std::fmt::Debug;

use async_trait::async_trait;
use tracing::debug;

use crate::error::{Result, TlsError};

/// Trait for DNS providers that can create and delete TXT records
/// for ACME DNS-01 challenge validation.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait DnsProvider: Send + Sync {
    /// Create a TXT record for the ACME challenge.
    async fn create_txt_record(&self, fqdn: &str, value: &str) -> Result<()>;

    /// Remove the TXT record after validation completes.
    async fn delete_txt_record(&self, fqdn: &str) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Cloudflare API abstraction (mockable boundary)
// ---------------------------------------------------------------------------

/// Minimal zone info returned by zone lookup.
#[derive(Debug, Clone)]
pub struct ZoneInfo {
    pub id: String,
}

/// Minimal DNS record info returned by record listing.
#[derive(Debug, Clone)]
pub struct RecordInfo {
    pub id: String,
}

/// Trait abstracting Cloudflare API calls so `CloudflareDnsProvider` can be
/// tested without real HTTP calls.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait CloudflareApi: Send + Sync {
    /// List zones matching the given domain name.
    async fn list_zones(&self, domain: String) -> Result<Vec<ZoneInfo>>;

    /// Create a TXT DNS record in the given zone.
    async fn create_txt_record_api(
        &self,
        zone_id: String,
        name: String,
        content: String,
    ) -> Result<()>;

    /// List TXT DNS records matching the given name in the zone.
    async fn list_txt_records(&self, zone_id: String, name: String) -> Result<Vec<RecordInfo>>;

    /// Delete a DNS record by ID.
    async fn delete_record(&self, zone_id: String, record_id: String) -> Result<()>;
}

/// Real Cloudflare API implementation using the `cloudflare` crate.
pub struct RealCloudflareApi {
    client: cloudflare::framework::client::async_api::Client,
}

impl RealCloudflareApi {
    pub fn new(api_token: &str) -> Result<Self> {
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

        Ok(Self { client })
    }
}

#[async_trait]
impl CloudflareApi for RealCloudflareApi {
    async fn list_zones(&self, domain: String) -> Result<Vec<ZoneInfo>> {
        use cloudflare::endpoints::zones::zone::{ListZones, ListZonesParams};

        let response = self
            .client
            .request(&ListZones {
                params: ListZonesParams {
                    name: Some(domain),
                    ..Default::default()
                },
            })
            .await
            .map_err(|e| TlsError::DnsProvider(format!("failed to list zones: {e}")))?;

        Ok(response
            .result
            .into_iter()
            .map(|z| ZoneInfo { id: z.id })
            .collect())
    }

    async fn create_txt_record_api(
        &self,
        zone_id: String,
        name: String,
        content: String,
    ) -> Result<()> {
        use cloudflare::endpoints::dns::dns::{CreateDnsRecord, CreateDnsRecordParams, DnsContent};

        self.client
            .request(&CreateDnsRecord {
                zone_identifier: &zone_id,
                params: CreateDnsRecordParams {
                    name: &name,
                    content: DnsContent::TXT { content },
                    ttl: Some(60),
                    priority: None,
                    proxied: Some(false),
                },
            })
            .await
            .map_err(|e| TlsError::DnsProvider(format!("failed to create TXT record: {e}")))?;

        Ok(())
    }

    async fn list_txt_records(&self, zone_id: String, name: String) -> Result<Vec<RecordInfo>> {
        use cloudflare::endpoints::dns::dns::{DnsContent, ListDnsRecords, ListDnsRecordsParams};

        let response = self
            .client
            .request(&ListDnsRecords {
                zone_identifier: &zone_id,
                params: ListDnsRecordsParams {
                    name: Some(name),
                    record_type: Some(DnsContent::TXT {
                        content: String::new(),
                    }),
                    ..Default::default()
                },
            })
            .await
            .map_err(|e| TlsError::DnsProvider(format!("failed to list TXT records: {e}")))?;

        Ok(response
            .result
            .into_iter()
            .map(|r| RecordInfo { id: r.id })
            .collect())
    }

    async fn delete_record(&self, zone_id: String, record_id: String) -> Result<()> {
        use cloudflare::endpoints::dns::dns::DeleteDnsRecord;

        self.client
            .request(&DeleteDnsRecord {
                zone_identifier: &zone_id,
                identifier: &record_id,
            })
            .await
            .map_err(|e| {
                TlsError::DnsProvider(format!("failed to delete TXT record {record_id}: {e}"))
            })?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CloudflareDnsProvider (testable via CloudflareApi trait)
// ---------------------------------------------------------------------------

/// Cloudflare DNS provider using the `CloudflareApi` abstraction.
///
/// The zone ID is resolved once at construction time by looking up
/// the zone matching the provided domain.
pub struct CloudflareDnsProvider {
    api: Box<dyn CloudflareApi>,
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
        let api = RealCloudflareApi::new(api_token)?;
        Self::with_api(Box::new(api), domain).await
    }

    /// Create a provider with a custom `CloudflareApi` implementation.
    /// Used internally and for testing.
    async fn with_api(api: Box<dyn CloudflareApi>, domain: &str) -> Result<Self> {
        let zones = api.list_zones(domain.to_string()).await?;

        let zone = zones.first().ok_or_else(|| {
            TlsError::DnsProvider(format!("no Cloudflare zone found for domain '{domain}'"))
        })?;

        debug!(
            "Resolved Cloudflare zone ID '{}' for domain '{}'",
            zone.id, domain
        );

        Ok(Self {
            api,
            zone_id: zone.id.clone(),
        })
    }
}

#[async_trait]
impl DnsProvider for CloudflareDnsProvider {
    async fn create_txt_record(&self, fqdn: &str, value: &str) -> Result<()> {
        debug!("Creating TXT record: {} = {}", fqdn, value);
        self.api
            .create_txt_record_api(self.zone_id.clone(), fqdn.to_string(), value.to_string())
            .await?;
        debug!("Created TXT record for {}", fqdn);
        Ok(())
    }

    async fn delete_txt_record(&self, fqdn: &str) -> Result<()> {
        debug!("Deleting TXT records for {}", fqdn);

        let records = self
            .api
            .list_txt_records(self.zone_id.clone(), fqdn.to_string())
            .await?;

        if records.is_empty() {
            debug!("No TXT records found for {} (already cleaned up)", fqdn);
            return Ok(());
        }

        for record in &records {
            self.api
                .delete_record(self.zone_id.clone(), record.id.clone())
                .await?;
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
        fn _assert_object_safe(_: &dyn DnsProvider) {}
    }

    // -----------------------------------------------------------------------
    // build_dns_provider tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn build_dns_provider_unknown_provider_returns_error() {
        let creds = HashMap::from([("api_token".to_string(), "tok".to_string())]);
        let result = build_dns_provider("unknown", &creds, "example.com").await;
        assert!(result.is_err());
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("unsupported DNS provider"),
            "error should mention unsupported provider: {msg}"
        );
    }

    #[tokio::test]
    async fn build_dns_provider_cloudflare_missing_token_returns_error() {
        let creds = HashMap::new();
        let result = build_dns_provider("cloudflare", &creds, "example.com").await;
        assert!(result.is_err());
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("api_token"),
            "error should mention missing api_token: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // CloudflareDnsProvider tests (via MockCloudflareApi)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cloudflare_provider_resolves_zone_id() {
        let mut mock_api = MockCloudflareApi::new();
        mock_api.expect_list_zones().returning(|_| {
            Ok(vec![ZoneInfo {
                id: "zone-123".to_string(),
            }])
        });

        let provider = CloudflareDnsProvider::with_api(Box::new(mock_api), "example.com")
            .await
            .unwrap();
        assert_eq!(provider.zone_id, "zone-123");
    }

    #[tokio::test]
    async fn cloudflare_provider_no_zone_found_returns_error() {
        let mut mock_api = MockCloudflareApi::new();
        mock_api.expect_list_zones().returning(|_| Ok(vec![]));

        let err = CloudflareDnsProvider::with_api(Box::new(mock_api), "missing.com")
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no Cloudflare zone found"), "error: {msg}");
    }

    #[tokio::test]
    async fn cloudflare_provider_creates_txt_record() {
        let mut mock_api = MockCloudflareApi::new();
        mock_api.expect_list_zones().returning(|_| {
            Ok(vec![ZoneInfo {
                id: "zone-abc".to_string(),
            }])
        });
        mock_api
            .expect_create_txt_record_api()
            .withf(|zone_id, name, content| {
                zone_id == "zone-abc"
                    && name == "_acme-challenge.example.com"
                    && content == "dns-value-xyz"
            })
            .returning(|_, _, _| Ok(()));

        let provider = CloudflareDnsProvider::with_api(Box::new(mock_api), "example.com")
            .await
            .unwrap();
        provider
            .create_txt_record("_acme-challenge.example.com", "dns-value-xyz")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cloudflare_provider_create_record_error_propagates() {
        let mut mock_api = MockCloudflareApi::new();
        mock_api.expect_list_zones().returning(|_| {
            Ok(vec![ZoneInfo {
                id: "zone-1".to_string(),
            }])
        });
        mock_api
            .expect_create_txt_record_api()
            .returning(|_, _, _| Err(TlsError::DnsProvider("API error".to_string())));

        let provider = CloudflareDnsProvider::with_api(Box::new(mock_api), "example.com")
            .await
            .unwrap();
        let err = provider
            .create_txt_record("_acme-challenge.example.com", "val")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("API error"), "error: {err}");
    }

    #[tokio::test]
    async fn cloudflare_provider_deletes_matching_records() {
        let mut mock_api = MockCloudflareApi::new();
        mock_api.expect_list_zones().returning(|_| {
            Ok(vec![ZoneInfo {
                id: "zone-del".to_string(),
            }])
        });
        mock_api.expect_list_txt_records().returning(|_, _| {
            Ok(vec![
                RecordInfo {
                    id: "rec-1".to_string(),
                },
                RecordInfo {
                    id: "rec-2".to_string(),
                },
            ])
        });
        mock_api
            .expect_delete_record()
            .times(2)
            .returning(|_, _| Ok(()));

        let provider = CloudflareDnsProvider::with_api(Box::new(mock_api), "example.com")
            .await
            .unwrap();
        provider
            .delete_txt_record("_acme-challenge.example.com")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cloudflare_provider_delete_no_records_is_ok() {
        let mut mock_api = MockCloudflareApi::new();
        mock_api.expect_list_zones().returning(|_| {
            Ok(vec![ZoneInfo {
                id: "zone-empty".to_string(),
            }])
        });
        mock_api
            .expect_list_txt_records()
            .returning(|_, _| Ok(vec![]));
        mock_api.expect_delete_record().never();

        let provider = CloudflareDnsProvider::with_api(Box::new(mock_api), "example.com")
            .await
            .unwrap();
        provider
            .delete_txt_record("_acme-challenge.example.com")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cloudflare_provider_delete_record_error_propagates() {
        let mut mock_api = MockCloudflareApi::new();
        mock_api.expect_list_zones().returning(|_| {
            Ok(vec![ZoneInfo {
                id: "zone-err".to_string(),
            }])
        });
        mock_api.expect_list_txt_records().returning(|_, _| {
            Ok(vec![RecordInfo {
                id: "rec-x".to_string(),
            }])
        });
        mock_api
            .expect_delete_record()
            .returning(|_, _| Err(TlsError::DnsProvider("delete failed".to_string())));

        let provider = CloudflareDnsProvider::with_api(Box::new(mock_api), "example.com")
            .await
            .unwrap();
        let err = provider
            .delete_txt_record("_acme-challenge.example.com")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("delete failed"), "error: {err}");
    }

    #[tokio::test]
    async fn cloudflare_provider_list_records_error_propagates() {
        let mut mock_api = MockCloudflareApi::new();
        mock_api.expect_list_zones().returning(|_| {
            Ok(vec![ZoneInfo {
                id: "zone-le".to_string(),
            }])
        });
        mock_api
            .expect_list_txt_records()
            .returning(|_, _| Err(TlsError::DnsProvider("list failed".to_string())));

        let provider = CloudflareDnsProvider::with_api(Box::new(mock_api), "example.com")
            .await
            .unwrap();
        let err = provider
            .delete_txt_record("_acme-challenge.example.com")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("list failed"), "error: {err}");
    }
}
