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
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_bind")]
    pub bind_address: String,
}

fn default_port() -> u16 {
    11115
}

fn default_bind() -> String {
    "127.0.0.1".to_string()
}

pub fn load_config(path: &str) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| RpError::Config(format!("failed to read config file '{}': {}", path, e)))?;
    serde_json::from_str(&contents)
        .map_err(|e| RpError::Config(format!("failed to parse config file '{}': {}", path, e)))
}
