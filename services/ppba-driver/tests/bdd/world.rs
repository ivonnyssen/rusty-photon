//! World struct for PPBA Driver BDD tests

use std::sync::Arc;

use cucumber::World;
use ppba_driver::io::SerialPortFactory;
use ppba_driver::{Config, PpbaObservingConditionsDevice, PpbaSwitchDevice, SerialManager};

#[path = "mock_serial.rs"]
pub mod mock_serial;

#[derive(Debug, Default, World)]
pub struct PpbaWorld {
    pub config: Option<Config>,
    pub switch_device: Option<Arc<PpbaSwitchDevice>>,
    pub oc_device: Option<Arc<PpbaObservingConditionsDevice>>,
    pub serial_manager: Option<Arc<SerialManager>>,
    pub last_error: Option<String>,
    pub last_error_code: Option<u16>,
    #[cfg(feature = "mock")]
    pub server_port: Option<u16>,
    #[cfg(feature = "mock")]
    pub server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl PpbaWorld {
    /// Build a switch device with mock serial responses and a long polling interval.
    pub fn build_switch_device_with_responses(&mut self, responses: Vec<String>) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::MockSerialPortFactory::new(responses));
        let mut config = self.config.clone().unwrap_or_default();
        config.serial.polling_interval_ms = 60_000;
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        self.serial_manager = Some(Arc::clone(&serial_manager));
        self.switch_device = Some(Arc::new(PpbaSwitchDevice::new(
            config.switch,
            serial_manager,
        )));
    }

    /// Build a switch device with custom config and mock responses.
    pub fn build_switch_device_with_config_and_responses(
        &mut self,
        config: Config,
        responses: Vec<String>,
    ) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::MockSerialPortFactory::new(responses));
        let mut config = config;
        config.serial.polling_interval_ms = 60_000;
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        self.serial_manager = Some(Arc::clone(&serial_manager));
        self.switch_device = Some(Arc::new(PpbaSwitchDevice::new(
            config.switch,
            serial_manager,
        )));
    }

    /// Build an OC device with mock serial responses.
    ///
    /// Uses default polling interval (5s) to preserve the correct averaging window
    /// calculation (polling_interval_ms * 60 = 300_000ms = 5 min). Responses must
    /// include poller-tick padding (status + PS pair) after the connect sequence.
    pub fn build_oc_device_with_responses(&mut self, responses: Vec<String>) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::MockSerialPortFactory::new(responses));
        let config = self.config.clone().unwrap_or_default();
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        self.serial_manager = Some(Arc::clone(&serial_manager));
        self.oc_device = Some(Arc::new(PpbaObservingConditionsDevice::new(
            config.observingconditions,
            serial_manager,
        )));
    }

    /// Build an OC device with custom config and mock responses.
    ///
    /// Uses default polling interval to preserve the correct averaging window.
    pub fn build_oc_device_with_config_and_responses(
        &mut self,
        config: Config,
        responses: Vec<String>,
    ) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::MockSerialPortFactory::new(responses));
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        self.serial_manager = Some(Arc::clone(&serial_manager));
        self.oc_device = Some(Arc::new(PpbaObservingConditionsDevice::new(
            config.observingconditions,
            serial_manager,
        )));
    }

    /// Abort the server task if one was spawned (server_registration scenarios).
    #[cfg(feature = "mock")]
    fn abort_server(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
    }

    /// Build a serial manager directly (no device) with mock responses and long polling.
    pub fn build_manager_with_responses(&mut self, responses: Vec<String>) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::MockSerialPortFactory::new(responses));
        let mut config = self.config.clone().unwrap_or_default();
        config.serial.polling_interval_ms = 60_000;
        self.serial_manager = Some(Arc::new(SerialManager::new(config, factory)));
    }

    /// Build a switch device with a failing factory.
    pub fn build_switch_device_with_failing_factory(&mut self, error_msg: &str) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::FailingFactory::new(error_msg));
        let config = self.config.clone().unwrap_or_default();
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        self.serial_manager = Some(Arc::clone(&serial_manager));
        self.switch_device = Some(Arc::new(PpbaSwitchDevice::new(
            config.switch,
            serial_manager,
        )));
    }

    /// Build an OC device with a failing factory.
    pub fn build_oc_device_with_failing_factory(&mut self, error_msg: &str) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::FailingFactory::new(error_msg));
        let config = self.config.clone().unwrap_or_default();
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        self.serial_manager = Some(Arc::clone(&serial_manager));
        self.oc_device = Some(Arc::new(PpbaObservingConditionsDevice::new(
            config.observingconditions,
            serial_manager,
        )));
    }

    /// Build a serial manager with a failing factory.
    pub fn build_manager_with_failing_factory(&mut self, error_msg: &str) {
        let factory: Arc<dyn SerialPortFactory> =
            Arc::new(mock_serial::FailingFactory::new(error_msg));
        let config = self.config.clone().unwrap_or_default();
        self.serial_manager = Some(Arc::new(SerialManager::new(config, factory)));
    }

    /// Build a switch device with bad ping response.
    pub fn build_switch_device_with_bad_ping(&mut self) {
        self.build_switch_device_with_responses(vec!["GARBAGE".to_string()]);
    }

    /// Build an OC device with bad ping response.
    pub fn build_oc_device_with_bad_ping(&mut self) {
        self.build_oc_device_with_responses(vec!["GARBAGE".to_string()]);
    }
}

#[cfg(feature = "mock")]
impl Drop for PpbaWorld {
    fn drop(&mut self) {
        self.abort_server();
    }
}
