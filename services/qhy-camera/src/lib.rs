//! QHY Camera Driver
//!
//! ASCOM Alpaca driver for QHYCCD cameras and filter wheels.
//!
//! This driver exposes ASCOM Camera and FilterWheel devices for controlling
//! QHYCCD cameras via the qhyccd-rs SDK bindings.

pub mod camera_device;
pub mod config;
pub mod error;
pub mod filter_wheel_device;
pub mod io;
#[cfg(feature = "mock")]
pub mod mock;

pub use camera_device::QhyccdCamera;
pub use config::{load_config, CameraConfig, Config, FilterWheelConfig, ServerConfig};
pub use error::{QhyCameraError, Result};
pub use filter_wheel_device::QhyccdFilterWheel;
pub use io::{CameraHandle, FilterWheelHandle, SdkProvider};

#[cfg(feature = "mock")]
pub use mock::MockSdkProvider;

use std::net::SocketAddr;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use tracing::info;

/// Builder for the ASCOM Alpaca server.
///
/// Configures camera and filter wheel devices, then binds the server.
pub struct ServerBuilder {
    config: Config,
    sdk_provider: Box<dyn SdkProvider>,
}

impl ServerBuilder {
    pub fn new(config: Config, sdk_provider: Box<dyn SdkProvider>) -> Self {
        Self {
            config,
            sdk_provider,
        }
    }

    pub async fn build(
        self,
    ) -> std::result::Result<ascom_alpaca::BoundServer, Box<dyn std::error::Error>> {
        let mut server = Server::new(CargoServerInfo!());
        server.listen_addr = SocketAddr::from(([0, 0, 0, 0], self.config.server.port));

        // Register cameras
        for cam_config in &self.config.cameras {
            if !cam_config.enabled {
                continue;
            }
            let handle = self
                .sdk_provider
                .open_camera(&cam_config.unique_id)
                .map_err(|e| format!("Failed to open camera {}: {}", cam_config.unique_id, e))?;
            let camera = QhyccdCamera::new(cam_config.clone(), handle);
            server.devices.register(camera);
            info!(
                "Registered Camera: {} (device number {})",
                cam_config.name, cam_config.device_number
            );
        }

        // Register filter wheels
        for fw_config in &self.config.filter_wheels {
            if !fw_config.enabled {
                continue;
            }
            let handle = self
                .sdk_provider
                .open_filter_wheel(&fw_config.unique_id)
                .map_err(|e| {
                    format!("Failed to open filter wheel {}: {}", fw_config.unique_id, e)
                })?;
            let filter_wheel = QhyccdFilterWheel::new(fw_config.clone(), handle);
            server.devices.register(filter_wheel);
            info!(
                "Registered FilterWheel: {} (device number {})",
                fw_config.name, fw_config.device_number
            );
        }

        let bound = server.bind().await?;
        println!("Bound Alpaca server bound_addr={}", bound.listen_addr());
        info!("Bound Alpaca server bound_addr={}", bound.listen_addr());
        Ok(bound)
    }
}
