//! PPBA Driver
//!
//! ASCOM Alpaca driver for the Pegasus Astro Pocket Powerbox Advance Gen2 (PPBA).
//!
//! This driver exposes two ASCOM devices:
//! - Switch device for power control and sensor monitoring
//! - ObservingConditions device for environmental sensors

pub mod config;
pub mod error;
pub mod io;
pub mod mean;
#[cfg(feature = "mock")]
pub mod mock;
pub mod observingconditions_device;
pub mod protocol;
pub mod serial;
pub mod serial_manager;
pub mod switch_device;
pub mod switches;

pub use config::{
    load_config, Config, DeviceConfig, ObservingConditionsConfig, SerialConfig, ServerConfig,
    SwitchConfig,
};
pub use error::{PpbaError, Result};
pub use io::SerialPortFactory;
pub use observingconditions_device::PpbaObservingConditionsDevice;
pub use serial_manager::SerialManager;
pub use switch_device::PpbaSwitchDevice;
pub use switches::{SwitchId, SwitchInfo, MAX_SWITCH};

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
/// Configures devices and serial port factory, then binds the server.
/// The returned `BoundServer` can be inspected (e.g. `listen_addr()`)
/// before calling `start()`.
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

        if self.config.switch.enabled {
            let switch_device =
                PpbaSwitchDevice::new(self.config.switch.clone(), Arc::clone(&serial_manager));
            server.devices.register(switch_device);
            info!(
                "Registered Switch device: {} (device number {})",
                self.config.switch.name, self.config.switch.device_number
            );
        }

        if self.config.observingconditions.enabled {
            let oc_device = PpbaObservingConditionsDevice::new(
                self.config.observingconditions.clone(),
                Arc::clone(&serial_manager),
            );
            server.devices.register(oc_device);
            info!(
                "Registered ObservingConditions device: {} (device number {})",
                self.config.observingconditions.name, self.config.observingconditions.device_number
            );
        }

        info!("Serial port: {}", self.config.serial.port);

        let bound = server.bind().await?;
        // This println is parsed by conformu_integration tests to discover the bound port.
        // It must go to stdout (not tracing/stderr) so the subprocess output can be read.
        println!("Bound Alpaca server bound_addr={}", bound.listen_addr());
        info!("Bound Alpaca server bound_addr={}", bound.listen_addr());
        Ok(bound)
    }
}
