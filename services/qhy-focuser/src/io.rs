//! I/O traits for serial communication
//!
//! This module provides trait abstractions for serial port operations.
//! These traits enable mockall-based testing without requiring actual hardware.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;

/// Pair of reader and writer for a serial connection
pub struct SerialPair {
    /// Reader for receiving data
    pub reader: Box<dyn SerialReader>,
    /// Writer for sending data
    pub writer: Box<dyn SerialWriter>,
}

/// Trait for reading lines from a serial port
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait SerialReader: Send {
    /// Read a line from the serial port
    ///
    /// Returns `Ok(Some(line))` if a line was read successfully,
    /// `Ok(None)` if the port was closed,
    /// or an error if reading failed.
    async fn read_line(&mut self) -> Result<Option<String>>;
}

/// Trait for writing data to a serial port
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait SerialWriter: Send {
    /// Write a message to the serial port
    ///
    /// The message is written with a newline terminator.
    async fn write_message(&mut self, message: &str) -> Result<()>;
}

/// Trait for creating serial port connections
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait SerialPortFactory: Send + Sync {
    /// Open a serial port connection
    ///
    /// Returns a pair of reader and writer on success.
    async fn open(&self, port: &str, baud_rate: u32, timeout: Duration) -> Result<SerialPair>;

    /// Check if a serial port exists and can be opened
    async fn port_exists(&self, port: &str) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_pair_creation() {
        let reader = MockSerialReader::new();
        let writer = MockSerialWriter::new();
        let _pair = SerialPair {
            reader: Box::new(reader),
            writer: Box::new(writer),
        };
    }
}
