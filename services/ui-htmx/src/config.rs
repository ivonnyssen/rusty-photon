//! Configuration for the `ui-htmx` BFF.
//!
//! The BFF is not an ASCOM device; this is its own small config. It targets a
//! **map of drivers** keyed by service id (`dsd-fp2`, `qhy-focuser`, …); each
//! entry says how to reach that driver's Alpaca config actions. The default
//! config carries a single local `dsd-fp2` entry so `cargo run` works with no
//! config file. Later the device list is derived from `rp`'s equipment roster
//! (see `docs/services/ui-htmx.md`).

use std::collections::BTreeMap;
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

/// The driver targets the BFF knows about, keyed by service id (the path
/// segment in `/config/{service}`). A newtype over the map so it can carry a
/// non-empty default (a single local `dsd-fp2`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Drivers(pub BTreeMap<String, DriverTarget>);

impl Default for Drivers {
    fn default() -> Self {
        let mut map = BTreeMap::new();
        map.insert("dsd-fp2".to_string(), DriverTarget::default());
        Self(map)
    }
}

/// How to reach one driver's Alpaca config actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverTarget {
    /// Display name shown in the UI; falls back to the service id when absent.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_base_url")]
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

fn default_base_url() -> String {
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
            name: None,
            base_url: default_base_url(),
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
        let dsd = c.drivers.0.get("dsd-fp2").unwrap();
        assert_eq!(dsd.base_url, "http://127.0.0.1:11119");
        assert_eq!(dsd.device_type, "covercalibrator");
        assert_eq!(dsd.device_number, 0);
        assert!(dsd.auth.is_none());
    }

    #[test]
    fn deserialises_with_defaults_for_omitted_fields() {
        let json = r#"{ "server": { "port": 9000 } }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.server.port, 9000);
        assert_eq!(c.server.bind, "127.0.0.1");
        // Omitting `drivers` falls back to the default single dsd-fp2 entry.
        assert!(c.drivers.0.contains_key("dsd-fp2"));
    }

    #[test]
    fn deserialises_multiple_drivers() {
        let json = r#"{
            "drivers": {
                "dsd-fp2": {
                    "base_url": "https://pi.local:11119",
                    "auth": { "username": "obs", "password": "secret" }
                },
                "qhy-focuser": {
                    "base_url": "http://127.0.0.1:11121",
                    "device_type": "focuser"
                }
            }
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.drivers.0.len(), 2);
        let dsd = c.drivers.0.get("dsd-fp2").unwrap();
        assert_eq!(dsd.base_url, "https://pi.local:11119");
        let auth = dsd.auth.as_ref().unwrap();
        assert_eq!(auth.username, "obs");
        assert_eq!(auth.password, "secret");
        let qhy = c.drivers.0.get("qhy-focuser").unwrap();
        assert_eq!(qhy.device_type, "focuser");
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
