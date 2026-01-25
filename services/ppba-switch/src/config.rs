//! Configuration types for the PPBA Switch driver

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub device: DeviceConfig,
    pub serial: SerialConfig,
    pub server: ServerConfig,
}

/// Device identification configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
}

/// Serial port configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerialConfig {
    pub port: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    #[serde(default = "default_polling_interval")]
    pub polling_interval_seconds: u64,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    #[serde(default)]
    pub device_number: u32,
}

fn default_baud_rate() -> u32 {
    9600
}

fn default_polling_interval() -> u64 {
    5
}

fn default_timeout() -> u64 {
    2
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: "/dev/ttyUSB0".to_string(),
            baud_rate: default_baud_rate(),
            polling_interval_seconds: default_polling_interval(),
            timeout_seconds: default_timeout(),
        }
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            name: "Pegasus PPBA".to_string(),
            unique_id: "ppba-switch-001".to_string(),
            description: "Pegasus Astro Pocket Powerbox Advance Gen2".to_string(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 11112,
            device_number: 0,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: DeviceConfig::default(),
            serial: SerialConfig::default(),
            server: ServerConfig::default(),
        }
    }
}

/// Load configuration from a JSON file
pub fn load_config(path: &PathBuf) -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}
