#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! sky-survey-camera: ASCOM Alpaca Camera simulator backed by NASA SkyView.
//!
//! Phase-2 scaffold: this crate currently only loads its config and binds
//! a TCP listener that prints `bound_addr=` for `bdd-infra::ServiceHandle`
//! port discovery. The Camera trait implementation, SkyView fetch, FITS
//! cache, and runtime pointing endpoints all land in phase 3.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub device_number: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SkySurveyCameraError {
    #[error("config I/O: {0}")]
    ConfigIo(#[from] std::io::Error),
    #[error("config parse: {0}")]
    ConfigParse(#[from] serde_json::Error),
}

pub async fn load_config(path: &Path) -> Result<Config, SkySurveyCameraError> {
    let bytes = tokio::fs::read(path).await?;
    let config: Config = serde_json::from_slice(&bytes)?;
    Ok(config)
}

/// Phase-2 stub: bind a TCP listener so port discovery succeeds, then
/// accept and immediately drop connections. Replaced in phase 3 by the
/// composed ASCOM + `/sky-survey/*` axum router.
pub async fn run(config_path: &Path) -> Result<(), SkySurveyCameraError> {
    let config = load_config(config_path).await?;
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.server.port)).await?;
    let local = listener.local_addr()?;
    println!("bound_addr={local}");
    tracing::info!(address = %local, "sky-survey-camera bound (phase-2 stub: no HTTP serving)");
    loop {
        let (_socket, _peer) = listener.accept().await?;
        // Drop the connection immediately; phase 3 replaces this with axum.
    }
}
