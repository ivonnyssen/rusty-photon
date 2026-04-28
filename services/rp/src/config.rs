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
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    pub data_directory: String,
    #[serde(default)]
    pub session_state_file: String,
    #[serde(default)]
    pub file_naming_pattern: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EquipmentConfig {
    #[serde(default)]
    pub cameras: Vec<CameraConfig>,
    #[serde(default)]
    pub mount: Value,
    #[serde(default)]
    pub focusers: Vec<Value>,
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
    }

    #[test]
    fn load_config_missing_file() {
        let err = load_config(Path::new("/nonexistent/rp/config.json")).unwrap_err();
        assert!(err.to_string().contains("failed to read config file"));
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
