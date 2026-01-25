//! Mock serial port implementation for testing
//!
//! This module provides mock implementations of the serial I/O traits
//! that return predefined responses, allowing the driver to be tested
//! without real hardware.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::debug;

use crate::error::Result;
use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};

/// Mock serial reader that returns predefined responses
pub struct MockSerialReader {
    responses: Arc<Mutex<Vec<String>>>,
    index: Arc<Mutex<usize>>,
}

impl MockSerialReader {
    /// Create a new mock reader with predefined responses
    pub fn new(responses: Vec<String>) -> Self {
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
            debug!("Mock serial read: {}", response);
            Ok(Some(response))
        } else {
            // Cycle back to provide continuous responses for polling
            *index = 0;
            if !responses.is_empty() {
                let response = responses[0].clone();
                *index = 1;
                debug!("Mock serial read (cycled): {}", response);
                Ok(Some(response))
            } else {
                Ok(None)
            }
        }
    }
}

/// Mock serial writer that logs sent messages
pub struct MockSerialWriter;

impl MockSerialWriter {
    /// Create a new mock writer
    pub fn new() -> Self {
        Self
    }
}

impl Default for MockSerialWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SerialWriter for MockSerialWriter {
    async fn write_message(&mut self, message: &str) -> Result<()> {
        debug!("Mock serial write: {}", message);
        Ok(())
    }
}

/// Mock serial port factory for testing
#[derive(Clone)]
pub struct MockSerialPortFactory {
    responses: Vec<String>,
}

impl MockSerialPortFactory {
    /// Create a new mock factory with predefined responses
    pub fn new(responses: Vec<String>) -> Self {
        Self { responses }
    }

    /// Create a factory with standard PPBA responses for basic operation
    pub fn with_default_responses() -> Self {
        Self::new(vec![
            // Initial connection sequence
            "PPBA_OK".to_string(),                                     // Ping
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // Status
            "PS:2.5:10.5:126.0:3600000".to_string(),                   // Power stats
            // Responses for polling and set operations (cycling)
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // Status
            "PS:2.5:10.5:126.0:3600000".to_string(),                   // Power stats
            // Set command responses
            "P1:1".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(),
            "P1:0".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:1:0:0".to_string(),
            "P2:1".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:1:128:64:1:0:0".to_string(),
            "P2:0".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:1:0:0".to_string(),
            "P3:128".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:1:0:0".to_string(),
            "P4:64".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:1:0:0".to_string(),
            "PU:1".to_string(),
            "PU:0".to_string(),
            "PD:1".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:1:0:0".to_string(),
            "PD:0".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:0:0:0".to_string(),
        ])
    }
}

impl Default for MockSerialPortFactory {
    fn default() -> Self {
        Self::with_default_responses()
    }
}

#[async_trait]
impl SerialPortFactory for MockSerialPortFactory {
    async fn open(&self, port: &str, baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        debug!("Mock serial port opened: {} at {} baud", port, baud_rate);
        Ok(SerialPair {
            reader: Box::new(MockSerialReader::new(self.responses.clone())),
            writer: Box::new(MockSerialWriter::new()),
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
    async fn test_mock_reader_returns_responses_in_order() {
        let mut reader = MockSerialReader::new(vec!["first".to_string(), "second".to_string()]);

        assert_eq!(reader.read_line().await.unwrap(), Some("first".to_string()));
        assert_eq!(
            reader.read_line().await.unwrap(),
            Some("second".to_string())
        );
    }

    #[tokio::test]
    async fn test_mock_reader_cycles_after_exhausted() {
        let mut reader = MockSerialReader::new(vec!["only".to_string()]);

        assert_eq!(reader.read_line().await.unwrap(), Some("only".to_string()));
        // Should cycle back
        assert_eq!(reader.read_line().await.unwrap(), Some("only".to_string()));
    }

    #[tokio::test]
    async fn test_mock_writer_accepts_any_message() {
        let mut writer = MockSerialWriter::new();
        writer.write_message("test").await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_factory_creates_pair() {
        let factory = MockSerialPortFactory::default();
        let mut pair = factory
            .open("/dev/mock", 9600, Duration::from_secs(1))
            .await
            .unwrap();
        // Verify the pair works by reading a response
        let response = pair.reader.read_line().await.unwrap();
        assert!(response.is_some());
    }

    #[tokio::test]
    async fn test_mock_factory_port_always_exists() {
        let factory = MockSerialPortFactory::default();
        assert!(factory.port_exists("/dev/nonexistent").await);
    }
}
