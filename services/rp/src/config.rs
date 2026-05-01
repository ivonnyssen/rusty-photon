use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Result, RpError};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub session: SessionConfig,
    pub equipment: EquipmentConfig,
    #[serde(default)]
    pub plugins: Vec<Value>,
    #[serde(default)]
    pub targets: Value,
    #[serde(default)]
    pub planner: Value,
    #[serde(default)]
    pub safety: Value,
    #[serde(default)]
    pub imaging: ImagingConfig,
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    pub data_directory: String,
    #[serde(default)]
    pub session_state_file: String,
    /// Optional template for capture filenames. `None` is the default and
    /// produces filenames of the form `<doc_uuid_8>.fits` plus a matching
    /// `.json` sidecar — fully self-identifying via the UUID-8 suffix that
    /// drives the disk-fallback resolution path. When set, the template is
    /// reserved for a future token resolver (planner/capture context feeding
    /// `{target}` / `{filter}` / etc.); until that lands `capture` ignores
    /// the value and writes `<doc_uuid_8>.fits` regardless. See
    /// `docs/services/rp.md` (Persistence) and Phase 7 of
    /// `docs/plans/image-evaluation-tools.md`.
    #[serde(default)]
    pub file_naming_pattern: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EquipmentConfig {
    #[serde(default)]
    pub cameras: Vec<CameraConfig>,
    #[serde(default)]
    pub mount: Value,
    #[serde(default)]
    pub focusers: Vec<FocuserConfig>,
    #[serde(default)]
    pub filter_wheels: Vec<FilterWheelConfig>,
    #[serde(default)]
    pub cover_calibrators: Vec<CoverCalibratorConfig>,
    #[serde(default)]
    pub safety_monitors: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CameraConfig {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_type: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default)]
    pub cooler_target_c: Option<f64>,
    #[serde(default)]
    pub gain: Option<i32>,
    #[serde(default)]
    pub offset: Option<i32>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FocuserConfig {
    pub id: String,
    #[serde(default)]
    pub camera_id: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    /// Operator-supplied lower bound for `move_focuser` validation. The
    /// device-reported `max_step` is the hardware ceiling; these fields
    /// let the operator enforce a tighter safe-travel range.
    #[serde(default)]
    pub min_position: Option<i32>,
    /// Operator-supplied upper bound for `move_focuser` validation.
    #[serde(default)]
    pub max_position: Option<i32>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilterWheelConfig {
    pub id: String,
    #[serde(default)]
    pub camera_id: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default)]
    pub filters: Vec<String>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoverCalibratorConfig {
    pub id: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    /// Poll interval when waiting for cover/calibrator state changes (default `"3s"`)
    #[serde(
        default = "default_cover_calibrator_poll_interval",
        with = "humantime_serde"
    )]
    pub poll_interval: Duration,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

fn default_cover_calibrator_poll_interval() -> Duration {
    Duration::from_secs(3)
}

/// Image cache + future analysis-tool tuning. Pi-5-friendly defaults.
#[derive(Debug, Clone, Deserialize)]
pub struct ImagingConfig {
    #[serde(default = "default_cache_max_mib")]
    pub cache_max_mib: usize,
    #[serde(default = "default_cache_max_images")]
    pub cache_max_images: usize,
}

impl Default for ImagingConfig {
    fn default() -> Self {
        Self {
            cache_max_mib: default_cache_max_mib(),
            cache_max_images: default_cache_max_images(),
        }
    }
}

fn default_cache_max_mib() -> usize {
    1024
}

fn default_cache_max_images() -> usize {
    8
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_bind")]
    pub bind_address: String,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
    #[serde(default)]
    pub auth: Option<rp_auth::config::AuthConfig>,
}

fn default_port() -> u16 {
    11115
}

fn default_bind() -> String {
    "127.0.0.1".to_string()
}

pub fn load_config(path: &Path) -> Result<Config> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        RpError::Config(format!(
            "failed to read config file '{}': {}",
            path.display(),
            e
        ))
    })?;
    serde_json::from_str(&contents).map_err(|e| {
        RpError::Config(format!(
            "failed to parse config file '{}': {}",
            path.display(),
            e
        ))
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const MINIMAL_CONFIG_JSON: &str = r#"{
        "session": {"data_directory": "/tmp/rp-test"},
        "equipment": {},
        "server": {}
    }"#;

    #[test]
    fn load_config_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.session.data_directory, "/tmp/rp-test");
        assert_eq!(config.server.port, 11115);
        assert_eq!(config.server.bind_address, "127.0.0.1");
        assert_eq!(config.imaging.cache_max_mib, 1024);
        assert_eq!(config.imaging.cache_max_images, 8);
    }

    #[test]
    fn imaging_config_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "imaging": {"cache_max_mib": 256, "cache_max_images": 4},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.imaging.cache_max_mib, 256);
        assert_eq!(config.imaging.cache_max_images, 4);
    }

    #[test]
    fn load_config_missing_file() {
        let err = load_config(Path::new("/nonexistent/rp/config.json")).unwrap_err();
        assert!(err.to_string().contains("failed to read config file"));
    }

    #[test]
    fn file_naming_pattern_defaults_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert!(
            config.session.file_naming_pattern.is_none(),
            "omitted file_naming_pattern must deserialize to None"
        );
    }

    #[test]
    fn file_naming_pattern_round_trips_when_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {
                    "data_directory": "/tmp/rp-test",
                    "file_naming_pattern": "{target}_{filter}"
                },
                "equipment": {},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(
            config.session.file_naming_pattern.as_deref(),
            Some("{target}_{filter}")
        );
    }

    #[test]
    fn focuser_config_minimal_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "focusers": [
                        {
                            "id": "main-focuser",
                            "alpaca_url": "http://localhost:11113"
                        }
                    ]
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.equipment.focusers.len(), 1);
        let f = &config.equipment.focusers[0];
        assert_eq!(f.id, "main-focuser");
        assert_eq!(f.alpaca_url, "http://localhost:11113");
        assert_eq!(f.device_number, 0);
        assert!(f.min_position.is_none());
        assert!(f.max_position.is_none());
        assert!(f.auth.is_none());
    }

    #[test]
    fn focuser_config_with_bounds_and_auth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "focusers": [
                        {
                            "id": "main-focuser",
                            "camera_id": "main-cam",
                            "alpaca_url": "http://localhost:11113",
                            "device_number": 2,
                            "min_position": 0,
                            "max_position": 100000,
                            "auth": {"username": "u", "password": "p"}
                        }
                    ]
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let f = &config.equipment.focusers[0];
        assert_eq!(f.camera_id, "main-cam");
        assert_eq!(f.device_number, 2);
        assert_eq!(f.min_position, Some(0));
        assert_eq!(f.max_position, Some(100000));
        let auth = f.auth.as_ref().unwrap();
        assert_eq!(auth.username, "u");
        assert_eq!(auth.password, "p");
    }

    #[test]
    fn load_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "not valid json").unwrap();

        let err = load_config(&path).unwrap_err();
        assert!(err.to_string().contains("failed to parse config file"));
    }
}
