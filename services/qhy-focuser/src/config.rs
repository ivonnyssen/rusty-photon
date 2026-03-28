//! Configuration types for the QHY Q-Focuser driver

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub serial: SerialConfig,
    pub server: ServerConfig,
    pub focuser: FocuserConfig,
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
    #[serde(default = "default_discovery_port")]
    pub discovery_port: Option<u16>,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
}

fn default_discovery_port() -> Option<u16> {
    Some(ascom_alpaca::discovery::DEFAULT_DISCOVERY_PORT)
}

/// Focuser device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocuserConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_step")]
    pub max_step: u32,
    #[serde(default)]
    pub speed: u8,
    #[serde(default)]
    pub reverse: bool,
}

fn default_baud_rate() -> u32 {
    9600
}

fn default_polling_interval() -> u64 {
    1000
}

fn default_timeout() -> u64 {
    2
}

fn default_true() -> bool {
    true
}

fn default_max_step() -> u32 {
    64_000
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: "/dev/ttyACM0".to_string(),
            baud_rate: default_baud_rate(),
            polling_interval_ms: default_polling_interval(),
            timeout_seconds: default_timeout(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 11113,
            discovery_port: default_discovery_port(),
            tls: None,
        }
    }
}

impl Default for FocuserConfig {
    fn default() -> Self {
        Self {
            name: "QHY Q-Focuser".to_string(),
            unique_id: "qhy-focuser-001".to_string(),
            description: "QHY Q-Focuser (EAF) Stepper Motor Controller".to_string(),
            device_number: 0,
            enabled: true,
            max_step: default_max_step(),
            speed: 0,
            reverse: false,
        }
    }
}

/// Load configuration from a JSON file
pub fn load_config(path: &PathBuf) -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let config = Config::default();

        assert_eq!(config.focuser.name, "QHY Q-Focuser");
        assert!(config.focuser.enabled);
        assert_eq!(config.focuser.max_step, 64_000);
        assert_eq!(config.focuser.speed, 0);
        assert!(!config.focuser.reverse);

        assert_eq!(config.serial.port, "/dev/ttyACM0");
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.serial.polling_interval_ms, 1000);
        assert_eq!(config.serial.timeout_seconds, 2);

        assert_eq!(config.server.port, 11113);
    }

    #[test]
    fn focuser_config_default() {
        let config = FocuserConfig::default();

        assert_eq!(config.name, "QHY Q-Focuser");
        assert_eq!(config.unique_id, "qhy-focuser-001");
        assert!(!config.description.is_empty());
        assert_eq!(config.device_number, 0);
        assert!(config.enabled);
        assert_eq!(config.max_step, 64_000);
        assert_eq!(config.speed, 0);
        assert!(!config.reverse);
    }

    #[test]
    fn serial_config_default() {
        let config = SerialConfig::default();

        assert_eq!(config.port, "/dev/ttyACM0");
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.polling_interval_ms, 1000);
        assert_eq!(config.timeout_seconds, 2);
    }

    #[test]
    fn server_config_default() {
        let config = ServerConfig::default();

        assert_eq!(config.port, 11113);
    }

    #[test]
    fn config_serializes_to_json() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();

        assert!(json.contains("QHY Q-Focuser"));
        assert!(json.contains("/dev/ttyACM0"));
        assert!(json.contains("9600"));
        assert!(json.contains("11113"));
    }

    #[test]
    fn config_deserializes_from_json() {
        let json = r#"{
            "serial": {
                "port": "/dev/ttyACM0",
                "baud_rate": 115200,
                "polling_interval_ms": 2000,
                "timeout_seconds": 5
            },
            "server": {
                "port": 8080
            },
            "focuser": {
                "name": "Test Focuser",
                "unique_id": "test-focuser-001",
                "description": "Test focuser description",
                "device_number": 1,
                "enabled": true,
                "max_step": 100000,
                "speed": 3,
                "reverse": true
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.focuser.name, "Test Focuser");
        assert_eq!(config.focuser.unique_id, "test-focuser-001");
        assert_eq!(config.focuser.device_number, 1);
        assert!(config.focuser.enabled);
        assert_eq!(config.focuser.max_step, 100000);
        assert_eq!(config.focuser.speed, 3);
        assert!(config.focuser.reverse);

        assert_eq!(config.serial.port, "/dev/ttyACM0");
        assert_eq!(config.serial.baud_rate, 115200);
        assert_eq!(config.serial.polling_interval_ms, 2000);
        assert_eq!(config.server.port, 8080);
    }

    #[test]
    fn config_deserializes_with_defaults() {
        let json = r#"{
            "serial": {
                "port": "/dev/ttyUSB1"
            },
            "server": {
                "port": 9000
            },
            "focuser": {
                "name": "Minimal Focuser",
                "unique_id": "min-focuser-001",
                "description": "Minimal config"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.focuser.name, "Minimal Focuser");
        assert_eq!(config.serial.port, "/dev/ttyUSB1");
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.serial.polling_interval_ms, 1000);
        assert_eq!(config.serial.timeout_seconds, 2);
        assert_eq!(config.focuser.device_number, 0);
        assert!(config.focuser.enabled);
        assert_eq!(config.focuser.max_step, 64_000);
        assert_eq!(config.focuser.speed, 0);
        assert!(!config.focuser.reverse);
    }

    #[test]
    fn config_clone_works() {
        let config = Config::default();
        let cloned = config.clone();

        assert_eq!(config.focuser.name, cloned.focuser.name);
        assert_eq!(config.serial.port, cloned.serial.port);
        assert_eq!(config.server.port, cloned.server.port);
    }

    #[test]
    fn config_debug_works() {
        let config = Config::default();
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("Config"));
        assert!(debug_str.contains("FocuserConfig"));
        assert!(debug_str.contains("SerialConfig"));
        assert!(debug_str.contains("ServerConfig"));
    }

    #[test]
    fn load_config_from_file() {
        let dir = std::env::temp_dir().join("qhy_focuser_test_load_config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");

        let json = r#"{
            "serial": { "port": "/dev/ttyUSB0", "baud_rate": 115200 },
            "server": { "port": 9999 },
            "focuser": {
                "name": "Test Focuser",
                "unique_id": "test-001",
                "description": "A test focuser",
                "speed": 7
            }
        }"#;
        std::fs::write(&path, json).unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.serial.port, "/dev/ttyUSB0");
        assert_eq!(config.serial.baud_rate, 115200);
        assert_eq!(config.server.port, 9999);
        assert_eq!(config.focuser.name, "Test Focuser");
        assert_eq!(config.focuser.speed, 7);

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_config_nonexistent_file() {
        let path = PathBuf::from("/tmp/qhy_focuser_nonexistent_config_12345.json");
        let result = load_config(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_config_invalid_json() {
        let dir = std::env::temp_dir().join("qhy_focuser_test_invalid_json");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad_config.json");

        std::fs::write(&path, "this is not valid json").unwrap();

        let result = load_config(&path);
        assert!(result.is_err());

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
