//! World struct for QHY-Focuser BDD tests

use std::sync::Arc;

use cucumber::World;
use qhy_focuser::io::SerialPortFactory;
use qhy_focuser::{Config, QhyFocuserDevice, SerialManager};

#[path = "mock_serial.rs"]
pub mod mock_serial;

#[derive(Debug, Default, World)]
pub struct QhyFocuserWorld {
    pub config: Option<Config>,
    pub device: Option<Arc<QhyFocuserDevice>>,
    pub serial_manager: Option<Arc<SerialManager>>,
    pub last_error: Option<String>,
    pub last_error_code: Option<u16>,
    pub position_result: Option<i32>,
    pub temperature_result: Option<f64>,
    pub is_moving_result: Option<bool>,
}

impl QhyFocuserWorld {
    /// Build a device with mock serial responses and a long polling interval
    /// (to avoid the background poller consuming mock responses).
    pub fn build_device_with_responses(&mut self, responses: Vec<String>) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::MockSerialPortFactory::new(responses));
        let mut config = self.config.clone().unwrap_or_default();
        // Use long polling interval to prevent background poller from consuming responses
        config.serial.polling_interval_ms = 60_000;
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        self.serial_manager = Some(Arc::clone(&serial_manager));
        self.device = Some(Arc::new(QhyFocuserDevice::new(
            config.focuser,
            serial_manager,
        )));
    }

    /// Build a device with a failing factory.
    pub fn build_device_with_failing_factory(&mut self, error_msg: &str) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::FailingFactory::new(error_msg));
        let config = self.config.clone().unwrap_or_default();
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        self.serial_manager = Some(Arc::clone(&serial_manager));
        self.device = Some(Arc::new(QhyFocuserDevice::new(
            config.focuser,
            serial_manager,
        )));
    }

    /// Build a serial manager directly (no device) with mock responses and long polling.
    pub fn build_manager_with_responses(&mut self, responses: Vec<String>) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::MockSerialPortFactory::new(responses));
        let mut config = self.config.clone().unwrap_or_default();
        config.serial.polling_interval_ms = 60_000;
        self.serial_manager = Some(Arc::new(SerialManager::new(config, factory)));
    }

    /// Build a serial manager with fast polling for polling tests.
    pub fn build_manager_with_fast_polling(&mut self, responses: Vec<String>) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::MockSerialPortFactory::new(responses));
        let mut config = self.config.clone().unwrap_or_default();
        config.serial.polling_interval_ms = 50;
        self.serial_manager = Some(Arc::new(SerialManager::new(config, factory)));
    }
}
