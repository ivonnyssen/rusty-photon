//! PPBA Switch Driver
//!
//! ASCOM Alpaca Switch driver for the Pegasus Astro Pocket Powerbox Advance Gen2 (PPBA).
//!
//! This driver exposes the following functionality via the ASCOM Switch interface:
//!
//! ## Controllable Switches (CanWrite = true)
//!
//! | ID | Name | Type | Description |
//! |----|------|------|-------------|
//! | 0 | Quad 12V Output | Boolean | Controls the quad 12V power output |
//! | 1 | Adjustable Output | Boolean | Controls the adjustable voltage output |
//! | 2 | Dew Heater A | Analog (0-255) | PWM control for Dew Heater A |
//! | 3 | Dew Heater B | Analog (0-255) | PWM control for Dew Heater B |
//! | 4 | USB Hub | Boolean | Controls the USB 2.0 hub power |
//! | 5 | Auto-Dew | Boolean | Enables automatic dew heater control |
//!
//! ## Read-Only Switches (CanWrite = false)
//!
//! | ID | Name | Type | Description |
//! |----|------|------|-------------|
//! | 6 | Average Current | Analog (A) | Average current draw |
//! | 7 | Amp Hours | Analog (Ah) | Cumulative amp-hours consumed |
//! | 8 | Watt Hours | Analog (Wh) | Cumulative watt-hours consumed |
//! | 9 | Uptime | Analog (hours) | Device uptime |
//! | 10 | Input Voltage | Analog (V) | Input voltage |
//! | 11 | Total Current | Analog (A) | Total current draw |
//! | 12 | Temperature | Analog (°C) | Ambient temperature |
//! | 13 | Humidity | Analog (%) | Relative humidity |
//! | 14 | Dewpoint | Analog (°C) | Calculated dewpoint |
//! | 15 | Power Warning | Boolean | Power warning flag |

pub mod config;
pub mod device;
pub mod error;
pub mod io;
pub mod protocol;
pub mod serial;
pub mod switches;

pub use config::{load_config, Config, DeviceConfig, SerialConfig, ServerConfig};
pub use device::PpbaSwitchDevice;
pub use error::{PpbaError, Result};
pub use switches::{get_switch_info, SwitchId, SwitchInfo, MAX_SWITCH};

use std::net::SocketAddr;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use tracing::info;

/// Start the ASCOM Alpaca server with the PPBA Switch device
pub async fn start_server(config: Config) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let device = PpbaSwitchDevice::new(config.clone());

    let mut server = Server::new(CargoServerInfo!());
    server.listen_addr = SocketAddr::from(([0, 0, 0, 0], config.server.port));
    server.devices.register(device);

    info!(
        "Starting ASCOM Alpaca server on port {}",
        config.server.port
    );
    info!(
        "Device: {} ({})",
        config.device.name, config.device.unique_id
    );
    info!("Serial port: {}", config.serial.port);

    server.start().await?;

    Ok(())
}
