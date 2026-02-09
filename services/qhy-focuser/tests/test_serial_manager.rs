//! Tests for the SerialManager module

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use qhy_focuser::error::QhyFocuserError;
use qhy_focuser::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use qhy_focuser::{Config, Result, SerialManager};
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

struct FailingFactory {
    error_msg: String,
}

impl FailingFactory {
    fn new(error_msg: &str) -> Self {
        Self {
            error_msg: error_msg.to_string(),
        }
    }
}

#[async_trait]
impl SerialPortFactory for FailingFactory {
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        Err(QhyFocuserError::ConnectionFailed(self.error_msg.clone()))
    }

    async fn port_exists(&self, _port: &str) -> bool {
        false
    }
}

/// Standard responses: version + position + temperature (enough for handshake)
/// Then additional position+temp pairs for polling cycles
fn standard_connection_responses() -> Vec<String> {
    vec![
        // Handshake: version response
        r#"{"idx": 1, "firmware_version": "2.1.0", "board_version": "1.0"}"#.to_string(),
        // Handshake: position response
        r#"{"idx": 5, "position": 10000}"#.to_string(),
        // Handshake: temperature response
        r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string(),
        // Polling: position
        r#"{"idx": 5, "position": 10000}"#.to_string(),
        // Polling: temperature
        r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string(),
        // Extra polling responses
        r#"{"idx": 5, "position": 10000}"#.to_string(),
        r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string(),
    ]
}

fn create_manager(factory: Arc<dyn SerialPortFactory>) -> SerialManager {
    let config = Config::default();
    SerialManager::new(config, factory)
}

// ============================================================================
// Creation & Initialization Tests
// ============================================================================

#[tokio::test]
async fn test_new_creates_manager() {
    let factory = Arc::new(MockSerialPortFactory::new(vec![]));
    let manager = create_manager(factory);

    let debug_str = format!("{:?}", manager);
    assert!(debug_str.contains("SerialManager"));
}

#[tokio::test]
async fn test_initially_not_available() {
    let factory = Arc::new(MockSerialPortFactory::new(vec![]));
    let manager = create_manager(factory);

    assert!(!manager.is_available());
}

// ============================================================================
// Connection Lifecycle Tests
// ============================================================================

#[tokio::test]
async fn test_connect_first_device() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();
    assert!(manager.is_available());

    manager.disconnect().await;
}

#[tokio::test]
async fn test_connect_second_device_increments_refcount() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();
    assert!(manager.is_available());

    manager.connect().await.unwrap();
    assert!(manager.is_available());

    // First disconnect: still available
    manager.disconnect().await;
    assert!(manager.is_available());

    // Second disconnect: now closed
    manager.disconnect().await;
    assert!(!manager.is_available());
}

#[tokio::test]
async fn test_disconnect_at_zero_is_noop() {
    let factory = Arc::new(MockSerialPortFactory::new(vec![]));
    let manager = create_manager(factory);

    // Should not panic
    manager.disconnect().await;
    assert!(!manager.is_available());
}

#[tokio::test]
async fn test_connect_failure() {
    let factory = Arc::new(FailingFactory::new("port busy"));
    let manager = create_manager(factory);

    let err = manager.connect().await.unwrap_err();
    assert!(err.to_string().contains("port busy"));
    assert!(!manager.is_available());
}

// ============================================================================
// Cached State Tests
// ============================================================================

#[tokio::test]
async fn test_cached_state_after_connect() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();

    let state = manager.get_cached_state().await;
    assert_eq!(state.position, Some(10000));
    assert!((state.outer_temp.unwrap() - 25.0).abs() < 0.001);
    assert!((state.chip_temp.unwrap() - 30.0).abs() < 0.001);
    assert!((state.voltage.unwrap() - 12.5).abs() < 0.001);
    assert_eq!(state.firmware_version, Some("2.1.0".to_string()));
    assert_eq!(state.board_version, Some("1.0".to_string()));
    assert!(!state.is_moving);

    manager.disconnect().await;
}

#[tokio::test]
async fn test_cached_state_default() {
    let factory = Arc::new(MockSerialPortFactory::new(vec![]));
    let manager = create_manager(factory);

    let state = manager.get_cached_state().await;
    assert_eq!(state.position, None);
    assert_eq!(state.outer_temp, None);
    assert!(!state.is_moving);
    assert_eq!(state.firmware_version, None);
}

// ============================================================================
// Command Sending Tests
// ============================================================================

#[tokio::test]
async fn test_send_command_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(vec![]));
    let manager = create_manager(factory);

    let err = manager
        .send_command(qhy_focuser::protocol::Command::GetPosition)
        .await
        .unwrap_err();
    match err {
        QhyFocuserError::NotConnected => {}
        other => panic!("Expected NotConnected, got {:?}", other),
    }
}

#[tokio::test]
async fn test_move_absolute_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(vec![]));
    let manager = create_manager(factory);

    let err = manager.move_absolute(5000).await.unwrap_err();
    match err {
        QhyFocuserError::NotConnected => {}
        other => panic!("Expected NotConnected, got {:?}", other),
    }
}

#[tokio::test]
async fn test_abort_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(vec![]));
    let manager = create_manager(factory);

    let err = manager.abort().await.unwrap_err();
    match err {
        QhyFocuserError::NotConnected => {}
        other => panic!("Expected NotConnected, got {:?}", other),
    }
}

#[tokio::test]
async fn test_move_absolute_sets_state() {
    let mut responses = standard_connection_responses();
    // Add response for the absolute move command
    responses.push(r#"{"idx": 6}"#.to_string());
    // Add extra polling responses
    responses.push(r#"{"idx": 5, "position": 10000}"#.to_string());
    responses.push(r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string());

    let factory = Arc::new(MockSerialPortFactory::new(responses));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();
    manager.move_absolute(20000).await.unwrap();

    let state = manager.get_cached_state().await;
    assert!(state.is_moving);
    assert_eq!(state.target_position, Some(20000));

    manager.disconnect().await;
}

#[tokio::test]
async fn test_abort_clears_moving_state() {
    let mut responses = standard_connection_responses();
    // Move command response
    responses.push(r#"{"idx": 6}"#.to_string());
    // Abort command response
    responses.push(r#"{"idx": 3}"#.to_string());
    // Extra polling
    responses.push(r#"{"idx": 5, "position": 15000}"#.to_string());
    responses.push(r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string());

    let factory = Arc::new(MockSerialPortFactory::new(responses));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();
    manager.move_absolute(20000).await.unwrap();

    let state = manager.get_cached_state().await;
    assert!(state.is_moving);

    manager.abort().await.unwrap();

    let state = manager.get_cached_state().await;
    assert!(!state.is_moving);
    assert_eq!(state.target_position, None);

    manager.disconnect().await;
}
