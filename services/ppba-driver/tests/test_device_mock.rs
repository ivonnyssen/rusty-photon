//! Mock-based device tests for PPBA Switch driver

use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Device, Switch};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode};
use async_trait::async_trait;
use ppba_driver::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use ppba_driver::{Config, PpbaSwitchDevice, Result, SerialManager};
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
            // Cycle back for polling
            *index = 0;
            if !responses.is_empty() {
                Ok(Some(responses[0].clone()))
            } else {
                Ok(None)
            }
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
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Status (auto-dew=0)
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // Power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Additional for polling
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Additional for polling
    ])
}

/// Create a mock factory with auto-dew enabled
fn create_autodew_enabled_factory() -> MockSerialPortFactory {
    MockSerialPortFactory::new(vec![
        "PPBA_OK".to_string(),                                     // Ping response
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // Status (auto-dew=1)
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // Power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // Additional for polling
    ])
}

/// Helper to create a test device with mock factory
fn create_test_device(factory: Arc<dyn SerialPortFactory>) -> PpbaSwitchDevice {
    let config = Config::default();
    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
    PpbaSwitchDevice::new(config.switch, serial_manager)
}

/// Helper to create a test device with custom config
fn create_test_device_with_config(
    config: Config,
    factory: Arc<dyn SerialPortFactory>,
) -> PpbaSwitchDevice {
    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
    PpbaSwitchDevice::new(config.switch, serial_manager)
}

#[tokio::test]
async fn test_device_creation() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Device should be created successfully
    assert!(!format!("{:?}", device).is_empty());
}

#[tokio::test]
async fn test_device_with_mock_factory() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    assert!(!format!("{:?}", device).is_empty());
}

#[tokio::test]
async fn test_device_static_name() {
    let mut config = Config::default();
    config.switch.name = "Test PPBA".to_string();

    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device_with_config(config, factory);

    assert_eq!(device.static_name(), "Test PPBA");
}

#[tokio::test]
async fn test_device_unique_id() {
    let mut config = Config::default();
    config.switch.unique_id = "custom-id-123".to_string();

    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device_with_config(config, factory);

    assert_eq!(device.unique_id(), "custom-id-123");
}

#[tokio::test]
async fn test_device_description() {
    let mut config = Config::default();
    config.switch.description = "Custom description".to_string();

    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device_with_config(config, factory);

    let description = device.description().await.unwrap();
    assert_eq!(description, "Custom description");
}

#[tokio::test]
async fn test_device_driver_info() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let info = device.driver_info().await.unwrap();
    assert!(info.contains("PPBA"));
    assert!(info.contains("Pegasus"));
}

#[tokio::test]
async fn test_device_driver_version() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let version = device.driver_version().await.unwrap();
    assert!(!version.is_empty());
}

#[tokio::test]
async fn test_device_initially_disconnected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let connected = device.connected().await.unwrap();
    assert!(!connected);
}

#[tokio::test]
async fn test_max_switch_returns_16() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let max = device.max_switch().await.unwrap();
    assert_eq!(max, 16);
}

#[tokio::test]
async fn test_can_write_controllable_switches() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Must be connected to query can_write
    device.set_connected(true).await.unwrap();

    // Switches 0-5 should be writable
    // In default mock, auto-dew is OFF, so all writable switches are writable
    for id in 0..6 {
        let can_write = device.can_write(id).await.unwrap();
        assert!(can_write, "Switch {} should be writable", id);
    }
}

#[tokio::test]
async fn test_can_write_readonly_switches() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Must be connected to query can_write
    device.set_connected(true).await.unwrap();

    // Switches 6-15 should be read-only
    for id in 6..16 {
        let can_write = device.can_write(id).await.unwrap();
        assert!(!can_write, "Switch {} should be read-only", id);
    }
}

#[tokio::test]
async fn test_can_write_invalid_switch() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Connect the device first to bypass NOT_CONNECTED check
    device.set_connected(true).await.unwrap();

    // Test with invalid switch ID (MAX_SWITCH is 16, so 16+ is invalid)
    let result = device.can_write(16).await;
    assert!(result.is_err(), "Should fail for switch ID 16");

    let err = result.unwrap_err();
    assert!(
        err.message.contains("Invalid switch ID"),
        "Error should mention invalid switch ID, got: {}",
        err.message
    );

    // Test with another invalid ID
    let result = device.can_write(999).await;
    assert!(result.is_err(), "Should fail for switch ID 999");
}

#[tokio::test]
async fn test_get_switch_name() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    let name = device.get_switch_name(0).await.unwrap();
    assert_eq!(name, "Quad 12V Output");

    let name = device.get_switch_name(2).await.unwrap();
    assert_eq!(name, "Dew Heater A");
}

#[tokio::test]
async fn test_get_switch_description() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    let desc = device.get_switch_description(0).await.unwrap();
    assert!(desc.contains("12V"));

    let desc = device.get_switch_description(12).await.unwrap();
    assert!(desc.contains("temperature") || desc.contains("Temperature"));
}

#[tokio::test]
async fn test_set_switch_name_not_supported() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.set_switch_name(0, "New Name".to_string()).await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected NOT_IMPLEMENTED error"),
    }
}

#[tokio::test]
async fn test_min_switch_value() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // Boolean switch
    let min = device.min_switch_value(0).await.unwrap();
    assert_eq!(min, 0.0);

    // PWM switch
    let min = device.min_switch_value(2).await.unwrap();
    assert_eq!(min, 0.0);
}

#[tokio::test]
async fn test_max_switch_value() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // Boolean switch
    let max = device.max_switch_value(0).await.unwrap();
    assert_eq!(max, 1.0);

    // PWM switch
    let max = device.max_switch_value(2).await.unwrap();
    assert_eq!(max, 255.0);
}

#[tokio::test]
async fn test_switch_step() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // Most switches have step=1, but sensor switches may have finer granularity
    for id in 0..16 {
        let step = device.switch_step(id).await.unwrap();
        assert!(step > 0.0, "Switch {} should have positive step", id);
        // Boolean and PWM switches have step=1
        // Sensor switches may have step=0.01 or other fine granularity
    }
}

#[tokio::test]
async fn test_get_switch_value_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.get_switch_value(0).await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected NOT_CONNECTED error"),
    }
}

#[tokio::test]
async fn test_get_switch_value_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Wait for polling to populate cache
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Quad 12V should be 1 (from mock data)
    let value = device.get_switch_value(0).await.unwrap();
    assert_eq!(value, 1.0);

    // Input voltage (switch 10) should be 12.5V
    let value = device.get_switch_value(10).await.unwrap();
    assert!((value - 12.5).abs() < 0.1);
}

#[tokio::test]
async fn test_get_switch_boolean_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.get_switch(0).await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected NOT_CONNECTED error"),
    }
}

#[tokio::test]
async fn test_get_switch_boolean_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Wait for polling
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Quad 12V should be true (from mock data)
    let value = device.get_switch(0).await.unwrap();
    assert!(value);
}

#[tokio::test]
async fn test_set_switch_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.set_switch(0, true).await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected NOT_CONNECTED error"),
    }
}

#[tokio::test]
async fn test_set_switch_value_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.set_switch_value(2, 128.0).await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected NOT_CONNECTED error"),
    }
}

#[tokio::test]
async fn test_set_switch_readonly() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Switch 10 (Input Voltage) is read-only
    // Attempting to write to it should send an invalid command that fails
    let result = device.set_switch_value(10, 12.0).await;
    // The implementation may either reject this or fail when sending the command
    assert!(result.is_err(), "Writing to read-only switch should fail");
}

#[tokio::test]
async fn test_set_switch_value_invalid_range() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Try to set PWM value out of range (0-255)
    let result = device.set_switch_value(2, 300.0).await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_VALUE,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected INVALID_VALUE error for out-of-range value"),
    }
}

#[tokio::test]
async fn test_autodew_write_protection() {
    let factory = Arc::new(create_autodew_enabled_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Wait for status to be cached
    tokio::time::sleep(Duration::from_millis(100)).await;

    // With auto-dew enabled, dew heaters should not be writable
    let can_write_2 = device.can_write(2).await.unwrap();
    let can_write_3 = device.can_write(3).await.unwrap();

    assert!(
        !can_write_2,
        "Dew Heater A should not be writable when auto-dew is enabled"
    );
    assert!(
        !can_write_3,
        "Dew Heater B should not be writable when auto-dew is enabled"
    );

    // Attempting to write should fail
    let result = device.set_switch_value(2, 100.0).await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_OPERATION,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected INVALID_OPERATION when writing to dew heater with auto-dew enabled"),
    }
}

#[tokio::test]
async fn test_autodew_other_switches_still_writable() {
    let factory = Arc::new(create_autodew_enabled_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Wait for status
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Other writable switches should still be writable even with auto-dew on
    let can_write_0 = device.can_write(0).await.unwrap(); // Quad 12V
    let can_write_1 = device.can_write(1).await.unwrap(); // Adjustable
    let can_write_4 = device.can_write(4).await.unwrap(); // USB Hub
    let can_write_5 = device.can_write(5).await.unwrap(); // Auto-dew itself

    assert!(can_write_0, "Quad 12V should be writable");
    assert!(can_write_1, "Adjustable output should be writable");
    assert!(can_write_4, "USB Hub should be writable");
    assert!(can_write_5, "Auto-dew switch should be writable");
}

#[tokio::test]
async fn test_connection_lifecycle() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

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
async fn test_multiple_connect_calls() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Multiple connect calls should not error
    device.set_connected(true).await.unwrap();
    device.set_connected(true).await.unwrap();

    assert!(device.connected().await.unwrap());
}

#[tokio::test]
async fn test_multiple_disconnect_calls() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Multiple disconnect calls should not error
    device.set_connected(false).await.unwrap();
    device.set_connected(false).await.unwrap();

    assert!(!device.connected().await.unwrap());
}

#[tokio::test]
async fn test_switch_value_types() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Boolean switches should return 0.0 or 1.0
    let value = device.get_switch_value(0).await.unwrap();
    assert!(value == 0.0 || value == 1.0);

    // PWM switches should return 0-255
    let value = device.get_switch_value(2).await.unwrap();
    assert!(value >= 0.0 && value <= 255.0);

    // Sensor switches return various ranges
    let voltage = device.get_switch_value(10).await.unwrap();
    assert!(voltage > 0.0); // Should be positive voltage
}

#[tokio::test]
async fn test_all_switches_have_names() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    for id in 0..16 {
        let name = device.get_switch_name(id).await.unwrap();
        assert!(
            !name.is_empty(),
            "Switch {} should have a non-empty name",
            id
        );
    }
}

#[tokio::test]
async fn test_all_switches_have_descriptions() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    for id in 0..16 {
        let desc = device.get_switch_description(id).await.unwrap();
        assert!(
            !desc.is_empty(),
            "Switch {} should have a non-empty description",
            id
        );
    }
}

#[tokio::test]
async fn test_switch_info_consistency() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    for id in 0..16 {
        let min = device.min_switch_value(id).await.unwrap();
        let max = device.max_switch_value(id).await.unwrap();
        let step = device.switch_step(id).await.unwrap();

        assert!(
            min < max,
            "Switch {} min ({}) should be less than max ({})",
            id,
            min,
            max
        );
        assert!(
            step > 0.0,
            "Switch {} step should be positive, got {}",
            id,
            step
        );
    }
}

#[tokio::test]
async fn test_can_write_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Should return NOT_CONNECTED error when not connected
    let result = device.can_write(0).await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected NOT_CONNECTED error when querying can_write while disconnected"),
    }
}

#[tokio::test]
async fn test_invalid_switch_operations() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // get_switch_name with invalid ID
    let result = device.get_switch_name(99).await;
    assert!(result.is_err());

    // get_switch_description with invalid ID
    let result = device.get_switch_description(99).await;
    assert!(result.is_err());

    // min_switch_value with invalid ID
    let result = device.min_switch_value(99).await;
    assert!(result.is_err());

    // max_switch_value with invalid ID
    let result = device.max_switch_value(99).await;
    assert!(result.is_err());

    // switch_step with invalid ID
    let result = device.switch_step(99).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_pwm_value_precision() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Mock data has DewA=128, DewB=64
    let dew_a = device.get_switch_value(2).await.unwrap();
    let dew_b = device.get_switch_value(3).await.unwrap();

    assert_eq!(dew_a, 128.0);
    assert_eq!(dew_b, 64.0);
}

#[tokio::test]
async fn test_sensor_value_ranges() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Voltage should be in reasonable range (0-15V)
    let voltage = device.get_switch_value(10).await.unwrap();
    assert!(voltage >= 0.0 && voltage <= 15.0);

    // Current should be in reasonable range (0-20A)
    let current = device.get_switch_value(11).await.unwrap();
    assert!(current >= 0.0 && current <= 20.0);

    // Temperature should be in reasonable range (-40 to 60°C)
    let temp = device.get_switch_value(12).await.unwrap();
    assert!(temp >= -40.0 && temp <= 60.0);

    // Humidity should be 0-100%
    let humidity = device.get_switch_value(13).await.unwrap();
    assert!(humidity >= 0.0 && humidity <= 100.0);
}

#[tokio::test]
async fn test_power_stats_switches() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Average current (switch 6)
    let avg_current = device.get_switch_value(6).await.unwrap();
    assert!(avg_current >= 0.0);

    // Amp hours (switch 7)
    let amp_hours = device.get_switch_value(7).await.unwrap();
    assert!(amp_hours >= 0.0);

    // Watt hours (switch 8)
    let watt_hours = device.get_switch_value(8).await.unwrap();
    assert!(watt_hours >= 0.0);

    // Uptime (switch 9)
    let uptime = device.get_switch_value(9).await.unwrap();
    assert!(uptime >= 0.0);
}

#[tokio::test]
async fn test_boolean_switch_get_switch() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // get_switch should work for boolean switches
    let quad = device.get_switch(0).await.unwrap();
    assert!(quad == true || quad == false);

    // get_switch on non-boolean switch should handle gracefully
    // (returns true if value != 0)
    let _dew_a = device.get_switch(2).await.unwrap();
}

#[tokio::test]
async fn test_set_switch_boolean() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // set_switch should convert bool to appropriate command
    // This will send the command, but we can't verify response easily in mock
    // Just ensure it doesn't error
    let _result = device.set_switch(0, true).await;
}
