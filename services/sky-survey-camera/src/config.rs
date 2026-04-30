use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::SkySurveyCameraError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub device: DeviceConfig,
    pub optics: OpticsConfig,
    pub pointing: PointingConfig,
    pub survey: SurveyConfig,
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpticsConfig {
    pub focal_length_mm: f64,
    pub pixel_size_x_um: f64,
    pub pixel_size_y_um: f64,
    pub sensor_width_px: u32,
    pub sensor_height_px: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointingConfig {
    pub initial_ra_deg: f64,
    pub initial_dec_deg: f64,
    #[serde(default)]
    pub initial_rotation_deg: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurveyConfig {
    pub name: String,
    #[serde(with = "humantime_serde")]
    pub request_timeout: Duration,
    pub cache_dir: PathBuf,
    /// Base URL the SurveyClient hits. Defaults to NASA SkyView; tests
    /// override it with a stub server.
    #[serde(default = "default_survey_endpoint")]
    pub endpoint: String,
}

fn default_survey_endpoint() -> String {
    "https://skyview.gsfc.nasa.gov/current/cgi/runquery.pl".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub device_number: u32,
}

pub async fn load_config(path: &Path) -> Result<Config, SkySurveyCameraError> {
    let bytes = tokio::fs::read(path).await?;
    let config: Config = serde_json::from_slice(&bytes)?;
    Ok(config)
}
