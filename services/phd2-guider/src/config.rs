//! Configuration types for the PHD2 guider service

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// PHD2 service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub phd2: Phd2Config,
    #[serde(default)]
    pub settling: SettleParams,
}

/// PHD2 connection settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phd2Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub executable_path: Option<PathBuf>,
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_seconds: u64,
    #[serde(default = "default_command_timeout")]
    pub command_timeout_seconds: u64,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub auto_connect_equipment: bool,
}

impl Default for Phd2Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            executable_path: None,
            connection_timeout_seconds: default_connection_timeout(),
            command_timeout_seconds: default_command_timeout(),
            auto_start: false,
            auto_connect_equipment: false,
        }
    }
}

fn default_host() -> String {
    "localhost".to_string()
}

fn default_port() -> u16 {
    4400
}

fn default_connection_timeout() -> u64 {
    10
}

fn default_command_timeout() -> u64 {
    30
}

/// Settling parameters for guiding operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettleParams {
    #[serde(default = "default_settle_pixels")]
    pub pixels: f64,
    #[serde(default = "default_settle_time")]
    pub time: u32,
    #[serde(default = "default_settle_timeout")]
    pub timeout: u32,
}

impl Default for SettleParams {
    fn default() -> Self {
        Self {
            pixels: default_settle_pixels(),
            time: default_settle_time(),
            timeout: default_settle_timeout(),
        }
    }
}

fn default_settle_pixels() -> f64 {
    0.5
}

fn default_settle_time() -> u32 {
    10
}

fn default_settle_timeout() -> u32 {
    60
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
    fn test_settle_params_default() {
        let params = SettleParams::default();
        assert_eq!(params.pixels, 0.5);
        assert_eq!(params.time, 10);
        assert_eq!(params.timeout, 60);
    }

    #[test]
    fn test_phd2_config_default() {
        let config = Phd2Config::default();
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 4400);
        assert_eq!(config.connection_timeout_seconds, 10);
        assert_eq!(config.command_timeout_seconds, 30);
        assert!(!config.auto_start);
        assert!(!config.auto_connect_equipment);
    }

    #[test]
    fn test_settle_params_serialization() {
        let params = SettleParams {
            pixels: 1.5,
            time: 15,
            timeout: 120,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["pixels"], 1.5);
        assert_eq!(json["time"], 15);
        assert_eq!(json["timeout"], 120);
    }
}
