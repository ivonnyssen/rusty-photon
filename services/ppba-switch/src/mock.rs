//! Mock serial port implementation for testing
//!
//! This module provides mock implementations of the serial I/O traits
//! that simulate PPBA device responses, allowing the driver to be tested
//! without real hardware.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::debug;

use crate::error::Result;
use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};

/// Shared state between mock reader and writer
#[derive(Debug)]
struct MockState {
    /// Queue of responses to return
    response_queue: Vec<String>,
    /// Current device state for generating realistic responses
    device_state: MockDeviceState,
}

/// Simulated device state
#[derive(Debug, Clone)]
struct MockDeviceState {
    quad_12v: bool,
    adjustable: bool,
    dew_a: u8,
    dew_b: u8,
    usb_hub: bool,
    auto_dew: bool,
    voltage: f64,
    current: f64,
    temperature: f64,
    humidity: f64,
    dewpoint: f64,
    power_warning: bool,
    average_amps: f64,
    amp_hours: f64,
    watt_hours: f64,
    uptime_ms: u64,
}

impl Default for MockDeviceState {
    fn default() -> Self {
        Self {
            quad_12v: true,
            adjustable: false,
            dew_a: 128,
            dew_b: 64,
            usb_hub: false,
            auto_dew: true,
            voltage: 12.5,
            current: 3.2,
            temperature: 25.0,
            humidity: 60.0,
            dewpoint: 15.5,
            power_warning: false,
            average_amps: 2.5,
            amp_hours: 10.5,
            watt_hours: 126.0,
            uptime_ms: 3600000,
        }
    }
}

impl MockDeviceState {
    /// Generate status response (PA command)
    fn status_response(&self) -> String {
        format!(
            "PPBA:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            self.voltage,
            self.current,
            self.temperature,
            self.humidity as u8,
            self.dewpoint,
            if self.quad_12v { 1 } else { 0 },
            if self.adjustable { 1 } else { 0 },
            self.dew_a,
            self.dew_b,
            if self.auto_dew { 1 } else { 0 },
            if self.power_warning { 1 } else { 0 },
            0 // power adjust
        )
    }

    /// Generate power stats response (PS command)
    fn power_stats_response(&self) -> String {
        format!(
            "PS:{}:{}:{}:{}",
            self.average_amps, self.amp_hours, self.watt_hours, self.uptime_ms
        )
    }
}

impl MockState {
    fn new() -> Self {
        Self {
            response_queue: Vec::new(),
            device_state: MockDeviceState::default(),
        }
    }

    /// Process a command and queue the appropriate response
    fn process_command(&mut self, command: &str) {
        let command = command.trim();
        debug!("Mock processing command: {}", command);

        let response = if command == "P#" {
            // Ping
            "PPBA_OK".to_string()
        } else if command == "PA" {
            // Status
            self.device_state.status_response()
        } else if command == "PS" {
            // Power stats
            self.device_state.power_stats_response()
        } else if command == "PV" {
            // Firmware version
            "1.0.0".to_string()
        } else if let Some(value) = command.strip_prefix("P1:") {
            // Set quad 12V
            let state = value == "1";
            self.device_state.quad_12v = state;
            format!("P1:{}", if state { 1 } else { 0 })
        } else if let Some(value) = command.strip_prefix("P2:") {
            // Set adjustable output
            let state = value == "1";
            self.device_state.adjustable = state;
            format!("P2:{}", if state { 1 } else { 0 })
        } else if let Some(value) = command.strip_prefix("P3:") {
            // Set dew A PWM
            if let Ok(pwm) = value.parse::<u8>() {
                self.device_state.dew_a = pwm;
                format!("P3:{}", pwm)
            } else {
                "ERR".to_string()
            }
        } else if let Some(value) = command.strip_prefix("P4:") {
            // Set dew B PWM
            if let Ok(pwm) = value.parse::<u8>() {
                self.device_state.dew_b = pwm;
                format!("P4:{}", pwm)
            } else {
                "ERR".to_string()
            }
        } else if let Some(value) = command.strip_prefix("PU:") {
            // Set USB hub
            let state = value == "1";
            self.device_state.usb_hub = state;
            format!("PU:{}", if state { 1 } else { 0 })
        } else if let Some(value) = command.strip_prefix("PD:") {
            // Set auto-dew
            let state = value == "1";
            self.device_state.auto_dew = state;
            format!("PD:{}", if state { 1 } else { 0 })
        } else {
            debug!("Mock: unknown command '{}'", command);
            "ERR".to_string()
        };

        debug!("Mock queuing response: {}", response);
        self.response_queue.push(response);
    }

    /// Get the next response from the queue
    fn next_response(&mut self) -> Option<String> {
        if self.response_queue.is_empty() {
            None
        } else {
            Some(self.response_queue.remove(0))
        }
    }
}

/// Mock serial reader that returns command-appropriate responses
pub struct MockSerialReader {
    state: Arc<Mutex<MockState>>,
}

impl MockSerialReader {
    /// Create a new mock reader with shared state
    fn new(state: Arc<Mutex<MockState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl SerialReader for MockSerialReader {
    async fn read_line(&mut self) -> Result<Option<String>> {
        let mut state = self.state.lock().await;
        if let Some(response) = state.next_response() {
            debug!("Mock serial read: {}", response);
            Ok(Some(response))
        } else {
            // No response queued - this shouldn't happen in normal operation
            debug!("Mock serial read: no response queued");
            Ok(None)
        }
    }
}

/// Mock serial writer that processes commands and queues responses
pub struct MockSerialWriter {
    state: Arc<Mutex<MockState>>,
}

impl MockSerialWriter {
    /// Create a new mock writer with shared state
    fn new(state: Arc<Mutex<MockState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl SerialWriter for MockSerialWriter {
    async fn write_message(&mut self, message: &str) -> Result<()> {
        debug!("Mock serial write: {}", message);
        let mut state = self.state.lock().await;
        state.process_command(message);
        Ok(())
    }
}

/// Mock serial port factory for testing
#[derive(Clone, Default)]
pub struct MockSerialPortFactory;

impl MockSerialPortFactory {
    /// Create a new mock factory
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SerialPortFactory for MockSerialPortFactory {
    async fn open(&self, port: &str, baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        debug!("Mock serial port opened: {} at {} baud", port, baud_rate);

        let state = Arc::new(Mutex::new(MockState::new()));

        Ok(SerialPair {
            reader: Box::new(MockSerialReader::new(Arc::clone(&state))),
            writer: Box::new(MockSerialWriter::new(state)),
        })
    }

    async fn port_exists(&self, _port: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_ping_response() {
        let state = Arc::new(Mutex::new(MockState::new()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message("P#").await.unwrap();
        let response = reader.read_line().await.unwrap();
        assert_eq!(response, Some("PPBA_OK".to_string()));
    }

    #[tokio::test]
    async fn test_mock_status_response() {
        let state = Arc::new(Mutex::new(MockState::new()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message("PA").await.unwrap();
        let response = reader.read_line().await.unwrap().unwrap();
        assert!(response.starts_with("PPBA:"));
    }

    #[tokio::test]
    async fn test_mock_power_stats_response() {
        let state = Arc::new(Mutex::new(MockState::new()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message("PS").await.unwrap();
        let response = reader.read_line().await.unwrap().unwrap();
        assert!(response.starts_with("PS:"));
    }

    #[tokio::test]
    async fn test_mock_set_quad_12v() {
        let state = Arc::new(Mutex::new(MockState::new()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message("P1:1").await.unwrap();
        let response = reader.read_line().await.unwrap();
        assert_eq!(response, Some("P1:1".to_string()));

        writer.write_message("P1:0").await.unwrap();
        let response = reader.read_line().await.unwrap();
        assert_eq!(response, Some("P1:0".to_string()));
    }

    #[tokio::test]
    async fn test_mock_set_dew_heater() {
        let state = Arc::new(Mutex::new(MockState::new()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message("P3:200").await.unwrap();
        let response = reader.read_line().await.unwrap();
        assert_eq!(response, Some("P3:200".to_string()));

        writer.write_message("P4:150").await.unwrap();
        let response = reader.read_line().await.unwrap();
        assert_eq!(response, Some("P4:150".to_string()));
    }

    #[tokio::test]
    async fn test_mock_state_persists() {
        let state = Arc::new(Mutex::new(MockState::new()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        // Turn off quad 12V
        writer.write_message("P1:0").await.unwrap();
        reader.read_line().await.unwrap();

        // Check status reflects the change
        writer.write_message("PA").await.unwrap();
        let response = reader.read_line().await.unwrap().unwrap();

        // Status should show quad_12v as 0 (off)
        // Format: PPBA:voltage:current:temp:humidity:dewpoint:quad:adj:dewA:dewB:autodew:warn:pwradj
        let parts: Vec<&str> = response.split(':').collect();
        assert_eq!(parts[6], "0"); // quad_12v should be 0
    }

    #[tokio::test]
    async fn test_mock_factory_creates_working_pair() {
        let factory = MockSerialPortFactory::new();
        let mut pair = factory
            .open("/dev/mock", 9600, Duration::from_secs(1))
            .await
            .unwrap();

        pair.writer.write_message("P#").await.unwrap();
        let response = pair.reader.read_line().await.unwrap();
        assert_eq!(response, Some("PPBA_OK".to_string()));
    }

    #[tokio::test]
    async fn test_mock_usb_hub() {
        let state = Arc::new(Mutex::new(MockState::new()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message("PU:1").await.unwrap();
        let response = reader.read_line().await.unwrap();
        assert_eq!(response, Some("PU:1".to_string()));

        writer.write_message("PU:0").await.unwrap();
        let response = reader.read_line().await.unwrap();
        assert_eq!(response, Some("PU:0".to_string()));
    }

    #[tokio::test]
    async fn test_mock_auto_dew() {
        let state = Arc::new(Mutex::new(MockState::new()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message("PD:0").await.unwrap();
        let response = reader.read_line().await.unwrap();
        assert_eq!(response, Some("PD:0".to_string()));

        writer.write_message("PD:1").await.unwrap();
        let response = reader.read_line().await.unwrap();
        assert_eq!(response, Some("PD:1".to_string()));
    }
}
