//! Serial port implementation using tokio-serial
//!
//! This module provides concrete implementations of the I/O traits
//! using tokio-serial for actual hardware communication.

use std::io::ErrorKind;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::debug;

use crate::error::{QhyFocuserError, Result};
use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};

/// Serial reader using tokio-serial
pub struct TokioSerialReader {
    reader: BufReader<ReadHalf<SerialStream>>,
    buffer: Vec<u8>,
}

impl TokioSerialReader {
    /// Create a new serial reader from a read half of a serial stream
    pub fn new(reader: ReadHalf<SerialStream>) -> Self {
        Self {
            reader: BufReader::new(reader),
            buffer: Vec::new(),
        }
    }
}

#[async_trait]
impl SerialReader for TokioSerialReader {
    async fn read_line(&mut self) -> Result<Option<String>> {
        self.buffer.clear();
        match self.reader.read_until(b'}', &mut self.buffer).await {
            Ok(0) => Ok(None),
            Ok(_) => {
                let raw = String::from_utf8_lossy(&self.buffer);
                // Trim any leading junk before the opening `{`
                let trimmed = match raw.find('{') {
                    Some(start) => &raw[start..],
                    None => raw.trim(),
                };
                let line = trimmed.to_string();
                debug!("Serial read: {}", line);
                Ok(Some(line))
            }
            Err(e) if e.kind() == ErrorKind::TimedOut => Err(QhyFocuserError::Timeout(
                "Serial read timed out".to_string(),
            )),
            Err(e) => Err(QhyFocuserError::Io(e)),
        }
    }
}

/// Serial writer using tokio-serial
pub struct TokioSerialWriter {
    writer: WriteHalf<SerialStream>,
}

impl TokioSerialWriter {
    /// Create a new serial writer from a write half of a serial stream
    pub fn new(writer: WriteHalf<SerialStream>) -> Self {
        Self { writer }
    }
}

#[async_trait]
impl SerialWriter for TokioSerialWriter {
    async fn write_message(&mut self, message: &str) -> Result<()> {
        debug!("Serial write: {}", message);
        self.writer
            .write_all(message.as_bytes())
            .await
            .map_err(|e| QhyFocuserError::Communication(format!("Failed to write: {}", e)))?;
        self.writer
            .flush()
            .await
            .map_err(|e| QhyFocuserError::Communication(format!("Failed to flush: {}", e)))?;
        Ok(())
    }
}

/// Serial port factory using tokio-serial
#[derive(Default, Clone)]
pub struct TokioSerialPortFactory;

impl TokioSerialPortFactory {
    /// Create a new serial port factory
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SerialPortFactory for TokioSerialPortFactory {
    async fn open(&self, port: &str, baud_rate: u32, timeout: Duration) -> Result<SerialPair> {
        debug!(
            "Opening serial port {} at {} baud with {:?} timeout",
            port, baud_rate, timeout
        );

        let stream = tokio_serial::new(port, baud_rate)
            .timeout(timeout)
            .open_native_async()
            .map_err(|e| QhyFocuserError::SerialPort(format!("Failed to open {}: {}", port, e)))?;

        debug!("Serial port {} opened successfully", port);

        let (reader, writer) = tokio::io::split(stream);

        Ok(SerialPair {
            reader: Box::new(TokioSerialReader::new(reader)),
            writer: Box::new(TokioSerialWriter::new(writer)),
        })
    }

    async fn port_exists(&self, port: &str) -> bool {
        std::path::Path::new(port).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_port_factory_new() {
        let factory = TokioSerialPortFactory::new();
        let _ = factory;
    }

    #[test]
    fn test_serial_port_factory_default() {
        let factory = TokioSerialPortFactory;
        let _ = factory;
    }

    #[tokio::test]
    async fn test_port_exists_nonexistent() {
        let factory = TokioSerialPortFactory::new();
        assert!(!factory.port_exists("/dev/nonexistent_port_12345").await);
    }

    #[tokio::test]
    async fn test_port_exists_existing_path() {
        let factory = TokioSerialPortFactory::new();
        // Use a path that exists on all platforms
        let path = std::env::current_exe().unwrap();
        assert!(factory.port_exists(path.to_str().unwrap()).await);
    }

    #[test]
    fn test_serial_port_factory_clone() {
        let factory = TokioSerialPortFactory::new();
        let _cloned = factory.clone();
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // tokio-serial uses unsupported syscall flags under Miri
    async fn test_open_nonexistent_port() {
        let factory = TokioSerialPortFactory::new();
        let result = factory
            .open("/dev/nonexistent_port_12345", 9600, Duration::from_secs(1))
            .await;
        match result {
            Err(QhyFocuserError::SerialPort(msg)) => {
                assert!(msg.contains("/dev/nonexistent_port_12345"), "got: {}", msg);
            }
            Err(other) => panic!("Expected SerialPort error, got {:?}", other),
            Ok(_) => panic!("Expected error opening nonexistent port"),
        }
    }
}
