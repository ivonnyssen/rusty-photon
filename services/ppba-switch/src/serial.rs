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

use crate::error::{PpbaError, Result};
use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};

/// Serial reader using tokio-serial
pub struct TokioSerialReader {
    reader: BufReader<ReadHalf<SerialStream>>,
    buffer: String,
}

impl TokioSerialReader {
    /// Create a new serial reader from a read half of a serial stream
    pub fn new(reader: ReadHalf<SerialStream>) -> Self {
        Self {
            reader: BufReader::new(reader),
            buffer: String::new(),
        }
    }
}

#[async_trait]
impl SerialReader for TokioSerialReader {
    async fn read_line(&mut self) -> Result<Option<String>> {
        self.buffer.clear();
        match self.reader.read_line(&mut self.buffer).await {
            Ok(0) => Ok(None), // EOF
            Ok(_) => {
                let line = self.buffer.trim().to_string();
                debug!("Serial read: {}", line);
                Ok(Some(line))
            }
            Err(e) if e.kind() == ErrorKind::TimedOut => {
                Err(PpbaError::Timeout("Serial read timed out".to_string()))
            }
            Err(e) => Err(PpbaError::Io(e)),
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
            .write_all(format!("{}\n", message).as_bytes())
            .await
            .map_err(|e| PpbaError::Communication(format!("Failed to write: {}", e)))?;
        self.writer
            .flush()
            .await
            .map_err(|e| PpbaError::Communication(format!("Failed to flush: {}", e)))?;
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
            .map_err(|e| PpbaError::SerialPort(format!("Failed to open {}: {}", port, e)))?;

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
        let factory = TokioSerialPortFactory::default();
        let _ = factory;
    }

    #[tokio::test]
    async fn test_port_exists_nonexistent() {
        let factory = TokioSerialPortFactory::new();
        assert!(!factory.port_exists("/dev/nonexistent_port_12345").await);
    }
}
