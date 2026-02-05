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

/// Start the ASCOM Alpaca server with configured devices
pub async fn start_server(config: Config) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let factory = Arc::new(TokioSerialPortFactory::new());
    start_server_with_factory(config, factory).await
}

/// Start the ASCOM Alpaca server with a custom serial port factory
#[cfg(feature = "mock")]
pub async fn start_server_with_factory(
    config: Config,
    factory: Arc<dyn SerialPortFactory>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    start_server_internal(config, factory).await
}

/// Start the ASCOM Alpaca server with a custom serial port factory (non-mock)
#[cfg(not(feature = "mock"))]
pub async fn start_server_with_factory(
    config: Config,
    factory: Arc<dyn SerialPortFactory>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    start_server_internal(config, factory).await
}

/// Internal server startup logic
async fn start_server_internal(
    config: Config,
    factory: Arc<dyn SerialPortFactory>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut server = Server::new(CargoServerInfo!());
    server.listen_addr = SocketAddr::from(([0, 0, 0, 0], config.server.port));

    // Create shared serial manager
    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));

    // Register Switch device if enabled
    if config.switch.enabled {
        let switch_device =
            PpbaSwitchDevice::new(config.switch.clone(), Arc::clone(&serial_manager));
        server.devices.register(switch_device);
        info!(
            "Registered Switch device: {} (device number {})",
            config.switch.name, config.switch.device_number
        );
    }

    // Register ObservingConditions device if enabled
    if config.observingconditions.enabled {
        let oc_device = PpbaObservingConditionsDevice::new(
            config.observingconditions.clone(),
            Arc::clone(&serial_manager),
        );
        server.devices.register(oc_device);
        info!(
            "Registered ObservingConditions device: {} (device number {})",
            config.observingconditions.name, config.observingconditions.device_number
        );
    }

    info!(
        "Starting ASCOM Alpaca server on port {}",
        config.server.port
    );
    info!("Serial port: {}", config.serial.port);

    server.start().await?;

    Ok(())
}
