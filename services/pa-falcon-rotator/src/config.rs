//! Configuration types for the pa-falcon-rotator driver

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub serial: SerialConfig,
    pub server: ServerConfig,
    pub rotator: RotatorConfig,
    pub switch: SwitchConfig,
}

/// Serial port configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerialConfig {
    pub port: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    #[serde(default = "default_timeout", with = "humantime_serde")]
    pub timeout: Duration,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    #[serde(default = "default_discovery_port")]
    pub discovery_port: Option<u16>,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
    #[serde(default)]
    pub auth: Option<rp_auth::config::AuthConfig>,
}

fn default_discovery_port() -> Option<u16> {
    Some(ascom_alpaca::discovery::DEFAULT_DISCOVERY_PORT)
}

/// Rotator device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotatorConfig {
    pub name: String,
    #[serde(default)]
    pub unique_id: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Status Switch device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchConfig {
    pub name: String,
    #[serde(default)]
    pub unique_id: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_baud_rate() -> u32 {
    9600
}

fn default_timeout() -> Duration {
    Duration::from_secs(2)
}

fn default_true() -> bool {
    true
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: "/dev/ttyUSB0".to_string(),
            baud_rate: default_baud_rate(),
            timeout: default_timeout(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 11118,
            discovery_port: default_discovery_port(),
            tls: None,
            auth: None,
        }
    }
}

impl Default for RotatorConfig {
    fn default() -> Self {
        Self {
            name: "Pegasus Falcon Rotator".to_string(),
            unique_id: String::new(),
            description: "Pegasus Astro Falcon Rotator (firmware >= 1.3)".to_string(),
            enabled: true,
        }
    }
}

impl Default for SwitchConfig {
    fn default() -> Self {
        Self {
            name: "Pegasus Falcon Status".to_string(),
            unique_id: String::new(),
            description: "Pegasus Falcon Rotator status sensors (voltage + limit-hit)".to_string(),
            enabled: true,
        }
    }
}

/// Load configuration from a JSON file
pub fn load_config(path: &Path) -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let config = Config::default();

        assert_eq!(config.rotator.name, "Pegasus Falcon Rotator");
        assert!(config.rotator.unique_id.is_empty());
        assert!(config.rotator.enabled);

        assert_eq!(config.switch.name, "Pegasus Falcon Status");
        assert!(config.switch.unique_id.is_empty());
        assert!(config.switch.enabled);

        assert_eq!(config.serial.port, "/dev/ttyUSB0");
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.serial.timeout, Duration::from_secs(2));

        assert_eq!(config.server.port, 11118);
        assert!(config.server.discovery_port.is_some());
        assert!(config.server.tls.is_none());
        assert!(config.server.auth.is_none());
    }

    #[test]
    fn rotator_config_default() {
        let config = RotatorConfig::default();
        assert_eq!(config.name, "Pegasus Falcon Rotator");
        assert!(config.unique_id.is_empty());
        assert_eq!(
            config.description,
            "Pegasus Astro Falcon Rotator (firmware >= 1.3)"
        );
        assert!(config.enabled);
    }

    #[test]
    fn switch_config_default() {
        let config = SwitchConfig::default();
        assert_eq!(config.name, "Pegasus Falcon Status");
        assert!(config.unique_id.is_empty());
        assert!(config.description.contains("voltage"));
        assert!(config.description.contains("limit"));
        assert!(config.enabled);
    }

    #[test]
    fn serial_config_default() {
        let config = SerialConfig::default();
        assert_eq!(config.port, "/dev/ttyUSB0");
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.timeout, Duration::from_secs(2));
    }

    #[test]
    fn server_config_default_uses_falcon_port() {
        let config = ServerConfig::default();
        assert_eq!(config.port, 11118);
    }

    #[test]
    fn config_serializes_to_json() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();

        assert!(json.contains("Pegasus Falcon Rotator"));
        assert!(json.contains("Pegasus Falcon Status"));
        assert!(json.contains("/dev/ttyUSB0"));
        assert!(json.contains("9600"));
        assert!(json.contains("11118"));
    }

    #[test]
    fn config_deserializes_from_json() {
        let json = r#"{
            "serial": {
                "port": "/dev/ttyUSB1",
                "baud_rate": 9600,
                "timeout": "3s"
            },
            "server": { "port": 12345 },
            "rotator": {
                "name": "Test Rotator",
                "unique_id": "test-rotator-001",
                "description": "Test rotator description",
                "enabled": true
            },
            "switch": {
                "name": "Test Status",
                "unique_id": "test-status-001",
                "description": "Test status description",
                "enabled": false
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.serial.port, "/dev/ttyUSB1");
        assert_eq!(config.serial.timeout, Duration::from_secs(3));
        assert_eq!(config.server.port, 12345);
        assert_eq!(config.rotator.name, "Test Rotator");
        assert!(config.rotator.enabled);
        assert_eq!(config.switch.name, "Test Status");
        assert!(!config.switch.enabled);
    }

    #[test]
    fn config_deserializes_with_defaults() {
        let json = r#"{
            "serial": { "port": "/dev/ttyUSB2" },
            "server": { "port": 9999 },
            "rotator": {
                "name": "Minimal Rotator",
                "unique_id": "min-rotator-001",
                "description": "Minimal config"
            },
            "switch": {
                "name": "Minimal Status",
                "unique_id": "min-status-001",
                "description": "Minimal status"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.serial.timeout, Duration::from_secs(2));
        assert!(config.rotator.enabled);
        assert!(config.switch.enabled);
    }

    #[test]
    fn load_config_from_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        let json = serde_json::to_string(&Config::default()).unwrap();
        std::fs::write(&path, json).unwrap();

        let loaded = load_config(&path).unwrap();
        assert_eq!(loaded.serial.port, "/dev/ttyUSB0");
        assert_eq!(loaded.server.port, 11118);
        assert_eq!(loaded.rotator.name, "Pegasus Falcon Rotator");
        assert_eq!(loaded.switch.name, "Pegasus Falcon Status");
    }

    #[test]
    fn load_config_nonexistent_file_errors() {
        let path = std::path::PathBuf::from("/tmp/pa_falcon_rotator_nonexistent_12345.json");
        let result = load_config(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_config_invalid_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "this is not json").unwrap();

        let result = load_config(&path);
        assert!(result.is_err());
    }

    #[test]
    fn config_clone_round_trip() {
        let config = Config::default();
        let cloned = config.clone();
        assert_eq!(config.rotator.name, cloned.rotator.name);
        assert_eq!(config.switch.name, cloned.switch.name);
    }

    #[test]
    fn config_debug_contains_struct_names() {
        let config = Config::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("Config"));
        assert!(debug_str.contains("SerialConfig"));
        assert!(debug_str.contains("ServerConfig"));
        assert!(debug_str.contains("RotatorConfig"));
        assert!(debug_str.contains("SwitchConfig"));
    }
}
