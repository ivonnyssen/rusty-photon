//! Additional device tests to improve code coverage
//!
//! This test file targets specific uncovered code paths in device.rs:
//! 1. Async operation methods (ASCOM Switch interface)
//! 2. Value range validation error paths
//! 3. Background polling error scenarios

use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Device, Switch};
use async_trait::async_trait;
use ppba_switch::error::PpbaError;
use ppba_switch::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use ppba_switch::{Config, PpbaSwitchDevice, Result};
use tokio::sync::Mutex;

// ============================================================================
// Mock Serial Infrastructure
// ============================================================================

/// Mock serial reader that can return errors
struct MockSerialReader {
    responses: Arc<Mutex<Vec<String>>>,
    error_indices: Arc<Mutex<Vec<usize>>>,
    index: Arc<Mutex<usize>>,
}

impl MockSerialReader {
    fn new(responses: Vec<String>, error_indices: Vec<usize>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            error_indices: Arc::new(Mutex::new(error_indices)),
            index: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl SerialReader for MockSerialReader {
    async fn read_line(&mut self) -> Result<Option<String>> {
        let responses = self.responses.lock().await;
        let error_indices = self.error_indices.lock().await;
        let mut index = self.index.lock().await;

        if *index < responses.len() {
            let current_index = *index;
            *index += 1;

            // Check if this index should return an error
            if error_indices.contains(&current_index) {
                return Err(PpbaError::Communication(format!(
                    "Simulated error at index {}",
                    current_index
                )));
            }

            let response = responses[current_index].clone();
            Ok(Some(response))
        } else {
            Ok(None)
        }
    }
}

/// Mock serial writer
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
    error_indices: Vec<usize>,
}

impl MockSerialPortFactory {
    fn new(responses: Vec<String>, error_indices: Vec<usize>) -> Self {
        Self {
            responses,
            error_indices,
        }
    }

    fn with_ok_responses(responses: Vec<String>) -> Self {
        Self::new(responses, vec![])
    }
}

#[async_trait]
impl SerialPortFactory for MockSerialPortFactory {
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        Ok(SerialPair {
            reader: Box::new(MockSerialReader::new(
                self.responses.clone(),
                self.error_indices.clone(),
            )),
            writer: Box::new(MockSerialWriter::new()),
        })
    }

    async fn port_exists(&self, _port: &str) -> bool {
        true
    }
}

/// Standard responses for a connected device with auto-dew OFF
fn standard_connection_responses() -> Vec<String> {
    vec![
        "PPBA_OK".to_string(),                                     // Ping
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Status (auto-dew=0)
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // Power stats
    ]
}

// ============================================================================
// Category 1: Async Operation Methods Tests
// ============================================================================

#[tokio::test]
async fn test_can_async_returns_false() {
    let config = Config::default();
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // All switches should return false for can_async
    for id in 0..16 {
        let result = device.can_async(id).await.unwrap();
        assert_eq!(result, false, "Switch {} should not support async ops", id);
    }
}

#[tokio::test]
async fn test_can_async_invalid_switch_id() {
    let config = Config::default();
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Test with invalid switch IDs (>= MAX_SWITCH which is 16)
    let result = device.can_async(16).await;
    assert!(result.is_err(), "Should fail for invalid switch ID 16");

    let result = device.can_async(999).await;
    assert!(result.is_err(), "Should fail for invalid switch ID 999");
}

#[tokio::test]
async fn test_state_change_complete_always_true() {
    let config = Config::default();
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // All switches should return true for state_change_complete (no async ops)
    for id in 0..16 {
        let result = device.state_change_complete(id).await.unwrap();
        assert_eq!(
            result, true,
            "Switch {} state change should always be complete",
            id
        );
    }
}

#[tokio::test]
async fn test_cancel_async_succeeds() {
    let config = Config::default();
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // cancel_async should succeed (no-op since we don't support async)
    for id in 0..16 {
        let result = device.cancel_async(id).await;
        assert!(
            result.is_ok(),
            "cancel_async should succeed for switch {}",
            id
        );
    }
}

#[tokio::test]
async fn test_cancel_async_invalid_switch_id() {
    let config = Config::default();
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let result = device.cancel_async(999).await;
    assert!(result.is_err(), "Should fail for invalid switch ID");
}

#[tokio::test]
async fn test_set_async_delegates_to_sync() {
    let config = Config::default();
    let mut responses = standard_connection_responses();
    // Add responses for set operation
    responses.push("P1:1".to_string()); // SetQuad12V(true) response
    responses.push("PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string()); // Refresh status

    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(responses));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // set_async should delegate to set_switch
    device.set_async(0, true).await.unwrap();
}

#[tokio::test]
async fn test_set_async_invalid_switch_id() {
    let config = Config::default();
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let result = device.set_async(999, true).await;
    assert!(result.is_err(), "Should fail for invalid switch ID");
}

#[tokio::test]
async fn test_set_async_value_delegates_to_sync() {
    let config = Config::default();
    let mut responses = standard_connection_responses();
    // Add responses for dew heater set operation (switch 2 = DewHeaterA uses P3 command)
    responses.push("PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string()); // Refresh before write
    responses.push("P3:100".to_string()); // SetDewA response (P3, not P2)
    responses.push("PPBA:12.5:3.2:25.0:60:15.5:1:0:100:64:0:0:0".to_string()); // Refresh after write

    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(responses));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // set_async_value should delegate to set_switch_value
    device.set_async_value(2, 100.0).await.unwrap();
}

#[tokio::test]
async fn test_set_async_value_invalid_switch_id() {
    let config = Config::default();
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    let result = device.set_async_value(999, 100.0).await;
    assert!(result.is_err(), "Should fail for invalid switch ID");
}

// ============================================================================
// Category 2: Value Range Validation Tests
// ============================================================================

#[tokio::test]
async fn test_set_value_below_minimum() {
    let config = Config::default();
    let mut responses = standard_connection_responses();
    // Add response for dew heater status refresh (happens before range check)
    responses.push("PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string());

    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(responses));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Try to set dew heater A (switch 2, range 0-255) to negative value
    let result = device.set_switch_value(2, -10.0).await;
    assert!(
        result.is_err(),
        "Should fail when setting value below minimum"
    );

    let err = result.unwrap_err();
    assert!(
        err.message.contains("out of range"),
        "Error should mention range: {}",
        err.message
    );
}

#[tokio::test]
async fn test_set_value_above_maximum() {
    let config = Config::default();
    let mut responses = standard_connection_responses();
    // Add response for dew heater status refresh
    responses.push("PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string());

    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(responses));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Try to set dew heater A (switch 2, range 0-255) above maximum
    let result = device.set_switch_value(2, 300.0).await;
    assert!(
        result.is_err(),
        "Should fail when setting value above maximum"
    );

    let err = result.unwrap_err();
    assert!(
        err.message.contains("out of range"),
        "Error should mention range: {}",
        err.message
    );
}

#[tokio::test]
async fn test_set_value_exactly_at_boundaries() {
    let config = Config::default();
    let mut responses = standard_connection_responses();
    // Need 4 refresh responses (2 sets * 2 refreshes each: before write + after write)
    // DewHeaterA is switch 2, uses P3 command
    responses.push("PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string()); // Refresh before min
    responses.push("P3:0".to_string()); // SetDewA(0) - uses P3, not P2
    responses.push("PPBA:12.5:3.2:25.0:60:15.5:1:0:0:64:0:0:0".to_string()); // Refresh after min
    responses.push("PPBA:12.5:3.2:25.0:60:15.5:1:0:0:64:0:0:0".to_string()); // Refresh before max
    responses.push("P3:255".to_string()); // SetDewA(255)
    responses.push("PPBA:12.5:3.2:25.0:60:15.5:1:0:255:64:0:0:0".to_string()); // Refresh after max

    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(responses));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Setting to exact minimum (0) should succeed
    let result = device.set_switch_value(2, 0.0).await;
    assert!(result.is_ok(), "Should succeed at minimum value");

    // Setting to exact maximum (255) should succeed
    let result = device.set_switch_value(2, 255.0).await;
    assert!(result.is_ok(), "Should succeed at maximum value");
}

// ============================================================================
// Category 3: Background Polling Error Scenarios
// ============================================================================

#[tokio::test]
async fn test_polling_continues_on_status_error() {
    use tokio::time::sleep;

    let mut config = Config::default();
    config.serial.polling_interval_ms = 50; // Fast polling for test

    let responses = vec![
        // Connection
        "PPBA_OK".to_string(),
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // First poll - status fails (index 3)
        "ERROR".to_string(),                     // Will be marked as error
        "PS:3.0:11.0:130.0:3700000".to_string(), // Power stats succeeds
        // Second poll - both succeed to verify recovery
        "PPBA:13.0:3.5:26.0:62:16.0:1:0:130:65:0:0:0".to_string(),
        "PS:3.5:12.0:140.0:3800000".to_string(),
    ];

    let error_indices = vec![3]; // Status poll at index 3 fails

    let factory = Arc::new(MockSerialPortFactory::new(responses, error_indices));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Wait for at least 2 polling cycles
    sleep(Duration::from_millis(150)).await;

    // Device should still be connected despite status error
    assert!(
        device.connected().await.unwrap(),
        "Device should remain connected after status poll error"
    );

    // Disconnect to stop polling
    device.set_connected(false).await.unwrap();
}

#[tokio::test]
async fn test_polling_continues_on_power_stats_error() {
    use tokio::time::sleep;

    let mut config = Config::default();
    config.serial.polling_interval_ms = 50;

    let responses = vec![
        // Connection
        "PPBA_OK".to_string(),
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // First poll - power stats fails (index 4)
        "PPBA:13.0:3.5:26.0:62:16.0:1:0:130:65:0:0:0".to_string(),
        "ERROR".to_string(), // Will be marked as error
        // Second poll - both succeed
        "PPBA:13.5:3.7:27.0:64:17.0:1:0:135:70:0:0:0".to_string(),
        "PS:4.0:13.0:150.0:3900000".to_string(),
    ];

    let error_indices = vec![4]; // Power stats poll at index 4 fails

    let factory = Arc::new(MockSerialPortFactory::new(responses, error_indices));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    device.set_connected(true).await.unwrap();

    // Wait for polling cycles
    sleep(Duration::from_millis(150)).await;

    // Device should still be connected
    assert!(
        device.connected().await.unwrap(),
        "Device should remain connected after power stats poll error"
    );

    device.set_connected(false).await.unwrap();
}

#[tokio::test]
async fn test_polling_stops_on_disconnect() {
    use tokio::time::sleep;

    let mut config = Config::default();
    config.serial.polling_interval_ms = 50;

    let responses = vec![
        // Connection
        "PPBA_OK".to_string(),
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // First poll cycle
        "PPBA:13.0:3.5:26.0:62:16.0:1:0:130:65:0:0:0".to_string(),
        "PS:3.0:11.0:130.0:3700000".to_string(),
        // Extra responses that shouldn't be used after disconnect
        "PPBA:14.0:4.0:28.0:65:18.0:1:0:140:75:0:0:0".to_string(),
        "PS:4.0:12.0:140.0:3800000".to_string(),
    ];

    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(responses));
    let device = PpbaSwitchDevice::with_serial_factory(config, factory);

    // Connect and start polling
    device.set_connected(true).await.unwrap();

    // Wait for one poll cycle
    sleep(Duration::from_millis(75)).await;

    // Disconnect - this should stop polling
    device.set_connected(false).await.unwrap();

    // Wait to ensure polling would have happened if it were still running
    sleep(Duration::from_millis(150)).await;

    // Verify device is disconnected
    assert!(
        !device.connected().await.unwrap(),
        "Device should be disconnected"
    );
}
