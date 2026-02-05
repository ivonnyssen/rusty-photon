//! Configuration types for the PPBA Driver

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for Config {
    fn default() -> Self {
        Self {
            serial: SerialConfig::default(),
            server: ServerConfig::default(),
            switch: SwitchConfig::default(),
            observingconditions: ObservingConditionsConfig::default(),
        }
    }
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
        Self { port: 11112 }
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
