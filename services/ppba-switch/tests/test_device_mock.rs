//! Mock-based device tests for PPBA Switch driver

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ppba_switch::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use ppba_switch::{Config, PpbaSwitchDevice, Result};
use tokio::sync::Mutex;

/// Mock serial reader that returns predefined responses
struct MockSerialReader {
    responses: Arc<Mutex<Vec<String>>>,
    index: Arc<Mutex<usize>>,
}

impl MockSerialReader {
    fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            index: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl SerialReader for MockSerialReader {
    async fn read_line(&mut self) -> Result<Option<String>> {
        let responses = self.responses.lock().await;
        let mut index = self.index.lock().await;

        if *index < responses.len() {
            let response = responses[*index].clone();
            *index += 1;
            Ok(Some(response))
        } else {
            Ok(None)
        }
    }
}

/// Mock serial writer that records sent messages
struct MockSerialWriter {
    sent_messages: Arc<Mutex<Vec<String>>>,
}

impl MockSerialWriter {
    fn new() -> Self {
        Self {
            sent_messages: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl SerialWriter for MockSerialWriter {
    async fn write_message(&mut self, message: &str) -> Result<()> {
        let mut messages = self.sent_messages.lock().await;
        messages.push(message.to_string());
        Ok(())
    }
}

/// Mock serial port factory
struct MockSerialPortFactory {
    responses: Vec<String>,
}

impl MockSerialPortFactory {
    fn new(responses: Vec<String>) -> Self {
        Self { responses }
    }
}

#[async_trait]
impl SerialPortFactory for MockSerialPortFactory {
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        Ok(SerialPair {
            reader: Box::new(MockSerialReader::new(self.responses.clone())),
            writer: Box::new(MockSerialWriter::new()),
        })
    }

    async fn port_exists(&self, _port: &str) -> bool {
        true
    }
}

/// Create a mock factory with standard responses for connection
fn create_connected_mock_factory() -> MockSerialPortFactory {
    MockSerialPortFactory::new(vec![
        "PPBA_OK".to_string(),                                     // Ping response
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // Status
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // Power stats
    ])
}

#[tokio::test]
async fn test_device_creation() {
    let config = Config::default();
    let device = PpbaSwitchDevice::new(config);

    // Device should be created successfully
    assert!(!format!("{:?}", device).is_empty());
}

#[tokio::test]
async fn test_device_with_mock_factory() {
    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    assert!(!format!("{:?}", device).is_empty());
}

#[tokio::test]
async fn test_device_static_name() {
    use ascom_alpaca::api::Device;

    let mut config = Config::default();
    config.device.name = "Test PPBA".to_string();

    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    assert_eq!(device.static_name(), "Test PPBA");
}

#[tokio::test]
async fn test_device_unique_id() {
    use ascom_alpaca::api::Device;

    let mut config = Config::default();
    config.device.unique_id = "custom-id-123".to_string();

    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    assert_eq!(device.unique_id(), "custom-id-123");
}

#[tokio::test]
async fn test_device_description() {
    use ascom_alpaca::api::Device;

    let mut config = Config::default();
    config.device.description = "Custom description".to_string();

    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let description = device.description().await.unwrap();
    assert_eq!(description, "Custom description");
}

#[tokio::test]
async fn test_device_driver_info() {
    use ascom_alpaca::api::Device;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let info = device.driver_info().await.unwrap();
    assert!(info.contains("PPBA"));
    assert!(info.contains("Pegasus"));
}

#[tokio::test]
async fn test_device_driver_version() {
    use ascom_alpaca::api::Device;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let version = device.driver_version().await.unwrap();
    assert!(!version.is_empty());
}

#[tokio::test]
async fn test_device_initially_disconnected() {
    use ascom_alpaca::api::Device;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let connected = device.connected().await.unwrap();
    assert!(!connected);
}

#[tokio::test]
async fn test_max_switch_returns_16() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let max = device.max_switch().await.unwrap();
    assert_eq!(max, 16);
}

#[tokio::test]
async fn test_can_write_controllable_switches() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Switches 0-5 should be writable
    for id in 0..6 {
        let can_write = device.can_write(id).await.unwrap();
        assert!(can_write, "Switch {} should be writable", id);
    }
}

#[tokio::test]
async fn test_can_write_readonly_switches() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Switches 6-15 should be read-only
    for id in 6..16 {
        let can_write = device.can_write(id).await.unwrap();
        assert!(!can_write, "Switch {} should be read-only", id);
    }
}

#[tokio::test]
async fn test_can_write_invalid_switch() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let result = device.can_write(16).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_switch_name() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let name = device.get_switch_name(0).await.unwrap();
    assert_eq!(name, "Quad 12V Output");

    let name = device.get_switch_name(2).await.unwrap();
    assert_eq!(name, "Dew Heater A");
}

#[tokio::test]
async fn test_get_switch_description() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let desc = device.get_switch_description(0).await.unwrap();
    assert!(desc.contains("12V"));

    let desc = device.get_switch_description(12).await.unwrap();
    assert!(desc.contains("temperature") || desc.contains("Temperature"));
}

#[tokio::test]
async fn test_set_switch_name_not_supported() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let result = device.set_switch_name(0, "New Name".to_string()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_min_switch_value() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Boolean switch
    let min = device.min_switch_value(0).await.unwrap();
    assert_eq!(min, 0.0);

    // PWM switch
    let min = device.min_switch_value(2).await.unwrap();
    assert_eq!(min, 0.0);

    // Temperature sensor
    let min = device.min_switch_value(12).await.unwrap();
    assert_eq!(min, -40.0);
}

#[tokio::test]
async fn test_max_switch_value() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Boolean switch
    let max = device.max_switch_value(0).await.unwrap();
    assert_eq!(max, 1.0);

    // PWM switch
    let max = device.max_switch_value(2).await.unwrap();
    assert_eq!(max, 255.0);

    // Humidity sensor
    let max = device.max_switch_value(13).await.unwrap();
    assert_eq!(max, 100.0);
}

#[tokio::test]
async fn test_switch_step() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Boolean switch
    let step = device.switch_step(0).await.unwrap();
    assert_eq!(step, 1.0);

    // PWM switch
    let step = device.switch_step(2).await.unwrap();
    assert_eq!(step, 1.0);

    // Current sensor
    let step = device.switch_step(11).await.unwrap();
    assert_eq!(step, 0.01);
}

#[tokio::test]
async fn test_state_change_complete_always_true() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // State changes are synchronous, so always complete
    for id in 0..16 {
        let complete = device.state_change_complete(id).await.unwrap();
        assert!(complete);
    }
}

#[tokio::test]
async fn test_get_switch_when_disconnected() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Should fail when not connected
    let result = device.get_switch(0).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_switch_value_when_disconnected() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Should fail when not connected
    let result = device.get_switch_value(0).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_switch_when_disconnected() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Should fail when not connected
    let result = device.set_switch(0, true).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_switch_value_when_disconnected() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Should fail when not connected
    let result = device.set_switch_value(0, 1.0).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_invalid_switch_id_errors() {
    use ascom_alpaca::api::Switch;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // All metadata methods should error on invalid ID
    assert!(device.get_switch_name(100).await.is_err());
    assert!(device.get_switch_description(100).await.is_err());
    assert!(device.min_switch_value(100).await.is_err());
    assert!(device.max_switch_value(100).await.is_err());
    assert!(device.switch_step(100).await.is_err());
}

// ============================================================================
// Connected state tests
// ============================================================================

/// Create a mock factory with extended responses for connection and switch operations
fn create_connected_mock_factory_with_set_responses() -> MockSerialPortFactory {
    MockSerialPortFactory::new(vec![
        "PPBA_OK".to_string(),                                     // Ping
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // Initial status
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // Initial power stats
        // Additional responses for set operations
        "P1:1".to_string(), // Set quad 12V response
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // Status after set
        "P1:0".to_string(), // Set quad 12V off
        "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:1:0:0".to_string(), // Status after set
        "P2:1".to_string(), // Set adjustable
        "PPBA:12.5:3.2:25.0:60:15.5:0:1:128:64:1:0:0".to_string(), // Status after set
        "P3:200".to_string(), // Set dew A
        "PPBA:12.5:3.2:25.0:60:15.5:0:1:200:64:1:0:0".to_string(), // Status after set
        "P4:150".to_string(), // Set dew B
        "PPBA:12.5:3.2:25.0:60:15.5:0:1:200:150:1:0:0".to_string(), // Status after set
        "PU:1".to_string(), // Set USB hub
        "PD:0".to_string(), // Set auto-dew off
        "PPBA:12.5:3.2:25.0:60:15.5:0:1:200:150:0:0:0".to_string(), // Status after set
    ])
}

#[tokio::test]
async fn test_connect_and_disconnect() {
    use ascom_alpaca::api::Device;

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Initially disconnected
    assert!(!device.connected().await.unwrap());

    // Connect
    device.set_connected(true).await.unwrap();
    assert!(device.connected().await.unwrap());

    // Disconnect
    device.set_connected(false).await.unwrap();
    assert!(!device.connected().await.unwrap());
}

#[tokio::test]
async fn test_get_switch_value_quad_12v_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Quad 12V is switch 0, status has it as 1 (on)
    let value = device.get_switch_value(0).await.unwrap();
    assert_eq!(value, 1.0);

    let state = device.get_switch(0).await.unwrap();
    assert!(state);
}

#[tokio::test]
async fn test_get_switch_value_adjustable_output_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Adjustable output is switch 1, status has it as 0 (off)
    let value = device.get_switch_value(1).await.unwrap();
    assert_eq!(value, 0.0);

    let state = device.get_switch(1).await.unwrap();
    assert!(!state);
}

#[tokio::test]
async fn test_get_switch_value_dew_heater_a_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Dew Heater A is switch 2, status has it as 128
    let value = device.get_switch_value(2).await.unwrap();
    assert_eq!(value, 128.0);

    // get_switch returns true if value > min (0)
    let state = device.get_switch(2).await.unwrap();
    assert!(state);
}

#[tokio::test]
async fn test_get_switch_value_dew_heater_b_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Dew Heater B is switch 3, status has it as 64
    let value = device.get_switch_value(3).await.unwrap();
    assert_eq!(value, 64.0);
}

#[tokio::test]
async fn test_get_switch_value_usb_hub_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // USB hub is switch 4, defaults to false/0.0
    let value = device.get_switch_value(4).await.unwrap();
    assert_eq!(value, 0.0);
}

#[tokio::test]
async fn test_get_switch_value_auto_dew_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Auto-dew is switch 5, status has it as 1 (on)
    let value = device.get_switch_value(5).await.unwrap();
    assert_eq!(value, 1.0);
}

#[tokio::test]
async fn test_get_switch_value_average_current_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Average current is switch 6, power stats has it as 2.5A
    let value = device.get_switch_value(6).await.unwrap();
    assert_eq!(value, 2.5);
}

#[tokio::test]
async fn test_get_switch_value_amp_hours_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Amp hours is switch 7, power stats has it as 10.5Ah
    let value = device.get_switch_value(7).await.unwrap();
    assert_eq!(value, 10.5);
}

#[tokio::test]
async fn test_get_switch_value_watt_hours_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Watt hours is switch 8, power stats has it as 126.0Wh
    let value = device.get_switch_value(8).await.unwrap();
    assert_eq!(value, 126.0);
}

#[tokio::test]
async fn test_get_switch_value_uptime_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Uptime is switch 9, power stats has it as 3600000ms = 1 hour
    let value = device.get_switch_value(9).await.unwrap();
    assert_eq!(value, 1.0);
}

#[tokio::test]
async fn test_get_switch_value_input_voltage_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Input voltage is switch 10, status has it as 12.5V
    let value = device.get_switch_value(10).await.unwrap();
    assert_eq!(value, 12.5);
}

#[tokio::test]
async fn test_get_switch_value_total_current_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Total current is switch 11, status has it as 3.2A
    let value = device.get_switch_value(11).await.unwrap();
    assert_eq!(value, 3.2);
}

#[tokio::test]
async fn test_get_switch_value_temperature_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Temperature is switch 12, status has it as 25.0C
    let value = device.get_switch_value(12).await.unwrap();
    assert_eq!(value, 25.0);
}

#[tokio::test]
async fn test_get_switch_value_humidity_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Humidity is switch 13, status has it as 60%
    let value = device.get_switch_value(13).await.unwrap();
    assert_eq!(value, 60.0);
}

#[tokio::test]
async fn test_get_switch_value_dewpoint_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Dewpoint is switch 14, status has it as 15.5C
    let value = device.get_switch_value(14).await.unwrap();
    assert_eq!(value, 15.5);
}

#[tokio::test]
async fn test_get_switch_value_power_warning_when_connected() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Power warning is switch 15, status has it as 0 (no warning)
    let value = device.get_switch_value(15).await.unwrap();
    assert_eq!(value, 0.0);
}

#[tokio::test]
async fn test_set_switch_quad_12v_on() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory_with_set_responses());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Set quad 12V on (switch 0)
    device.set_switch(0, true).await.unwrap();
}

#[tokio::test]
async fn test_set_switch_quad_12v_off() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory_with_set_responses());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Use set_switch_value to set quad 12V off (switch 0)
    device.set_switch(0, true).await.unwrap();
    device.set_switch(0, false).await.unwrap();
}

#[tokio::test]
async fn test_set_switch_value_adjustable_output() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory_with_set_responses());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Skip to adjustable output operations
    device.set_switch(0, true).await.unwrap();
    device.set_switch(0, false).await.unwrap();

    // Set adjustable output on (switch 1)
    device.set_switch_value(1, 1.0).await.unwrap();
}

#[tokio::test]
async fn test_set_switch_value_dew_heater_a() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory_with_set_responses());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Skip previous operations
    device.set_switch(0, true).await.unwrap();
    device.set_switch(0, false).await.unwrap();
    device.set_switch_value(1, 1.0).await.unwrap();

    // Set dew heater A to 200 (switch 2)
    device.set_switch_value(2, 200.0).await.unwrap();
}

#[tokio::test]
async fn test_set_switch_value_dew_heater_b() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory_with_set_responses());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Skip previous operations
    device.set_switch(0, true).await.unwrap();
    device.set_switch(0, false).await.unwrap();
    device.set_switch_value(1, 1.0).await.unwrap();
    device.set_switch_value(2, 200.0).await.unwrap();

    // Set dew heater B to 150 (switch 3)
    device.set_switch_value(3, 150.0).await.unwrap();
}

#[tokio::test]
async fn test_set_switch_usb_hub() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory_with_set_responses());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Skip previous operations
    device.set_switch(0, true).await.unwrap();
    device.set_switch(0, false).await.unwrap();
    device.set_switch_value(1, 1.0).await.unwrap();
    device.set_switch_value(2, 200.0).await.unwrap();
    device.set_switch_value(3, 150.0).await.unwrap();

    // Set USB hub on (switch 4)
    device.set_switch(4, true).await.unwrap();
}

#[tokio::test]
async fn test_set_switch_auto_dew() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory_with_set_responses());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Skip previous operations
    device.set_switch(0, true).await.unwrap();
    device.set_switch(0, false).await.unwrap();
    device.set_switch_value(1, 1.0).await.unwrap();
    device.set_switch_value(2, 200.0).await.unwrap();
    device.set_switch_value(3, 150.0).await.unwrap();
    device.set_switch(4, true).await.unwrap();

    // Set auto-dew off (switch 5)
    device.set_switch(5, false).await.unwrap();
}

#[tokio::test]
async fn test_set_readonly_switch_fails() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Try to set a read-only switch (voltage - switch 10)
    let result = device.set_switch_value(10, 5.0).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_switch_value_out_of_range_fails() {
    use ascom_alpaca::api::{Device, Switch};

    let config = Config::default();
    let factory = Arc::new(create_connected_mock_factory());
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Try to set dew heater A to value above max (255)
    let result = device.set_switch_value(2, 300.0).await;
    assert!(result.is_err());

    // Try to set dew heater A to negative value
    let result = device.set_switch_value(2, -10.0).await;
    assert!(result.is_err());
}
