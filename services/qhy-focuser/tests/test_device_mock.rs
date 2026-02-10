//! Mock-based device tests for QHY Q-Focuser

use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Device, Focuser};
use ascom_alpaca::ASCOMErrorCode;
use async_trait::async_trait;
use qhy_focuser::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use qhy_focuser::{Config, QhyFocuserDevice, Result, SerialManager};
use tokio::sync::Mutex;

// ============================================================================
// Mock Serial Infrastructure
// ============================================================================

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
            *index = 0;
            if !responses.is_empty() {
                Ok(Some(responses[0].clone()))
            } else {
                Ok(None)
            }
        }
    }
}

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

/// Standard handshake responses: version + set_speed + position + temperature
fn standard_connection_responses() -> Vec<String> {
    vec![
        r#"{"idx": 1, "firmware_version": "2.1.0", "board_version": "1.0"}"#.to_string(),
        r#"{"idx": 13}"#.to_string(),
        r#"{"idx": 5, "pos": 10000}"#.to_string(),
        r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string(),
        // Polling responses
        r#"{"idx": 5, "pos": 10000}"#.to_string(),
        r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string(),
        r#"{"idx": 5, "pos": 10000}"#.to_string(),
        r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string(),
    ]
}

fn create_test_device(factory: Arc<dyn SerialPortFactory>) -> QhyFocuserDevice {
    let mut config = Config::default();
    // Use long polling interval to prevent background poller from consuming mock responses
    config.serial.polling_interval_ms = 60_000;
    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
    QhyFocuserDevice::new(config.focuser, serial_manager)
}

fn create_test_device_with_config(
    config: Config,
    factory: Arc<dyn SerialPortFactory>,
) -> QhyFocuserDevice {
    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
    QhyFocuserDevice::new(config.focuser, serial_manager)
}

// ============================================================================
// Device trait tests
// ============================================================================

#[tokio::test]
async fn test_device_creation() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    assert!(!format!("{:?}", device).is_empty());
}

#[tokio::test]
async fn test_device_static_name() {
    let mut config = Config::default();
    config.focuser.name = "Test Focuser".to_string();
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device_with_config(config, factory);
    assert_eq!(device.static_name(), "Test Focuser");
}

#[tokio::test]
async fn test_device_unique_id() {
    let mut config = Config::default();
    config.focuser.unique_id = "custom-id-123".to_string();
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device_with_config(config, factory);
    assert_eq!(device.unique_id(), "custom-id-123");
}

#[tokio::test]
async fn test_device_description() {
    let mut config = Config::default();
    config.focuser.description = "Custom description".to_string();
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device_with_config(config, factory);
    let description = device.description().await.unwrap();
    assert_eq!(description, "Custom description");
}

#[tokio::test]
async fn test_device_driver_info() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    let info = device.driver_info().await.unwrap();
    assert!(info.contains("QHY"));
    assert!(info.contains("Focuser"));
}

#[tokio::test]
async fn test_device_driver_version() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    let version = device.driver_version().await.unwrap();
    assert!(!version.is_empty());
}

#[tokio::test]
async fn test_device_not_connected_initially() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    assert!(!device.connected().await.unwrap());
}

#[tokio::test]
async fn test_device_connect_disconnect() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    assert!(device.connected().await.unwrap());

    device.set_connected(false).await.unwrap();
    assert!(!device.connected().await.unwrap());
}

#[tokio::test]
async fn test_device_connect_idempotent() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    assert!(device.connected().await.unwrap());

    // Connecting again should be fine
    device.set_connected(true).await.unwrap();
    assert!(device.connected().await.unwrap());

    device.set_connected(false).await.unwrap();
}

// ============================================================================
// Focuser trait tests
// ============================================================================

#[tokio::test]
async fn test_focuser_absolute() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    assert!(device.absolute().await.unwrap());
}

#[tokio::test]
async fn test_focuser_max_step() {
    let mut config = Config::default();
    config.focuser.max_step = 100_000;
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device_with_config(config, factory);
    assert_eq!(device.max_step().await.unwrap(), 100_000);
}

#[tokio::test]
async fn test_focuser_max_increment() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    assert_eq!(device.max_increment().await.unwrap(), 64_000);
}

#[tokio::test]
async fn test_focuser_temp_comp() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    assert!(!device.temp_comp().await.unwrap());
}

#[tokio::test]
async fn test_focuser_temp_comp_available() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    assert!(!device.temp_comp_available().await.unwrap());
}

#[tokio::test]
async fn test_focuser_set_temp_comp_not_implemented() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    let err = device.set_temp_comp(true).await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn test_focuser_step_size_not_implemented() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    let err = device.step_size().await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn test_focuser_position_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    let err = device.position().await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
}

#[tokio::test]
async fn test_focuser_position_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    let position = device.position().await.unwrap();
    assert_eq!(position, 10000);

    device.set_connected(false).await.unwrap();
}

#[tokio::test]
async fn test_focuser_temperature_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    let temp = device.temperature().await.unwrap();
    assert!((temp - 25.0).abs() < 0.001);

    device.set_connected(false).await.unwrap();
}

#[tokio::test]
async fn test_focuser_temperature_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    let err = device.temperature().await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
}

#[tokio::test]
async fn test_focuser_is_moving_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    let err = device.is_moving().await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
}

#[tokio::test]
async fn test_focuser_is_moving_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    assert!(!device.is_moving().await.unwrap());

    device.set_connected(false).await.unwrap();
}

#[tokio::test]
async fn test_focuser_move_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    let err = device.move_(5000).await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
}

#[tokio::test]
#[cfg(feature = "mock")]
async fn test_focuser_move_valid_position() {
    let factory = Arc::new(qhy_focuser::MockSerialPortFactory::default());
    let mut config = Config::default();
    config.serial.polling_interval_ms = 60_000;
    let device = create_test_device_with_config(config, factory);
    device.set_connected(true).await.unwrap();

    // Mock starts at position 0, move to 20000
    device.move_(20000).await.unwrap();
    // Mock AbsoluteMove sets target but doesn't move immediately,
    // so is_moving should be true (position != target)
    assert!(device.is_moving().await.unwrap());

    device.set_connected(false).await.unwrap();
}

#[tokio::test]
async fn test_focuser_move_negative_position_rejected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    let err = device.move_(-1).await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);

    device.set_connected(false).await.unwrap();
}

#[tokio::test]
async fn test_focuser_move_over_max_rejected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    let err = device.move_(100_000).await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);

    device.set_connected(false).await.unwrap();
}

#[tokio::test]
async fn test_focuser_halt_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let device = create_test_device(factory);
    let err = device.halt().await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
}

#[tokio::test]
#[cfg(feature = "mock")]
async fn test_focuser_halt_connected() {
    let factory = Arc::new(qhy_focuser::MockSerialPortFactory::default());
    let mut config = Config::default();
    config.serial.polling_interval_ms = 60_000;
    let device = create_test_device_with_config(config, factory);
    device.set_connected(true).await.unwrap();

    device.move_(20000).await.unwrap();
    assert!(device.is_moving().await.unwrap());

    device.halt().await.unwrap();
    assert!(!device.is_moving().await.unwrap());

    device.set_connected(false).await.unwrap();
}
