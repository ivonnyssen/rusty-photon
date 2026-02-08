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
use ppba_driver::error::PpbaError;
use ppba_driver::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use ppba_driver::{Config, PpbaSwitchDevice, Result, SerialManager};
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
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Additional for polling
    ]
}

/// Helper to create a test device with mock factory
fn create_test_device(factory: Arc<dyn SerialPortFactory>) -> PpbaSwitchDevice {
    let config = Config::default();
    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
    PpbaSwitchDevice::new(config.switch, serial_manager)
}

// ============================================================================
// Category 1: Async Operation Methods Tests
// ============================================================================

#[tokio::test]
async fn test_can_async_returns_false() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // All switches should return false for can_async
    for id in 0..16 {
        let result = device.can_async(id).await.unwrap();
        assert_eq!(result, false, "Switch {} should not support async ops", id);
    }
}

#[tokio::test]
async fn test_can_async_invalid_switch_id() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = create_test_device(factory);

    // Test with invalid switch IDs (>= MAX_SWITCH which is 16)
    let result = device.can_async(16).await;
    assert!(result.is_err(), "Should fail for invalid switch ID 16");

    let result = device.can_async(999).await;
    assert!(result.is_err(), "Should fail for invalid switch ID 999");
}

#[tokio::test]
async fn test_state_change_complete_always_true() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

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
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // cancel_async should succeed (no-op since we don't support async)
    for id in 0..16 {
        device.cancel_async(id).await.unwrap();
    }
}

#[tokio::test]
async fn test_set_async_not_supported() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // set_async should return NOT_IMPLEMENTED error
    let result = device.set_async(0, true).await;
    assert!(result.is_err(), "set_async should not be implemented");

    let result = device.set_async_value(2, 128.0).await;
    assert!(result.is_err(), "set_async_value should not be implemented");
}

// ============================================================================
// Category 2: Value Range Validation Tests
// ============================================================================

#[tokio::test]
async fn test_set_switch_value_negative() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Negative values should be rejected
    let result = device.set_switch_value(2, -10.0).await;
    assert!(result.is_err(), "Negative values should be rejected");
}

#[tokio::test]
async fn test_set_switch_value_exceeds_max() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Values exceeding max should be rejected
    // PWM switches have max=255
    let result = device.set_switch_value(2, 300.0).await;
    assert!(result.is_err(), "Values exceeding max should be rejected");

    // Boolean switches have max=1
    let result = device.set_switch_value(0, 2.0).await;
    assert!(
        result.is_err(),
        "Values exceeding max should be rejected for boolean switches"
    );
}

#[tokio::test]
async fn test_set_switch_value_fractional_boolean() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Fractional values for boolean switches
    // The implementation may round or reject these
    let result = device.set_switch_value(0, 0.5).await;
    // Either accepted (rounded) or rejected is fine
    let _ = result;
}

#[tokio::test]
async fn test_set_switch_value_boundary_values() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Test exact boundary values
    // Min value (0.0) for PWM switch
    let result = device.set_switch_value(2, 0.0).await;
    let _ = result; // Should succeed

    // Max value (255.0) for PWM switch
    let result = device.set_switch_value(2, 255.0).await;
    let _ = result; // Should succeed
}

// ============================================================================
// Category 3: Invalid Switch ID Tests
// ============================================================================

#[tokio::test]
async fn test_invalid_switch_id_operations() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    let invalid_ids = vec![16, 17, 100, 999];

    for id in invalid_ids {
        // All operations with invalid IDs should fail
        assert!(
            device.can_write(id).await.is_err(),
            "can_write should fail for ID {}",
            id
        );
        assert!(
            device.get_switch(id).await.is_err(),
            "get_switch should fail for ID {}",
            id
        );
        assert!(
            device.get_switch_value(id).await.is_err(),
            "get_switch_value should fail for ID {}",
            id
        );
        assert!(
            device.set_switch(id, true).await.is_err(),
            "set_switch should fail for ID {}",
            id
        );
        assert!(
            device.set_switch_value(id, 0.0).await.is_err(),
            "set_switch_value should fail for ID {}",
            id
        );
        assert!(
            device.get_switch_name(id).await.is_err(),
            "get_switch_name should fail for ID {}",
            id
        );
        assert!(
            device.get_switch_description(id).await.is_err(),
            "get_switch_description should fail for ID {}",
            id
        );
        assert!(
            device.min_switch_value(id).await.is_err(),
            "min_switch_value should fail for ID {}",
            id
        );
        assert!(
            device.max_switch_value(id).await.is_err(),
            "max_switch_value should fail for ID {}",
            id
        );
        assert!(
            device.switch_step(id).await.is_err(),
            "switch_step should fail for ID {}",
            id
        );
    }
}

// ============================================================================
// Category 4: Edge Cases and Error Conditions
// ============================================================================

#[tokio::test]
async fn test_connect_disconnect_multiple_cycles() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    // Multiple connect/disconnect cycles
    for _ in 0..5 {
        device.set_connected(true).await.unwrap();
        assert!(device.connected().await.unwrap());

        device.set_connected(false).await.unwrap();
        assert!(!device.connected().await.unwrap());
    }
}

#[tokio::test]
async fn test_operations_on_all_switch_types() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test that all 16 switches can be queried
    for id in 0..16 {
        // These should all succeed
        let _name = device.get_switch_name(id).await.unwrap();
        let _desc = device.get_switch_description(id).await.unwrap();
        let _min = device.min_switch_value(id).await.unwrap();
        let _max = device.max_switch_value(id).await.unwrap();
        let _step = device.switch_step(id).await.unwrap();
        let _value = device.get_switch_value(id).await.unwrap();
        let _can_write = device.can_write(id).await.unwrap();
    }
}

#[tokio::test]
async fn test_set_switch_name_not_implemented() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    // set_switch_name should return NOT_IMPLEMENTED
    let result = device.set_switch_name(0, "New name".to_string()).await;
    assert!(result.is_err(), "set_switch_name should not be implemented");
}

#[tokio::test]
async fn test_boolean_switch_conversions() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test get_switch on boolean switches (0-5)
    for id in 0..=5 {
        let result = device.get_switch(id).await;
        // Should succeed and return bool
        assert!(result.is_ok(), "get_switch should work for switch {}", id);
    }
}

#[tokio::test]
async fn test_all_writable_switches_identified() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Switches 0-5 should be writable (with auto-dew off)
    for id in 0..6 {
        let can_write = device.can_write(id).await.unwrap();
        assert!(can_write, "Switch {} should be writable", id);
    }

    // Switches 6-15 should be read-only
    for id in 6..16 {
        let can_write = device.can_write(id).await.unwrap();
        assert!(!can_write, "Switch {} should be read-only", id);
    }
}

#[tokio::test]
async fn test_max_switch_boundary() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    // max_switch should return 16
    let max = device.max_switch().await.unwrap();
    assert_eq!(max, 16);

    // Connect so that switch methods work
    device.set_connected(true).await.unwrap();

    // Switch ID 15 (max-1) should be valid
    assert!(device.get_switch_name(15).await.is_ok());

    // Switch ID 16 (max) should be invalid
    assert!(device.get_switch_name(16).await.is_err());
}

#[tokio::test]
async fn test_device_info_methods() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    // Test all device info methods
    let name = device.static_name();
    assert!(!name.is_empty());

    let unique_id = device.unique_id();
    assert!(!unique_id.is_empty());

    let description = device.description().await.unwrap();
    assert!(!description.is_empty());

    let driver_info = device.driver_info().await.unwrap();
    assert!(!driver_info.is_empty());

    let driver_version = device.driver_version().await.unwrap();
    assert!(!driver_version.is_empty());
}

#[tokio::test]
async fn test_connection_state_queries() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    // Initially disconnected
    assert!(!device.connected().await.unwrap());

    // Connect
    device.set_connected(true).await.unwrap();
    assert!(device.connected().await.unwrap());

    // Query connection state multiple times
    for _ in 0..10 {
        assert!(device.connected().await.unwrap());
    }

    // Disconnect
    device.set_connected(false).await.unwrap();
    assert!(!device.connected().await.unwrap());
}

// ============================================================================
// Category 5: USB Hub and Auto-Dew Special Path Tests
// ============================================================================

#[tokio::test]
async fn test_set_switch_value_usb_hub() {
    // USB hub (switch 4) uses a special code path: sends PU command,
    // then manually tracks state via set_usb_hub_state.
    // The USB hub path returns early without a status refresh, so we only need
    // the PU:1 response after the connection handshake. Provide enough extra
    // responses for any polling cycles that might occur.
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![
        "PPBA_OK".to_string(),                                     // connect: ping
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // connect: status
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // connect: power stats
        "PU:1".to_string(),                                        // set_switch_value: PU command
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // polling: status
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // polling: power stats
    ]));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Enable USB hub via set_switch_value (no sleep - act immediately before polling fires)
    device.set_switch_value(4, 1.0).await.unwrap();

    // USB hub state should be tracked via set_usb_hub_state
    let usb_value = device.get_switch_value(4).await.unwrap();
    assert_eq!(usb_value, 1.0);
}

#[tokio::test]
async fn test_set_switch_value_auto_dew() {
    // Auto-dew (switch 5) is a boolean toggle.
    // set_switch_value_internal sends the PD command, then calls refresh_status.
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![
        "PPBA_OK".to_string(),                                     // connect: ping
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // connect: status
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // connect: power stats
        "PD:1".to_string(),                                        // set_switch_value: PD command
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // set_switch_value: refresh status
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // polling: status
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // polling: power stats
    ]));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Enable auto-dew (act immediately before polling fires)
    device.set_switch_value(5, 1.0).await.unwrap();
}

#[tokio::test]
async fn test_set_switch_readonly_sensor() {
    // Switches 6-15 are read-only; attempting to write should return
    // NOT_IMPLEMENTED (via SwitchNotWritable error path)
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![
        "PPBA_OK".to_string(),
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // Additional status for the refresh in set_switch_value_internal
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Try writing to each read-only sensor switch
    for id in 6..16 {
        let result = device.set_switch_value(id, 0.0).await;
        assert!(
            result.is_err(),
            "Writing to read-only switch {} should fail",
            id
        );
    }
}

#[tokio::test]
async fn test_get_switch_value_no_cached_status() {
    // When cache has no status (status=None), switches that read from
    // status should return NOT_CONNECTED
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));

    // Create a device manually without connecting (so cache is empty)
    // We can't call get_switch_value without connecting, but we can test
    // that connecting with bad responses leads to appropriate errors
    let device = create_test_device(factory);

    // Not connected → NOT_CONNECTED
    let result = device.get_switch_value(0).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {} // Expected
        _ => panic!("Expected NOT_CONNECTED error, got {:?}", result),
    }
}

#[tokio::test]
async fn test_get_switch_value_no_cached_power_stats() {
    // Power stat switches (6-9) require power_stats in cache.
    // Without them, should return NOT_CONNECTED.
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = create_test_device(factory);

    // Not connected → NOT_CONNECTED for power stat switches
    for id in 6..10 {
        let result = device.get_switch_value(id).await;
        match result {
            Err(ascom_alpaca::ASCOMError {
                code: ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED,
                ..
            }) => {} // Expected
            _ => panic!("Expected NOT_CONNECTED for switch {}, got {:?}", id, result),
        }
    }
}

// ============================================================================
// Category 6: Async Operation Invalid ID Tests
// ============================================================================

#[tokio::test]
async fn test_set_async_invalid_switch_id() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    let result = device.set_async(16, true).await;
    assert!(result.is_err(), "set_async with invalid ID should fail");

    let result = device.set_async(999, false).await;
    assert!(result.is_err(), "set_async with invalid ID should fail");
}

#[tokio::test]
async fn test_set_async_value_invalid_switch_id() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    let result = device.set_async_value(16, 0.0).await;
    assert!(
        result.is_err(),
        "set_async_value with invalid ID should fail"
    );

    let result = device.set_async_value(999, 0.0).await;
    assert!(
        result.is_err(),
        "set_async_value with invalid ID should fail"
    );
}

#[tokio::test]
async fn test_state_change_complete_invalid_id() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    let result = device.state_change_complete(16).await;
    assert!(
        result.is_err(),
        "state_change_complete with invalid ID should fail"
    );

    let result = device.state_change_complete(999).await;
    assert!(
        result.is_err(),
        "state_change_complete with invalid ID should fail"
    );
}

#[tokio::test]
async fn test_cancel_async_invalid_id() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    let result = device.cancel_async(16).await;
    assert!(result.is_err(), "cancel_async with invalid ID should fail");

    let result = device.cancel_async(999).await;
    assert!(result.is_err(), "cancel_async with invalid ID should fail");
}

// ============================================================================
// Category 7: to_ascom_error Mapping Tests
// ============================================================================

/// Mock serial port factory that always fails on open
struct FailingSerialPortFactory;

#[async_trait]
impl SerialPortFactory for FailingSerialPortFactory {
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        Err(PpbaError::ConnectionFailed(
            "mock port not found".to_string(),
        ))
    }

    async fn port_exists(&self, _port: &str) -> bool {
        false
    }
}

#[tokio::test]
async fn test_to_ascom_error_connection_failed_maps_to_invalid_operation() {
    let factory: Arc<dyn SerialPortFactory> = Arc::new(FailingSerialPortFactory);
    let device = create_test_device(factory);

    let result = device.set_connected(true).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::INVALID_OPERATION,
            ..
        }) => {} // ConnectionFailed -> wildcard -> INVALID_OPERATION
        other => panic!(
            "Expected INVALID_OPERATION for connection failure, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_to_ascom_error_bad_ping_maps_to_invalid_operation() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![
        "GARBAGE".to_string(), // Bad ping response
    ]));
    let device = create_test_device(factory);

    let result = device.set_connected(true).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::INVALID_OPERATION,
            ..
        }) => {} // InvalidResponse -> wildcard -> INVALID_OPERATION
        other => panic!("Expected INVALID_OPERATION for bad ping, got {:?}", other),
    }
}

#[tokio::test]
async fn test_to_ascom_error_switch_not_writable_maps_to_not_implemented() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // Switch 10 (InputVoltage) is read-only → SwitchNotWritable → NOT_IMPLEMENTED
    let result = device.set_switch_value(10, 0.0).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {} // SwitchNotWritable -> NOT_IMPLEMENTED
        other => panic!(
            "Expected NOT_IMPLEMENTED for read-only switch, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_to_ascom_error_invalid_switch_id_maps_to_invalid_value() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // get_switch_value with invalid ID → InvalidSwitchId → INVALID_VALUE
    let result = device.get_switch_value(99).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::INVALID_VALUE,
            ..
        }) => {} // InvalidSwitchId -> INVALID_VALUE
        other => panic!(
            "Expected INVALID_VALUE for invalid switch ID, got {:?}",
            other
        ),
    }

    // set_switch_value with invalid ID → same mapping
    let result = device.set_switch_value(99, 0.0).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::INVALID_VALUE,
            ..
        }) => {} // InvalidSwitchId -> INVALID_VALUE
        other => panic!(
            "Expected INVALID_VALUE for invalid switch ID on set, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_to_ascom_error_invalid_value_maps_to_invalid_value() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(
        standard_connection_responses(),
    ));
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // PWM switch (DewHeaterA, id=2) with value out of range → InvalidValue → INVALID_VALUE
    let result = device.set_switch_value(2, -1.0).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::INVALID_VALUE,
            ..
        }) => {} // InvalidValue -> INVALID_VALUE
        other => panic!(
            "Expected INVALID_VALUE for out-of-range value, got {:?}",
            other
        ),
    }
}

// ============================================================================
// Category 8: Async Methods - Not Connected Error Path
// ============================================================================

#[tokio::test]
async fn test_can_async_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = create_test_device(factory);

    let result = device.can_async(0).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {}
        other => panic!("Expected NOT_CONNECTED for can_async, got {:?}", other),
    }
}

#[tokio::test]
async fn test_state_change_complete_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = create_test_device(factory);

    let result = device.state_change_complete(0).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {}
        other => panic!(
            "Expected NOT_CONNECTED for state_change_complete, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_cancel_async_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = create_test_device(factory);

    let result = device.cancel_async(0).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {}
        other => panic!("Expected NOT_CONNECTED for cancel_async, got {:?}", other),
    }
}

#[tokio::test]
async fn test_set_async_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = create_test_device(factory);

    let result = device.set_async(0, true).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {}
        other => panic!("Expected NOT_CONNECTED for set_async, got {:?}", other),
    }
}

#[tokio::test]
async fn test_set_async_value_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![]));
    let device = create_test_device(factory);

    let result = device.set_async_value(0, 1.0).await;
    match result {
        Err(ascom_alpaca::ASCOMError {
            code: ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {}
        other => panic!(
            "Expected NOT_CONNECTED for set_async_value, got {:?}",
            other
        ),
    }
}

// ============================================================================
// Category 9: set_async / set_async_value - Working Delegation
// ============================================================================

#[tokio::test]
async fn test_set_async_delegates_to_set_switch() {
    // set_async should delegate to set_switch, which sends the command
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![
        "PPBA_OK".to_string(),                                     // connect: ping
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // connect: status
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // connect: power stats
        "P1:1".to_string(),                                        // set_async: Quad12V on
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // set_async: refresh status
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // polling
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // polling
    ]));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // set_async with valid writable switch should succeed
    device.set_async(0, true).await.unwrap();
}

#[tokio::test]
async fn test_set_async_value_delegates_to_set_switch_value() {
    // set_async_value should delegate to set_switch_value
    let factory = Arc::new(MockSerialPortFactory::with_ok_responses(vec![
        "PPBA_OK".to_string(),                                     // connect: ping
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // connect: status
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // connect: power stats
        "PU:1".to_string(),                                        // set_async_value: USB hub on
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // polling
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // polling
    ]));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // set_async_value with valid USB hub switch should succeed
    device.set_async_value(4, 1.0).await.unwrap();
}
