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
            port: "/dev/ttyUSB0".to_string(),
            baud_rate: default_baud_rate(),
            polling_interval_ms: default_polling_interval(),
            timeout_seconds: default_timeout(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { port: 11113 }
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
