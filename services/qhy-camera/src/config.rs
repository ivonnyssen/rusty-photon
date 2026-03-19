//! Configuration types for the QHY Camera driver

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub cameras: Vec<CameraConfig>,
    #[serde(default)]
    pub filter_wheels: Vec<FilterWheelConfig>,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
}

/// Camera device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraConfig {
    pub unique_id: String,
    #[serde(default = "default_camera_name")]
    pub name: String,
    #[serde(default = "default_camera_description")]
    pub description: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Filter wheel device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterWheelConfig {
    pub unique_id: String,
    #[serde(default = "default_filter_wheel_name")]
    pub name: String,
    #[serde(default = "default_filter_wheel_description")]
    pub description: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub filter_names: Vec<String>,
}

fn default_port() -> u16 {
    11116
}

fn default_true() -> bool {
    true
}

fn default_camera_name() -> String {
    "QHYCCD Camera".to_string()
}

fn default_camera_description() -> String {
    "QHYCCD camera via qhyccd-rs SDK".to_string()
}

fn default_filter_wheel_name() -> String {
    "QHYCCD Filter Wheel".to_string()
}

fn default_filter_wheel_description() -> String {
    "QHYCCD filter wheel via qhyccd-rs SDK".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
        }
    }
}

/// Load configuration from a JSON file
pub fn load_config(path: &PathBuf) -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}
