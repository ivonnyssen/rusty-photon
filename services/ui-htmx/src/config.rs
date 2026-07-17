//! Configuration for the `ui-htmx` BFF.
//!
//! The BFF is not an ASCOM device; this is its own small config, and since
//! doctor-plan D3 its source of truth is **rp's roster** (ADR-016 decision
//! 9): the default config is the listening port plus the single-box
//! [`RpTarget`], and the config targets come from the roster at request
//! time. The `drivers` map survives as an empty-by-default **override** —
//! a third-party device rp does not manage, or a driver needing its own
//! credentials/CA — keyed by service id (`dsd-fp2`, `qhy-focuser`, …).
//! Writing `"rp": null` explicitly leaves the BFF the pure driver-config UI
//! (see `docs/services/ui-htmx.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use rusty_photon_server_config::ServerConfig;
use serde::{Deserialize, Serialize};

/// Top-level BFF configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_server")]
    pub server: ServerConfig,
    #[serde(default)]
    pub drivers: Drivers,
    /// The rp orchestrator — the roster source of truth. Enables the
    /// `/config/rp` page, the `/equipment` roster, the `/stream` activity
    /// feed, and roster-derived device config pages. An absent key defaults
    /// to rp on this box (`http://127.0.0.1:11115`); an **explicit `null`**
    /// runs without rp (pure driver-config UI), and is serialized back as
    /// `null` so the choice survives a rewrite.
    #[serde(default = "default_rp_target")]
    pub rp: Option<RpTarget>,
    /// Where Sentinel's dashboard/REST API lives. Absent (the default) means
    /// no restart affordances are rendered anywhere — Sentinel is optional
    /// infrastructure. See `docs/services/ui-htmx.md` §Restart via Sentinel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentinel: Option<SentinelTarget>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: default_server(),
            drivers: Drivers::default(),
            rp: default_rp_target(),
            sentinel: None,
        }
    }
}

/// The single-box default: rp on localhost at its default port.
fn default_rp_target() -> Option<RpTarget> {
    Some(RpTarget::default())
}

/// The BFF's default `server` block when the config file omits it: port 11120
/// on all interfaces, plain HTTP.
fn default_server() -> ServerConfig {
    ServerConfig::new(11120)
}

/// The driver override targets, keyed by service id (the path segment in
/// `/config/{service}`). Empty by default — rp's roster is the source of
/// truth, and doctor's `--fix` never generates entries here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Drivers(pub BTreeMap<String, DriverTarget>);

/// How to reach one driver's Alpaca config actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// How to reach Sentinel's dashboard/REST API (the restart endpoint).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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

fn default_base_url() -> String {
    "http://127.0.0.1:11119".to_string()
}

fn default_sentinel_base_url() -> String {
    "http://127.0.0.1:11114".to_string()
}

fn default_device_type() -> String {
    "covercalibrator".to_string()
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
    fn defaults_are_roster_first() {
        // The single-box default: no driver overrides, rp on localhost —
        // rp's roster is the source of truth (doctor-plan D3).
        let c = Config::default();
        assert_eq!(c.server.bind_address.to_string(), "0.0.0.0");
        assert_eq!(c.server.port, 11120);
        assert!(c.server.tls.is_none());
        assert!(c.server.auth.is_none());
        assert!(c.drivers.0.is_empty());
        assert_eq!(c.rp.unwrap().base_url, "http://127.0.0.1:11115");
    }

    #[test]
    fn deserialises_with_defaults_for_omitted_fields() {
        let json = r#"{ "server": { "port": 9000 } }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.server.port, 9000);
        assert_eq!(c.server.bind_address.to_string(), "0.0.0.0");
        // Omitting `drivers` means no overrides; omitting `rp` means the
        // single-box default.
        assert!(c.drivers.0.is_empty());
        assert_eq!(c.rp.unwrap().base_url, "http://127.0.0.1:11115");
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
        assert!(c.drivers.0.is_empty());
        assert!(c.rp.is_some(), "the scaffolded rp target must load back");
    }

    #[test]
    fn sentinel_block_absent_by_default() {
        let c = Config::default();
        assert!(c.sentinel.is_none());
    }

    #[test]
    fn deserialises_sentinel_block() {
        let json = r#"{
            "sentinel": {
                "base_url": "http://127.0.0.1:19114",
                "auth": { "username": "obs", "password": "secret" }
            }
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        let sentinel = c.sentinel.unwrap();
        assert_eq!(sentinel.base_url, "http://127.0.0.1:19114");
        assert_eq!(sentinel.auth.unwrap().username, "obs");
    }

    #[test]
    fn retired_sentinel_service_field_is_rejected() {
        // Sentinel discovers services from the platform service manager now;
        // the per-driver name override is gone, and deny_unknown_fields makes
        // an old config carrying it fail loudly rather than silently ignore it.
        let json = r#"{"drivers": {"dsd-fp2": {"sentinel_service": "dsd-fp2-unit"}}}"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("sentinel_service"), "{err}");
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
    fn rp_target_null_is_explicit_and_survives_a_round_trip() {
        // Absent key -> the single-box default; explicit null -> no rp, and
        // the choice must survive serialization (rp is not skipped when
        // None, else a rewrite would silently resurrect the default).
        let c: Config = serde_json::from_str(r#"{ "rp": null }"#).unwrap();
        assert!(c.rp.is_none());
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert!(back.rp.is_none(), "explicit null must round-trip");
    }

    #[test]
    fn rp_target_parses_when_present() {
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
    fn config_rejects_unknown_top_level_field() {
        let err = serde_json::from_str::<Config>(r#"{"plugins": []}"#).unwrap_err();
        assert!(err.to_string().contains("plugins"), "{err}");
    }

    #[test]
    fn driver_target_rejects_unknown_field() {
        let json = r#"{"drivers": {"dsd-fp2": {"poll_interval": "5s"}}}"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("poll_interval"), "{err}");
    }

    #[test]
    fn sentinel_target_rejects_unknown_field() {
        let json = r#"{"sentinel": {"webhook_url": "http://x"}}"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("webhook_url"), "{err}");
    }

    #[test]
    fn driver_auth_rejects_unknown_field() {
        let json = r#"{"rp": {"auth": {"username": "u", "password": "p", "realm": "x"}}}"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("realm"), "{err}");
    }

    #[test]
    fn rp_target_rejects_unknown_field() {
        let json = r#"{"rp": {"discovery_port": 32227}}"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("discovery_port"), "{err}");
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod doctor_toml_parity {
    use rusty_photon_server_config::doctor_toml::{parse, ServerClass};

    use super::Config;

    /// `pkg/doctor.toml` is this service's catalog entry for
    /// `rusty-photon-doctor` and must match the config defaults
    /// (docs/services/doctor.md §The derived catalog).
    #[test]
    fn pkg_doctor_toml_matches_config_defaults() {
        let meta = parse(include_str!("../pkg/doctor.toml")).unwrap();
        assert_eq!(meta.port, Config::default().server.port);
        assert_eq!(meta.class, ServerClass::Core);
    }
}
