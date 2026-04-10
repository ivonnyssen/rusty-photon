use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Camera, CoverCalibrator, FilterWheel, TypedDevice};
use ascom_alpaca::Client;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rp_auth::config::ClientAuthConfig;
use serde::Serialize;
use tracing::debug;

use crate::config;

pub struct CameraEntry {
    pub id: String,
    pub connected: bool,
    pub config: config::CameraConfig,
    pub device: Option<Arc<dyn Camera>>,
}

pub struct FilterWheelEntry {
    pub id: String,
    pub connected: bool,
    pub config: config::FilterWheelConfig,
    pub device: Option<Arc<dyn FilterWheel>>,
}

pub struct CoverCalibratorEntry {
    pub id: String,
    pub connected: bool,
    pub config: config::CoverCalibratorConfig,
    pub device: Option<Arc<dyn CoverCalibrator>>,
}

pub struct EquipmentRegistry {
    pub cameras: Vec<CameraEntry>,
    pub filter_wheels: Vec<FilterWheelEntry>,
    pub cover_calibrators: Vec<CoverCalibratorEntry>,
}

#[derive(Serialize)]
pub struct EquipmentStatus {
    pub cameras: Vec<DeviceStatus>,
    pub filter_wheels: Vec<DeviceStatus>,
    pub cover_calibrators: Vec<DeviceStatus>,
}

#[derive(Serialize)]
pub struct DeviceStatus {
    pub id: String,
    pub connected: bool,
}

impl EquipmentRegistry {
    pub async fn new(equipment_config: &config::EquipmentConfig) -> Self {
        let mut cameras = Vec::new();
        let mut filter_wheels = Vec::new();
        let mut cover_calibrators = Vec::new();

        for cam_config in &equipment_config.cameras {
            let entry = connect_camera(cam_config).await;
            cameras.push(entry);
        }

        for fw_config in &equipment_config.filter_wheels {
            let entry = connect_filter_wheel(fw_config).await;
            filter_wheels.push(entry);
        }

        for cc_config in &equipment_config.cover_calibrators {
            let entry = connect_cover_calibrator(cc_config).await;
            cover_calibrators.push(entry);
        }

        Self {
            cameras,
            filter_wheels,
            cover_calibrators,
        }
    }

    pub fn status(&self) -> EquipmentStatus {
        EquipmentStatus {
            cameras: self
                .cameras
                .iter()
                .map(|c| DeviceStatus {
                    id: c.id.clone(),
                    connected: c.connected,
                })
                .collect(),
            filter_wheels: self
                .filter_wheels
                .iter()
                .map(|fw| DeviceStatus {
                    id: fw.id.clone(),
                    connected: fw.connected,
                })
                .collect(),
            cover_calibrators: self
                .cover_calibrators
                .iter()
                .map(|cc| DeviceStatus {
                    id: cc.id.clone(),
                    connected: cc.connected,
                })
                .collect(),
        }
    }

    pub fn find_camera(&self, id: &str) -> Option<&CameraEntry> {
        self.cameras.iter().find(|c| c.id == id)
    }

    pub fn find_filter_wheel(&self, id: &str) -> Option<&FilterWheelEntry> {
        self.filter_wheels.iter().find(|fw| fw.id == id)
    }

    pub fn find_cover_calibrator(&self, id: &str) -> Option<&CoverCalibratorEntry> {
        self.cover_calibrators.iter().find(|cc| cc.id == id)
    }
}

/// Build an Alpaca client with optional HTTP Basic Auth credentials.
fn build_alpaca_client(
    url: &str,
    auth: Option<&ClientAuthConfig>,
) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    match auth {
        Some(a) => {
            let encoded = BASE64.encode(format!("{}:{}", a.username, a.password));
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                "authorization",
                format!("Basic {encoded}")
                    .parse()
                    .expect("valid header value"),
            );
            let http = reqwest::Client::builder()
                .default_headers(headers)
                .build()?;
            Ok(Client::new_with_client(url, http)?)
        }
        None => Ok(Client::new(url)?),
    }
}

async fn connect_camera(config: &config::CameraConfig) -> CameraEntry {
    debug!(camera_id = %config.id, alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to camera");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            debug!(camera_id = %config.id, error = %e, "failed to create Alpaca client for camera");
            return CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let devices = match tokio::time::timeout(Duration::from_secs(5), client.get_devices()).await {
        Ok(Ok(devices)) => devices,
        Ok(Err(e)) => {
            debug!(camera_id = %config.id, error = %e, "failed to get devices from Alpaca server");
            return CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
        Err(_) => {
            debug!(camera_id = %config.id, "timeout connecting to Alpaca server");
            return CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let mut camera_index = 0u32;
    let mut found_camera: Option<Arc<dyn Camera>> = None;

    for device in devices {
        if let TypedDevice::Camera(cam) = device {
            if camera_index == config.device_number {
                found_camera = Some(cam);
                break;
            }
            camera_index += 1;
        }
    }

    let cam = match found_camera {
        Some(c) => c,
        None => {
            debug!(camera_id = %config.id, device_number = config.device_number, "camera not found on Alpaca server");
            return CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    match cam.set_connected(true).await {
        Ok(()) => {
            debug!(camera_id = %config.id, "camera connected successfully");
            CameraEntry {
                id: config.id.clone(),
                connected: true,
                config: config.clone(),
                device: Some(cam),
            }
        }
        Err(e) => {
            debug!(camera_id = %config.id, error = %e, "failed to connect camera");
            CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}

async fn connect_filter_wheel(config: &config::FilterWheelConfig) -> FilterWheelEntry {
    debug!(fw_id = %config.id, alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to filter wheel");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            debug!(fw_id = %config.id, error = %e, "failed to create Alpaca client for filter wheel");
            return FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let devices = match tokio::time::timeout(Duration::from_secs(5), client.get_devices()).await {
        Ok(Ok(devices)) => devices,
        Ok(Err(e)) => {
            debug!(fw_id = %config.id, error = %e, "failed to get devices from Alpaca server");
            return FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
        Err(_) => {
            debug!(fw_id = %config.id, "timeout connecting to Alpaca server");
            return FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let mut fw_index = 0u32;
    let mut found_fw: Option<Arc<dyn FilterWheel>> = None;

    for device in devices {
        if let TypedDevice::FilterWheel(fw) = device {
            if fw_index == config.device_number {
                found_fw = Some(fw);
                break;
            }
            fw_index += 1;
        }
    }

    let fw = match found_fw {
        Some(f) => f,
        None => {
            debug!(fw_id = %config.id, device_number = config.device_number, "filter wheel not found on Alpaca server");
            return FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    match fw.set_connected(true).await {
        Ok(()) => {
            debug!(fw_id = %config.id, "filter wheel connected successfully");
            FilterWheelEntry {
                id: config.id.clone(),
                connected: true,
                config: config.clone(),
                device: Some(fw),
            }
        }
        Err(e) => {
            debug!(fw_id = %config.id, error = %e, "failed to connect filter wheel");
            FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}

async fn connect_cover_calibrator(config: &config::CoverCalibratorConfig) -> CoverCalibratorEntry {
    debug!(cc_id = %config.id, alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to cover calibrator");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            debug!(cc_id = %config.id, error = %e, "failed to create Alpaca client for cover calibrator");
            return CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let devices = match tokio::time::timeout(Duration::from_secs(5), client.get_devices()).await {
        Ok(Ok(devices)) => devices,
        Ok(Err(e)) => {
            debug!(cc_id = %config.id, error = %e, "failed to get devices from Alpaca server");
            return CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
        Err(_) => {
            debug!(cc_id = %config.id, "timeout connecting to Alpaca server");
            return CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let mut cc_index = 0u32;
    let mut found_cc: Option<Arc<dyn CoverCalibrator>> = None;

    for device in devices {
        if let TypedDevice::CoverCalibrator(cc) = device {
            if cc_index == config.device_number {
                found_cc = Some(cc);
                break;
            }
            cc_index += 1;
        }
    }

    let cc = match found_cc {
        Some(c) => c,
        None => {
            debug!(cc_id = %config.id, device_number = config.device_number, "cover calibrator not found on Alpaca server");
            return CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    match cc.set_connected(true).await {
        Ok(()) => {
            debug!(cc_id = %config.id, "cover calibrator connected successfully");
            CoverCalibratorEntry {
                id: config.id.clone(),
                connected: true,
                config: config.clone(),
                device: Some(cc),
            }
        }
        Err(e) => {
            debug!(cc_id = %config.id, error = %e, "failed to connect cover calibrator");
            CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn build_alpaca_client_without_auth() {
        build_alpaca_client("http://localhost:11111", None).unwrap();
    }

    #[test]
    fn build_alpaca_client_with_auth() {
        let auth = ClientAuthConfig {
            username: "observatory".to_string(),
            password: "secret".to_string(),
        };
        build_alpaca_client("http://localhost:11111", Some(&auth)).unwrap();
    }

    #[test]
    fn build_alpaca_client_with_invalid_url_fails() {
        let result = build_alpaca_client("not-a-url", None);
        assert!(result.is_err());
    }
}
