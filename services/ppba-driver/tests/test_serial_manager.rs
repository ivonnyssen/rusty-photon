//! Unit tests for SerialManager internal API methods
//!
//! These tests exercise internal SerialManager methods (send_command, refresh,
//! cached state, averaging period) that are not directly exposed through the
//! ASCOM device interface. Connection lifecycle tests are in the BDD feature
//! `connection_lifecycle.feature`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ppba_driver::error::PpbaError;
use ppba_driver::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use ppba_driver::{Config, Result, SerialManager};
use tokio::sync::Mutex;

// ============================================================================
// Mock Serial Infrastructure
// ============================================================================

/// Mock serial reader that returns predefined responses in order
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

/// Standard responses: ping + status + power stats (enough for one connect)
fn standard_connection_responses() -> Vec<String> {
    vec![
        "PPBA_OK".to_string(),
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // Extra responses for polling cycles
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]
}

/// Helper to create a SerialManager with default config and given factory
fn create_manager(factory: Arc<dyn SerialPortFactory>) -> SerialManager {
    let config = Config::default();
    SerialManager::new(config, factory)
}

// ============================================================================
// Command Sending Tests
// ============================================================================

#[tokio::test]
async fn test_send_command_when_not_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(vec![]));
    let manager = create_manager(factory);

    let result = manager
        .send_command(ppba_driver::protocol::PpbaCommand::Ping)
        .await;

    match result {
        Err(PpbaError::NotConnected) => {} // Expected
        other => panic!("Expected NotConnected error, got {:?}", other),
    }
}

#[tokio::test]
async fn test_send_command_when_connected() {
    let factory = Arc::new(MockSerialPortFactory::new(vec![
        "PPBA_OK".to_string(),
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // Response to our send_command
        "P1:1".to_string(),
        // Polling responses
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();

    let response = manager
        .send_command(ppba_driver::protocol::PpbaCommand::SetQuad12V(true))
        .await
        .unwrap();
    assert!(response.starts_with("P1:"));

    manager.disconnect().await;
}

// ============================================================================
// Status Refresh Tests
// ============================================================================

#[tokio::test]
async fn test_refresh_status_updates_cache() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();

    let cached = manager.get_cached_state().await;
    let status = cached.status.unwrap();
    assert!((status.temperature - 25.0).abs() < 0.01);
    assert!((status.humidity - 60.0).abs() < 0.01);
    assert!((status.dewpoint - 15.5).abs() < 0.01);

    manager.disconnect().await;
}

#[tokio::test]
async fn test_refresh_status_updates_sensor_means() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();

    let cached = manager.get_cached_state().await;
    // After connect, refresh_status was called, so means should have samples
    let temp_mean = cached.temp_mean.get_mean().unwrap();
    assert!((temp_mean - 25.0).abs() < 0.01);

    let humidity_mean = cached.humidity_mean.get_mean().unwrap();
    assert!((humidity_mean - 60.0).abs() < 0.01);

    let dewpoint_mean = cached.dewpoint_mean.get_mean().unwrap();
    assert!((dewpoint_mean - 15.5).abs() < 0.01);

    manager.disconnect().await;
}

#[tokio::test]
async fn test_refresh_power_stats_updates_cache() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();

    let cached = manager.get_cached_state().await;
    let stats = cached.power_stats.unwrap();
    assert!((stats.average_amps - 2.5).abs() < 0.01);
    assert!((stats.amp_hours - 10.5).abs() < 0.01);
    assert!((stats.watt_hours - 126.0).abs() < 0.01);

    manager.disconnect().await;
}

// ============================================================================
// Utility Method Tests
// ============================================================================

#[tokio::test]
async fn test_set_averaging_period() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();

    let new_window = Duration::from_secs(120);
    manager.set_averaging_period(new_window).await;

    let cached = manager.get_cached_state().await;
    assert_eq!(cached.temp_mean.window(), new_window);
    assert_eq!(cached.humidity_mean.window(), new_window);
    assert_eq!(cached.dewpoint_mean.window(), new_window);

    manager.disconnect().await;
}

#[tokio::test]
async fn test_set_usb_hub_state() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();

    // Default should be false
    let cached = manager.get_cached_state().await;
    assert!(!cached.usb_hub_enabled);

    // Enable USB hub
    manager.set_usb_hub_state(true).await;
    let cached = manager.get_cached_state().await;
    assert!(cached.usb_hub_enabled);

    // Disable USB hub
    manager.set_usb_hub_state(false).await;
    let cached = manager.get_cached_state().await;
    assert!(!cached.usb_hub_enabled);

    manager.disconnect().await;
}

#[tokio::test]
async fn test_get_cached_state_returns_clone() {
    let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
    let manager = create_manager(factory);

    manager.connect().await.unwrap();

    // Get two snapshots
    let state1 = manager.get_cached_state().await;
    let state2 = manager.get_cached_state().await;

    // Both should have the same data (they're snapshots of the same state)
    assert_eq!(
        state1.status.as_ref().unwrap().temperature,
        state2.status.as_ref().unwrap().temperature
    );

    manager.disconnect().await;
}
