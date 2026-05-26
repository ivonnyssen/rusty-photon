//! Configuration for the `ui-htmx` BFF.
//!
//! The BFF is not an ASCOM device; this is its own small config. Phase 2 targets
//! a single hard-coded driver (`dsd-fp2`); later the device list is derived from
//! `rp`'s equipment roster (see `docs/services/ui-htmx.md`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Top-level BFF configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub drivers: Drivers,
}

/// Where the BFF itself listens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

/// The driver targets the BFF knows about. Phase 2 has exactly one.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Drivers {
    #[serde(rename = "dsd-fp2", default)]
    pub dsd_fp2: DriverTarget,
}

/// How to reach one driver's Alpaca config actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverTarget {
    #[serde(default = "default_dsd_fp2_base_url")]
    pub base_url: String,
    #[serde(default = "default_device_type")]
    pub device_type: String,
    #[serde(default)]
    pub device_number: u32,
    /// Optional HTTP Basic credentials for an auth-enabled driver.
    #[serde(default)]
    pub auth: Option<DriverAuth>,
    /// Optional PEM CA path for a TLS-enabled driver (trusted via `rp-tls`).
    #[serde(default)]
    pub ca_cert_path: Option<PathBuf>,
}

/// HTTP Basic credentials the BFF presents to a driver.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverAuth {
    pub username: String,
    pub password: String,
}

fn default_bind() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    11120
}

fn default_dsd_fp2_base_url() -> String {
    "http://127.0.0.1:11119".to_string()
}

fn default_device_type() -> String {
    "covercalibrator".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            port: default_port(),
        }
    }
}

impl Default for DriverTarget {
    fn default() -> Self {
        Self {
            base_url: default_dsd_fp2_base_url(),
            device_type: default_device_type(),
            device_number: 0,
            auth: None,
            ca_cert_path: None,
        }
    }
}

/// Load BFF configuration from a JSON file.
pub fn load_config(path: &Path) -> Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config = serde_json::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn defaults_target_local_dsd_fp2() {
        let c = Config::default();
        assert_eq!(c.server.bind, "127.0.0.1");
        assert_eq!(c.server.port, 11120);
        assert_eq!(c.drivers.dsd_fp2.base_url, "http://127.0.0.1:11119");
        assert_eq!(c.drivers.dsd_fp2.device_type, "covercalibrator");
        assert_eq!(c.drivers.dsd_fp2.device_number, 0);
        assert!(c.drivers.dsd_fp2.auth.is_none());
    }

    #[test]
    fn deserialises_with_defaults_for_omitted_fields() {
        let json = r#"{ "server": { "port": 9000 } }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.server.port, 9000);
        assert_eq!(c.server.bind, "127.0.0.1");
        // The driver target falls back entirely to its defaults.
        assert_eq!(c.drivers.dsd_fp2.base_url, "http://127.0.0.1:11119");
    }

    #[test]
    fn deserialises_driver_auth_and_url() {
        let json = r#"{
            "drivers": {
                "dsd-fp2": {
                    "base_url": "https://pi.local:11119",
                    "auth": { "username": "obs", "password": "secret" }
                }
            }
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.drivers.dsd_fp2.base_url, "https://pi.local:11119");
        let auth = c.drivers.dsd_fp2.auth.unwrap();
        assert_eq!(auth.username, "obs");
        assert_eq!(auth.password, "secret");
    }

    #[test]
    fn load_config_reads_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui-htmx.json");
        std::fs::write(&path, r#"{ "server": { "port": 8080 } }"#).unwrap();
        let c = load_config(&path).unwrap();
        assert_eq!(c.server.port, 8080);
    }

    #[test]
    fn load_config_missing_file_errors() {
        load_config(Path::new("/tmp/ui_htmx_nonexistent_4242.json")).unwrap_err();
    }
}
