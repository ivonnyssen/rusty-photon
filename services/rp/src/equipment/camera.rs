use std::sync::Arc;

use ascom_alpaca::api::{Camera, TypedDevice};
use tracing::{debug, error};

use super::alpaca::{
    build_alpaca_client, retry_connect_attempt, AttemptOutcome, GET_DEVICES_TIMEOUT,
};
use crate::config;

pub struct CameraEntry {
    pub id: String,
    pub connected: bool,
    pub config: config::CameraConfig,
    pub device: Option<Arc<dyn Camera>>,
}

pub(super) async fn connect_camera(config: &config::CameraConfig) -> CameraEntry {
    debug!(camera_id = %config.id, alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to camera");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            error!(camera_id = %config.id, error = %e, "failed to create Alpaca client for camera");
            return CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let label = format!("camera {}", config.id);
    let outcome = retry_connect_attempt(&label, |_attempt| async {
        let devices = match tokio::time::timeout(GET_DEVICES_TIMEOUT, client.get_devices()).await {
            Ok(Ok(devices)) => devices,
            Ok(Err(e)) => return AttemptOutcome::Transient(format!("get_devices: {e}")),
            Err(_) => {
                return AttemptOutcome::Transient(format!(
                    "get_devices: timeout after {:?}",
                    GET_DEVICES_TIMEOUT
                ));
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
                return AttemptOutcome::Permanent(format!(
                    "camera at index {} not found on Alpaca server",
                    config.device_number
                ));
            }
        };

        match cam.set_connected(true).await {
            Ok(()) => AttemptOutcome::Ok(cam),
            Err(e) => AttemptOutcome::Transient(format!("set_connected: {e}")),
        }
    })
    .await;

    match outcome {
        Ok(cam) => {
            debug!(camera_id = %config.id, "camera connected successfully");
            CameraEntry {
                id: config.id.clone(),
                connected: true,
                config: config.clone(),
                device: Some(cam),
            }
        }
        Err(msg) => {
            error!(camera_id = %config.id, error = %msg, "failed to connect camera");
            CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}
