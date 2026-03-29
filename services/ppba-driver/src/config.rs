//! Configuration types for the PPBA Driver

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub serial: SerialConfig,
    pub server: ServerConfig,
    pub switch: SwitchConfig,
    pub observingconditions: ObservingConditionsConfig,
}

/// Serial port configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerialConfig {
    pub port: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    #[serde(default = "default_polling_interval")]
    pub polling_interval_ms: u64,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
}

/// Switch device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// ObservingConditions device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservingConditionsConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_averaging_period")]
    pub averaging_period_ms: u64,
}

fn default_baud_rate() -> u32 {
    9600
}

fn default_polling_interval() -> u64 {
    5000
}

fn default_timeout() -> u64 {
    2
}

fn default_true() -> bool {
    true
}

fn default_averaging_period() -> u64 {
    300_000 // 5 minutes in milliseconds
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: "/dev/ttyUSB0".to_string(),
            baud_rate: default_baud_rate(),
            polling_interval_ms: default_polling_interval(),
            timeout_seconds: default_timeout(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 11112,
            tls: None,
        }
    }
}

impl Default for SwitchConfig {
    fn default() -> Self {
        Self {
            name: "Pegasus PPBA Switch".to_string(),
            unique_id: "ppba-switch-001".to_string(),
            description: "Pegasus Astro PPBA Gen2 Power Control".to_string(),
            device_number: 0,
            enabled: true,
        }
    }
}

impl Default for ObservingConditionsConfig {
    fn default() -> Self {
        Self {
            name: "Pegasus PPBA Weather".to_string(),
            unique_id: "ppba-observingconditions-001".to_string(),
            description: "Pegasus Astro PPBA Environmental Sensors".to_string(),
            device_number: 0,
            enabled: true,
            averaging_period_ms: default_averaging_period(),
        }
    }
}

/// Legacy DeviceConfig for backward compatibility during migration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
}

/// Load configuration from a JSON file
pub fn load_config(path: &PathBuf) -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let config = Config::default();

        assert_eq!(config.switch.name, "Pegasus PPBA Switch");
        assert!(config.switch.enabled);

        assert_eq!(config.observingconditions.name, "Pegasus PPBA Weather");
        assert!(config.observingconditions.enabled);

        assert_eq!(config.serial.port, "/dev/ttyUSB0");
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.serial.polling_interval_ms, 5000);
        assert_eq!(config.serial.timeout_seconds, 2);

        assert_eq!(config.server.port, 11112);
    }

    #[test]
    fn switch_config_default() {
        let config = SwitchConfig::default();

        assert_eq!(config.name, "Pegasus PPBA Switch");
        assert_eq!(config.unique_id, "ppba-switch-001");
        assert!(!config.description.is_empty());
        assert_eq!(config.device_number, 0);
        assert!(config.enabled);
    }

    #[test]
    fn observingconditions_config_default() {
        let config = ObservingConditionsConfig::default();

        assert_eq!(config.name, "Pegasus PPBA Weather");
        assert_eq!(config.unique_id, "ppba-observingconditions-001");
        assert!(!config.description.is_empty());
        assert_eq!(config.device_number, 0);
        assert!(config.enabled);
        assert_eq!(config.averaging_period_ms, 300_000); // 5 minutes
    }

    #[test]
    fn serial_config_default() {
        let config = SerialConfig::default();

        assert_eq!(config.port, "/dev/ttyUSB0");
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.polling_interval_ms, 5000);
        assert_eq!(config.timeout_seconds, 2);
    }

    #[test]
    fn server_config_default() {
        let config = ServerConfig::default();

        assert_eq!(config.port, 11112);
    }

    #[test]
    fn config_serializes_to_json() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();

        assert!(json.contains("Pegasus PPBA"));
        assert!(json.contains("/dev/ttyUSB0"));
        assert!(json.contains("9600"));
        assert!(json.contains("11112"));
    }

    #[test]
    fn config_deserializes_from_json() {
        let json = r#"{
            "serial": {
                "port": "/dev/ttyACM0",
                "baud_rate": 115200,
                "polling_interval_ms": 10000,
                "timeout_seconds": 5
            },
            "server": {
                "port": 8080
            },
            "switch": {
                "name": "Test Switch",
                "unique_id": "test-switch-001",
                "description": "Test switch description",
                "device_number": 1,
                "enabled": true
            },
            "observingconditions": {
                "name": "Test Weather",
                "unique_id": "test-weather-001",
                "description": "Test weather description",
                "device_number": 2,
                "enabled": false,
                "averaging_period_ms": 120000
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.switch.name, "Test Switch");
        assert_eq!(config.switch.unique_id, "test-switch-001");
        assert_eq!(config.switch.device_number, 1);
        assert!(config.switch.enabled);

        assert_eq!(config.observingconditions.name, "Test Weather");
        assert_eq!(config.observingconditions.device_number, 2);
        assert!(!config.observingconditions.enabled);
        assert_eq!(config.observingconditions.averaging_period_ms, 120000);

        assert_eq!(config.serial.port, "/dev/ttyACM0");
        assert_eq!(config.serial.baud_rate, 115200);
        assert_eq!(config.serial.polling_interval_ms, 10000);
        assert_eq!(config.server.port, 8080);
    }

    #[test]
    fn config_deserializes_with_defaults() {
        // Minimal JSON with only required fields
        let json = r#"{
            "serial": {
                "port": "/dev/ttyUSB1"
            },
            "server": {
                "port": 9000
            },
            "switch": {
                "name": "Minimal Switch",
                "unique_id": "min-switch-001",
                "description": "Minimal config"
            },
            "observingconditions": {
                "name": "Minimal Weather",
                "unique_id": "min-weather-001",
                "description": "Minimal weather config"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.switch.name, "Minimal Switch");
        assert_eq!(config.serial.port, "/dev/ttyUSB1");
        // These should have defaults
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.serial.polling_interval_ms, 5000);
        assert_eq!(config.serial.timeout_seconds, 2);
        assert_eq!(config.switch.device_number, 0);
        assert!(config.switch.enabled);
        assert_eq!(config.observingconditions.device_number, 0);
        assert!(config.observingconditions.enabled);
        assert_eq!(config.observingconditions.averaging_period_ms, 300_000);
    }

    #[test]
    fn config_clone_works() {
        let config = Config::default();
        let cloned = config.clone();

        assert_eq!(config.switch.name, cloned.switch.name);
        assert_eq!(config.serial.port, cloned.serial.port);
        assert_eq!(config.server.port, cloned.server.port);
    }

    #[test]
    fn config_debug_works() {
        let config = Config::default();
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("Config"));
        assert!(debug_str.contains("SwitchConfig"));
        assert!(debug_str.contains("ObservingConditionsConfig"));
        assert!(debug_str.contains("SerialConfig"));
        assert!(debug_str.contains("ServerConfig"));
    }
}
