//! Configuration for the `ui-htmx` BFF.
//!
//! The BFF is not an ASCOM device; this is its own small config. It targets a
//! **map of drivers** keyed by service id (`dsd-fp2`, `qhy-focuser`, …); each
//! entry says how to reach that driver's Alpaca config actions. The default
//! config carries a single local `dsd-fp2` entry so `cargo run` works with no
//! config file. The optional [`RpTarget`] additionally enables the rp-backed
//! surfaces — `/config/rp`, `/equipment`, `/stream`, and the roster-derived
//! config targets (see `docs/services/ui-htmx.md`).

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
    /// The rp orchestrator, when one is running. Enables the `/config/rp` page,
    /// the `/equipment` roster, the `/stream` activity feed, and roster-derived
    /// device config pages. `None` (the default) leaves the BFF a pure
    /// driver-config UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rp: Option<RpTarget>,
    /// Where Sentinel's dashboard/REST API lives. Absent (the default) means
    /// no restart affordances are rendered anywhere — Sentinel is optional
    /// infrastructure. See `docs/services/ui-htmx.md` §Restart via Sentinel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentinel: Option<SentinelTarget>,
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
    /// This driver's name in Sentinel's `services` map (the `{name}` in
    /// `POST /api/services/{name}/restart`). Defaults to the driver's own
    /// service id — the convention — so it only needs setting when the two
    /// differ. Meaningful only when the top-level `sentinel` block is present.
    #[serde(default)]
    pub sentinel_service: Option<String>,
}

/// How to reach Sentinel's dashboard/REST API (the restart endpoint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentinelTarget {
    #[serde(default = "default_sentinel_base_url")]
    pub base_url: String,
    /// Optional HTTP Basic credentials for an auth-enabled dashboard.
    #[serde(default)]
    pub auth: Option<DriverAuth>,
    /// Optional PEM CA path for a TLS-enabled dashboard (trusted via `rp-tls`).
    #[serde(default)]
    pub ca_cert_path: Option<PathBuf>,
}

/// HTTP Basic credentials the BFF presents to a driver.
#[derive(Clone, Serialize, Deserialize, derive_more::Debug)]
pub struct DriverAuth {
    pub username: String,
    /// Plaintext Basic-Auth password — redacted from `Debug` so it never lands
    /// in logs or test output.
    #[debug("<redacted>")]
    pub password: String,
}

/// How to reach the rp orchestrator's REST API (`/api/config`, `/api/equipment`,
/// `/api/events/subscribe`, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpTarget {
    #[serde(default = "default_rp_base_url")]
    pub base_url: String,
    /// Optional HTTP Basic credentials for an auth-enabled rp.
    #[serde(default)]
    pub auth: Option<DriverAuth>,
    /// Optional PEM CA path for a TLS-enabled rp (trusted via `rp-tls`).
    #[serde(default)]
    pub ca_cert_path: Option<PathBuf>,
}

impl Default for RpTarget {
    fn default() -> Self {
        Self {
            base_url: default_rp_base_url(),
            auth: None,
            ca_cert_path: None,
        }
    }
}

fn default_rp_base_url() -> String {
    // rp's default server port (services/rp/src/config/server.rs).
    "http://127.0.0.1:11115".to_string()
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

fn default_sentinel_base_url() -> String {
    "http://127.0.0.1:11114".to_string()
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
            sentinel_service: None,
        }
    }
}

/// Load BFF configuration from a JSON file.
pub fn load_config(path: &Path) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
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
                    "base_url": "http://127.0.0.1:11113",
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
    fn default_scaffold_round_trips_through_load_config() {
        // main() writes `Config::default()` to the XDG path on first start
        // (resolve_and_init); that serialized form must load back cleanly.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui-htmx.json");
        let scaffold = serde_json::to_string_pretty(&Config::default()).unwrap();
        std::fs::write(&path, scaffold).unwrap();
        let c = load_config(&path).unwrap();
        assert_eq!(c.server.port, 11120);
        assert!(c.drivers.0.contains_key("dsd-fp2"));
    }

    #[test]
    fn sentinel_block_absent_by_default() {
        let c = Config::default();
        assert!(c.sentinel.is_none());
        assert!(c
            .drivers
            .0
            .get("dsd-fp2")
            .unwrap()
            .sentinel_service
            .is_none());
    }

    #[test]
    fn deserialises_sentinel_block_and_service_override() {
        let json = r#"{
            "drivers": {
                "dsd-fp2": { "sentinel_service": "dsd-fp2-unit" }
            },
            "sentinel": {
                "base_url": "http://127.0.0.1:19114",
                "auth": { "username": "obs", "password": "secret" }
            }
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        let sentinel = c.sentinel.unwrap();
        assert_eq!(sentinel.base_url, "http://127.0.0.1:19114");
        assert_eq!(sentinel.auth.unwrap().username, "obs");
        assert_eq!(
            c.drivers
                .0
                .get("dsd-fp2")
                .unwrap()
                .sentinel_service
                .as_deref(),
            Some("dsd-fp2-unit")
        );
    }

    #[test]
    fn sentinel_base_url_defaults_to_dashboard_port() {
        let json = r#"{ "sentinel": {} }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.sentinel.unwrap().base_url, "http://127.0.0.1:11114");
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

    #[test]
    fn rp_target_is_absent_by_default_and_parses_when_present() {
        let c = Config::default();
        assert!(c.rp.is_none());

        let json = r#"{
            "rp": {
                "base_url": "https://pi.local:11115",
                "auth": { "username": "obs", "password": "secret" }
            }
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        let rp = c.rp.unwrap();
        assert_eq!(rp.base_url, "https://pi.local:11115");
        assert_eq!(rp.auth.unwrap().username, "obs");
    }

    #[test]
    fn rp_target_defaults_base_url_to_rp_port() {
        let c: Config = serde_json::from_str(r#"{ "rp": {} }"#).unwrap();
        assert_eq!(c.rp.unwrap().base_url, "http://127.0.0.1:11115");
    }

    #[test]
    fn rp_target_default_impl_matches_the_serde_defaults() {
        // `RpTarget::default()` and `serde_json::from_str("{}")` must agree —
        // both paths hand out the same scaffold.
        let d = RpTarget::default();
        assert_eq!(d.base_url, "http://127.0.0.1:11115");
        assert!(d.auth.is_none());
        assert!(d.ca_cert_path.is_none());
        // And an rp target round-trips through serialization unchanged.
        let json = serde_json::to_string(&d).unwrap();
        let back: RpTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(back.base_url, d.base_url);
        assert!(back.auth.is_none());
        assert!(back.ca_cert_path.is_none());
    }

    #[test]
    fn driver_auth_password_redacted_in_debug() {
        let auth = DriverAuth {
            username: "obs".to_string(),
            password: "hunter2".to_string(),
        };
        let rendered = format!("{auth:?}");
        assert!(!rendered.contains("hunter2"), "password leaked: {rendered}");
        assert!(rendered.contains("obs"));
        assert!(rendered.contains("<redacted>"));
    }
}
