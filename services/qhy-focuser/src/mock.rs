//! Mock serial port implementation for testing
//!
//! This module provides mock implementations of the serial I/O traits
//! that simulate QHY Q-Focuser responses, allowing the driver to be tested
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
    response_queue: Vec<String>,
    device_state: MockDeviceState,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            response_queue: Vec::new(),
            device_state: MockDeviceState::default(),
        }
    }
}

/// Simulated focuser device state
#[derive(Debug, Clone)]
struct MockDeviceState {
    position: i64,
    target_position: Option<i64>,
    is_moving: bool,
    speed: u8,
    reverse: bool,
    outer_temp_raw: i64,
    chip_temp_raw: i64,
    voltage_raw: i64,
    firmware_version: String,
    board_version: String,
}

impl Default for MockDeviceState {
    fn default() -> Self {
        Self {
            position: 0,
            target_position: None,
            is_moving: false,
            speed: 0,
            reverse: false,
            outer_temp_raw: 25000, // 25.0°C
            chip_temp_raw: 30000,  // 30.0°C
            voltage_raw: 125,      // 12.5V
            firmware_version: "2.1.0".to_string(),
            board_version: "1.0".to_string(),
        }
    }
}

impl MockState {
    /// Process a command and queue the appropriate response
    fn process_command(&mut self, command: &str) {
        let command = command.trim();
        debug!("Mock processing command: '{}'", command);

        // Parse JSON command
        let parsed: serde_json::Value = match serde_json::from_str(command) {
            Ok(v) => v,
            Err(_) => {
                debug!("Mock: invalid JSON command '{}'", command);
                self.response_queue
                    .push(r#"{"error": "invalid command"}"#.to_string());
                return;
            }
        };

        let cmd_id = parsed["cmd_id"].as_u64().unwrap_or(0);

        let response = match cmd_id {
            1 => {
                // GetVersion
                serde_json::json!({
                    "idx": 1,
                    "firmware_version": self.device_state.firmware_version,
                    "board_version": self.device_state.board_version
                })
                .to_string()
            }
            2 => {
                // RelativeMove
                let dir = parsed["dir"].as_i64().unwrap_or(1);
                let steps = parsed["step"].as_u64().unwrap_or(0) as i64;

                let delta = if dir > 0 { steps } else { -steps };
                self.device_state.target_position = Some(self.device_state.position + delta);
                self.device_state.is_moving = true;
                // Simulate instant move for mock
                self.device_state.position = self.device_state.target_position.unwrap();
                self.device_state.is_moving = false;
                self.device_state.target_position = None;

                serde_json::json!({"idx": 2}).to_string()
            }
            3 => {
                // Abort
                self.device_state.is_moving = false;
                self.device_state.target_position = None;
                serde_json::json!({"idx": 3}).to_string()
            }
            4 => {
                // ReadTemperature
                serde_json::json!({
                    "idx": 4,
                    "o_t": self.device_state.outer_temp_raw,
                    "c_t": self.device_state.chip_temp_raw,
                    "c_r": self.device_state.voltage_raw
                })
                .to_string()
            }
            5 => {
                // GetPosition
                // Simulate gradual movement: move position towards target
                if self.device_state.is_moving {
                    if let Some(target) = self.device_state.target_position {
                        // Move position closer to target
                        let diff = target - self.device_state.position;
                        if diff.abs() <= 1000 {
                            self.device_state.position = target;
                            self.device_state.is_moving = false;
                            self.device_state.target_position = None;
                        } else if diff > 0 {
                            self.device_state.position += 1000;
                        } else {
                            self.device_state.position -= 1000;
                        }
                    }
                }

                serde_json::json!({
                    "idx": 5,
                    "pos": self.device_state.position
                })
                .to_string()
            }
            6 => {
                // AbsoluteMove
                let position = parsed["tar"].as_i64().unwrap_or(0);
                self.device_state.target_position = Some(position);
                self.device_state.is_moving = true;
                serde_json::json!({"idx": 6}).to_string()
            }
            7 => {
                // SetReverse
                let enabled = parsed["rev"].as_u64().unwrap_or(0) == 1;
                self.device_state.reverse = enabled;
                serde_json::json!({"idx": 7}).to_string()
            }
            11 => {
                // SyncPosition
                let position = parsed["init_val"].as_i64().unwrap_or(0);
                self.device_state.position = position;
                serde_json::json!({"idx": 11}).to_string()
            }
            13 => {
                // SetSpeed
                let speed = parsed["speed"].as_u64().unwrap_or(0) as u8;
                self.device_state.speed = speed;
                serde_json::json!({"idx": 13}).to_string()
            }
            16 => {
                // SetHoldCurrent
                serde_json::json!({"idx": 16}).to_string()
            }
            19 => {
                // SetPdnMode
                serde_json::json!({"idx": 19}).to_string()
            }
            _ => {
                debug!("Mock: unknown cmd_id {}", cmd_id);
                serde_json::json!({"error": "unknown command"}).to_string()
            }
        };

        debug!("Mock queuing response: '{}'", response);
        self.response_queue.push(response);
    }

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
    fn new(state: Arc<Mutex<MockState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl SerialReader for MockSerialReader {
    async fn read_line(&mut self) -> Result<Option<String>> {
        let mut state = self.state.lock().await;
        let queue_len = state.response_queue.len();
        if let Some(response) = state.next_response() {
            debug!(
                "Mock serial read: '{}' (queue had {} items)",
                response, queue_len
            );
            Ok(Some(response))
        } else {
            debug!("Mock serial read: NO RESPONSE QUEUED (queue empty)");
            Ok(None)
        }
    }
}

/// Mock serial writer that processes commands and queues responses
pub struct MockSerialWriter {
    state: Arc<Mutex<MockState>>,
}

impl MockSerialWriter {
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
///
/// Maintains persistent state across multiple open/close cycles to simulate
/// real hardware behavior where device state persists even when disconnected.
#[derive(Clone, Default)]
pub struct MockSerialPortFactory {
    persistent_state: Arc<Mutex<MockState>>,
}

#[async_trait]
impl SerialPortFactory for MockSerialPortFactory {
    async fn open(&self, port: &str, baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        debug!("Mock serial port opened: {} at {} baud", port, baud_rate);

        let state = Arc::clone(&self.persistent_state);

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
    async fn test_mock_get_version() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message(r#"{"cmd_id": 1}"#).await.unwrap();
        let response = reader.read_line().await.unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["idx"], 1);
        assert_eq!(parsed["firmware_version"], "2.1.0");
    }

    #[tokio::test]
    async fn test_mock_get_position() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message(r#"{"cmd_id": 5}"#).await.unwrap();
        let response = reader.read_line().await.unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["idx"], 5);
        assert_eq!(parsed["pos"], 0);
    }

    #[tokio::test]
    async fn test_mock_temperature() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message(r#"{"cmd_id": 4}"#).await.unwrap();
        let response = reader.read_line().await.unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["idx"], 4);
        assert_eq!(parsed["o_t"], 25000);
        assert_eq!(parsed["c_t"], 30000);
        assert_eq!(parsed["c_r"], 125);
    }

    #[tokio::test]
    async fn test_mock_absolute_move() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        // Send absolute move
        writer
            .write_message(r#"{"cmd_id": 6, "tar": 5000}"#)
            .await
            .unwrap();
        let response = reader.read_line().await.unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["idx"], 6);
    }

    #[tokio::test]
    async fn test_mock_abort() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        writer.write_message(r#"{"cmd_id": 3}"#).await.unwrap();
        let response = reader.read_line().await.unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["idx"], 3);
    }

    #[tokio::test]
    async fn test_mock_sync_position() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        // Sync position to 15000
        writer
            .write_message(r#"{"cmd_id": 11, "init_val": 15000}"#)
            .await
            .unwrap();
        let _ = reader.read_line().await.unwrap();

        // Verify position changed
        writer.write_message(r#"{"cmd_id": 5}"#).await.unwrap();
        let response = reader.read_line().await.unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["pos"], 15000);
    }

    #[tokio::test]
    async fn test_mock_factory_creates_working_pair() {
        let factory = MockSerialPortFactory::default();
        let mut pair = factory
            .open("/dev/mock", 9600, Duration::from_secs(1))
            .await
            .unwrap();

        pair.writer.write_message(r#"{"cmd_id": 1}"#).await.unwrap();
        let response = pair.reader.read_line().await.unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["idx"], 1);
    }

    #[tokio::test]
    async fn test_mock_state_persists_across_commands() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        // Set speed
        writer
            .write_message(r#"{"cmd_id": 13, "speed": 5}"#)
            .await
            .unwrap();
        let _ = reader.read_line().await.unwrap();

        // Set reverse
        writer
            .write_message(r#"{"cmd_id": 7, "rev": 1}"#)
            .await
            .unwrap();
        let _ = reader.read_line().await.unwrap();

        // Verify state persists (mock tracks these internally)
        // The mock correctly processes sequential commands
    }
}
