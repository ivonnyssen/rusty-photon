//! QHY Q-Focuser Driver
//!
//! ASCOM Alpaca driver for the QHY Q-Focuser (EAF).
//!
//! This driver exposes an ASCOM Focuser device for controlling
//! a QHY Q-Focuser stepper motor over USB serial.

pub mod config;
pub mod error;
pub mod focuser_device;
pub mod io;
#[cfg(feature = "mock")]
pub mod mock;
pub mod protocol;
pub mod serial;
pub mod serial_manager;

pub use config::{load_config, Config, FocuserConfig, SerialConfig, ServerConfig};
pub use error::{QhyFocuserError, Result};
pub use focuser_device::QhyFocuserDevice;
pub use io::SerialPortFactory;
pub use serial_manager::SerialManager;

#[cfg(feature = "mock")]
pub use mock::MockSerialPortFactory;

use std::net::SocketAddr;
use std::sync::Arc;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use serial::TokioSerialPortFactory;
use tracing::info;

/// Builder for the ASCOM Alpaca server.
///
/// Configures the focuser device and serial port factory, then binds the server.
pub struct ServerBuilder {
    config: Config,
    factory: Arc<dyn SerialPortFactory>,
}

impl ServerBuilder {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            factory: Arc::new(TokioSerialPortFactory::new()),
        }
    }

    pub fn with_factory(mut self, factory: Arc<dyn SerialPortFactory>) -> Self {
        self.factory = factory;
        self
    }

    pub async fn build(
        self,
    ) -> std::result::Result<ascom_alpaca::BoundServer, Box<dyn std::error::Error>> {
        let mut server = Server::new(CargoServerInfo!());
        server.listen_addr = SocketAddr::from(([0, 0, 0, 0], self.config.server.port));

        let serial_manager = Arc::new(SerialManager::new(self.config.clone(), self.factory));

        if self.config.focuser.enabled {
            let focuser_device =
                QhyFocuserDevice::new(self.config.focuser.clone(), Arc::clone(&serial_manager));
            server.devices.register(focuser_device);
            info!(
                "Registered Focuser device: {} (device number {})",
                self.config.focuser.name, self.config.focuser.device_number
            );
        }

        info!("Serial port: {}", self.config.serial.port);

        let bound = server.bind().await?;
        println!("Bound Alpaca server bound_addr={}", bound.listen_addr());
        info!("Bound Alpaca server bound_addr={}", bound.listen_addr());
        Ok(bound)
    }
}
