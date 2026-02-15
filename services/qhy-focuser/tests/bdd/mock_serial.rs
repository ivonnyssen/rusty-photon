//! Shared mock serial infrastructure for BDD tests

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use qhy_focuser::error::QhyFocuserError;
use qhy_focuser::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use qhy_focuser::Result;
use tokio::sync::Mutex;

pub struct MockSerialReader {
    responses: Arc<Mutex<Vec<String>>>,
    index: Arc<Mutex<usize>>,
}

impl MockSerialReader {
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

pub struct MockSerialWriter {
    sent_messages: Arc<Mutex<Vec<String>>>,
}

impl MockSerialWriter {
    pub fn new() -> Self {
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

pub struct MockSerialPortFactory {
    responses: Vec<String>,
}

impl MockSerialPortFactory {
    pub fn new(responses: Vec<String>) -> Self {
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

pub struct FailingFactory {
    error_msg: String,
}

impl FailingFactory {
    pub fn new(error_msg: &str) -> Self {
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

/// Standard handshake responses: version + set_speed + position + temperature,
/// followed by extra polling response pairs.
pub fn standard_connection_responses() -> Vec<String> {
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
