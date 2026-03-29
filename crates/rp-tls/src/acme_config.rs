use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::config::expand_tilde;
use crate::error::{Result, TlsError};
use crate::permissions::set_restricted_permissions;

/// ACME configuration stored at `~/.rusty-photon/acme.json`.
///
/// This is standalone and decoupled from any service config, supporting
/// multi-machine deployments where the ACME client runs on one host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcmeConfig {
    /// ACME account email for expiry notifications.
    pub email: String,
    /// Base domain (wildcard cert issued for `*.<domain>`).
    pub domain: String,
    /// DNS provider identifier (e.g., `"cloudflare"`).
    pub dns_provider: String,
    /// Provider-specific credentials; values starting with `$` are read from
    /// environment variables.
    pub dns_credentials: HashMap<String, String>,
    /// Use Let's Encrypt staging endpoint (default: `false`).
    #[serde(default)]
    pub staging: bool,
    /// Days before expiry to trigger renewal (default: `30`).
    #[serde(default = "default_renewal_days")]
    pub renewal_days_before_expiry: u32,
    /// Shell commands to run after successful renewal.
    #[serde(default)]
    pub post_renewal_hooks: Vec<String>,
}

fn default_renewal_days() -> u32 {
    30
}

/// Default path for the ACME configuration file: `~/.rusty-photon/acme.json`.
pub fn default_acme_config_path() -> PathBuf {
    expand_tilde("~/.rusty-photon/acme.json")
}

/// Path to the ACME account credentials file within the PKI directory.
pub fn acme_account_path(pki_dir: &Path) -> PathBuf {
    pki_dir.join("acme-account.json")
}

/// Path to the ACME wildcard certificate file within the PKI directory.
pub fn acme_cert_path(pki_dir: &Path) -> PathBuf {
    pki_dir.join("certs").join("acme-cert.pem")
}

/// Path to the ACME wildcard private key file within the PKI directory.
pub fn acme_key_path(pki_dir: &Path) -> PathBuf {
    pki_dir.join("certs").join("acme-key.pem")
}

/// Load ACME configuration from a JSON file.
pub fn load_acme_config(path: &Path) -> Result<AcmeConfig> {
    debug!("Loading ACME config from {}", path.display());
    let content = std::fs::read_to_string(path)?;
    let config: AcmeConfig =
        serde_json::from_str(&content).map_err(|e| TlsError::Config(format!("{e}")))?;
    Ok(config)
}

/// Save ACME configuration to a JSON file with restricted permissions.
pub fn save_acme_config(config: &AcmeConfig, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| TlsError::Config(format!("failed to serialize ACME config: {e}")))?;
    std::fs::write(path, json)?;
    set_restricted_permissions(path)?;
    debug!("Saved ACME config to {}", path.display());
    Ok(())
}

/// Expand credential values that start with `$` by reading from environment variables.
///
/// Literal values (not starting with `$`) are passed through unchanged.
pub fn resolve_credentials(creds: &HashMap<String, String>) -> Result<HashMap<String, String>> {
    let mut resolved = HashMap::new();
    for (key, value) in creds {
        let resolved_value = if let Some(var_name) = value.strip_prefix('$') {
            std::env::var(var_name).map_err(|_| {
                TlsError::Config(format!(
                    "environment variable '{var_name}' not set (referenced by dns_credentials.{key})"
                ))
            })?
        } else {
            value.clone()
        };
        resolved.insert(key.clone(), resolved_value);
    }
    Ok(resolved)
}

/// Return the ACME directory URL for Let's Encrypt staging or production.
pub fn directory_url(staging: bool) -> &'static str {
    if staging {
        "https://acme-staging-v02.api.letsencrypt.org/directory"
    } else {
        "https://acme-v02.api.letsencrypt.org/directory"
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn acme_config_round_trip_serde() {
        let config = AcmeConfig {
            email: "user@example.com".to_string(),
            domain: "observatory.example.com".to_string(),
            dns_provider: "cloudflare".to_string(),
            dns_credentials: HashMap::from([("api_token".to_string(), "tok123".to_string())]),
            staging: true,
            renewal_days_before_expiry: 30,
            post_renewal_hooks: vec!["scp cert pi:~/".to_string()],
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AcmeConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.email, "user@example.com");
        assert_eq!(deserialized.domain, "observatory.example.com");
        assert_eq!(deserialized.dns_provider, "cloudflare");
        assert_eq!(
            deserialized.dns_credentials.get("api_token").unwrap(),
            "tok123"
        );
        assert!(deserialized.staging);
        assert_eq!(deserialized.renewal_days_before_expiry, 30);
        assert_eq!(deserialized.post_renewal_hooks.len(), 1);
    }

    #[test]
    fn acme_config_defaults() {
        let json = r#"{
            "email": "user@example.com",
            "domain": "example.com",
            "dns_provider": "cloudflare",
            "dns_credentials": {"api_token": "tok"}
        }"#;
        let config: AcmeConfig = serde_json::from_str(json).unwrap();
        assert!(!config.staging);
        assert_eq!(config.renewal_days_before_expiry, 30);
        assert!(config.post_renewal_hooks.is_empty());
    }

    #[test]
    fn resolve_credentials_expands_env_var() {
        std::env::set_var("TEST_ACME_TOKEN_XYZ", "secret123");
        let creds = HashMap::from([("api_token".to_string(), "$TEST_ACME_TOKEN_XYZ".to_string())]);
        let resolved = resolve_credentials(&creds).unwrap();
        assert_eq!(resolved.get("api_token").unwrap(), "secret123");
        std::env::remove_var("TEST_ACME_TOKEN_XYZ");
    }

    #[test]
    fn resolve_credentials_passes_through_literal() {
        let creds = HashMap::from([("api_token".to_string(), "literal-value".to_string())]);
        let resolved = resolve_credentials(&creds).unwrap();
        assert_eq!(resolved.get("api_token").unwrap(), "literal-value");
    }

    #[test]
    fn resolve_credentials_missing_env_var_returns_error() {
        let creds = HashMap::from([(
            "api_token".to_string(),
            "$NONEXISTENT_VAR_FOR_ACME_TEST".to_string(),
        )]);
        let err = resolve_credentials(&creds).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("NONEXISTENT_VAR_FOR_ACME_TEST"),
            "error should mention the missing var: {msg}"
        );
    }

    #[test]
    fn directory_url_staging() {
        let url = directory_url(true);
        assert!(url.contains("staging"), "staging URL: {url}");
    }

    #[test]
    fn directory_url_production() {
        let url = directory_url(false);
        assert!(!url.contains("staging"), "production URL: {url}");
        assert!(url.contains("acme-v02"), "production URL: {url}");
    }

    #[test]
    fn path_helpers_return_expected_paths() {
        let pki_dir = Path::new("/home/user/.rusty-photon/pki");
        assert_eq!(
            acme_account_path(pki_dir),
            PathBuf::from("/home/user/.rusty-photon/pki/acme-account.json")
        );
        assert_eq!(
            acme_cert_path(pki_dir),
            PathBuf::from("/home/user/.rusty-photon/pki/certs/acme-cert.pem")
        );
        assert_eq!(
            acme_key_path(pki_dir),
            PathBuf::from("/home/user/.rusty-photon/pki/certs/acme-key.pem")
        );
    }

    #[test]
    fn default_acme_config_path_under_home() {
        if std::env::var_os("HOME").is_some() || std::env::var_os("USERPROFILE").is_some() {
            let path = default_acme_config_path();
            assert!(
                !path.starts_with("~"),
                "tilde should be expanded: {}",
                path.display()
            );
            assert!(
                path.to_string_lossy().contains("acme.json"),
                "should end with acme.json: {}",
                path.display()
            );
        }
    }

    #[test]
    fn load_acme_config_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acme.json");
        let json = r#"{
            "email": "user@example.com",
            "domain": "example.com",
            "dns_provider": "cloudflare",
            "dns_credentials": {"api_token": "tok"}
        }"#;
        std::fs::write(&path, json).unwrap();
        let config = load_acme_config(&path).unwrap();
        assert_eq!(config.email, "user@example.com");
    }

    #[test]
    fn load_acme_config_invalid_json_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acme.json");
        std::fs::write(&path, "not json").unwrap();
        let result = load_acme_config(&path);
        assert!(result.is_err());
    }

    #[test]
    fn save_and_load_acme_config_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acme.json");

        let config = AcmeConfig {
            email: "test@example.com".to_string(),
            domain: "test.example.com".to_string(),
            dns_provider: "cloudflare".to_string(),
            dns_credentials: HashMap::from([("api_token".to_string(), "tok".to_string())]),
            staging: true,
            renewal_days_before_expiry: 15,
            post_renewal_hooks: vec![],
        };

        save_acme_config(&config, &path).unwrap();
        let loaded = load_acme_config(&path).unwrap();
        assert_eq!(loaded.email, "test@example.com");
        assert_eq!(loaded.domain, "test.example.com");
        assert!(loaded.staging);
        assert_eq!(loaded.renewal_days_before_expiry, 15);
    }
}
