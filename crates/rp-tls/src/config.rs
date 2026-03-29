use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// TLS configuration for a service endpoint.
///
/// When present in a service config, the service will serve over HTTPS.
/// When absent (`None`), the service runs plain HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Path to the PEM-encoded certificate file
    pub cert: String,
    /// Path to the PEM-encoded private key file
    pub key: String,
}

/// Expand a leading `~` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// Returns the user's home directory, or `None` if it cannot be determined.
fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

impl TlsConfig {
    /// Resolve cert and key paths, expanding `~` to the home directory.
    pub fn resolved_cert_path(&self) -> PathBuf {
        expand_tilde(&self.cert)
    }

    /// Resolve cert and key paths, expanding `~` to the home directory.
    pub fn resolved_key_path(&self) -> PathBuf {
        expand_tilde(&self.key)
    }
}

/// Default PKI directory: `~/.rusty-photon/pki`
pub fn default_pki_dir() -> PathBuf {
    expand_tilde("~/.rusty-photon/pki")
}

/// Default certs subdirectory: `~/.rusty-photon/pki/certs`
pub fn default_certs_dir() -> PathBuf {
    default_pki_dir().join("certs")
}

/// Build a `TlsConfig` pointing to the default cert locations for a service.
pub fn default_tls_config_for_service(service_name: &str) -> TlsConfig {
    let certs_dir = default_certs_dir();
    TlsConfig {
        cert: certs_dir
            .join(format!("{service_name}.pem"))
            .to_string_lossy()
            .into_owned(),
        key: certs_dir
            .join(format!("{service_name}-key.pem"))
            .to_string_lossy()
            .into_owned(),
    }
}

/// CA cert and key filenames within the PKI directory.
pub fn ca_cert_path(pki_dir: &Path) -> PathBuf {
    pki_dir.join("ca.pem")
}

pub fn ca_key_path(pki_dir: &Path) -> PathBuf {
    pki_dir.join("ca-key.pem")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn tls_config_from_json() {
        let json = r#"{"cert": "/path/to/cert.pem", "key": "/path/to/key.pem"}"#;
        let config: TlsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.cert, "/path/to/cert.pem");
        assert_eq!(config.key, "/path/to/key.pem");
    }

    #[test]
    fn optional_tls_config_defaults_to_none() {
        let json = r#"{}"#;
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            tls: Option<TlsConfig>,
        }
        let w: Wrapper = serde_json::from_str(json).unwrap();
        assert!(w.tls.is_none());
    }

    #[test]
    fn expand_tilde_with_home() {
        let expanded = expand_tilde("~/.rusty-photon/pki/ca.pem");
        // Should not start with ~ after expansion (assuming HOME is set)
        if std::env::var_os("HOME").is_some() || std::env::var_os("USERPROFILE").is_some() {
            assert!(!expanded.starts_with("~"));
        }
    }

    #[test]
    fn expand_tilde_without_tilde() {
        let path = "/absolute/path/to/cert.pem";
        assert_eq!(expand_tilde(path), PathBuf::from(path));
    }

    #[test]
    fn resolved_paths_expand_tilde() {
        let config = TlsConfig {
            cert: "~/.rusty-photon/pki/certs/rp.pem".to_string(),
            key: "~/.rusty-photon/pki/certs/rp-key.pem".to_string(),
        };
        if std::env::var_os("HOME").is_some() || std::env::var_os("USERPROFILE").is_some() {
            assert!(!config.resolved_cert_path().starts_with("~"));
            assert!(!config.resolved_key_path().starts_with("~"));
        }
    }

    #[test]
    fn default_tls_config_for_service_has_correct_paths() {
        let config = default_tls_config_for_service("ppba-driver");
        assert!(config.cert.contains("ppba-driver.pem"));
        assert!(config.key.contains("ppba-driver-key.pem"));
    }
}
