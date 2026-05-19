//! Serial port implementation using tokio-serial
//!
//! Concrete `SerialPortFactory` driving real hardware over `tokio-serial`.
//! Falcon framing is 9600-8N1 ASCII with LF terminators in both directions, so
//! the reader uses `BufReader::read_line` and the writer appends `\n` on
//! behalf of the caller (matching the [`SerialWriter`](crate::io::SerialWriter)
//! contract).

use std::io::ErrorKind;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::debug;

use crate::error::{FalconRotatorError, Result};
use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};

/// Serial reader backed by `tokio-serial`, framing on LF.
///
/// The reader's body is excluded from coverage: every branch hits real
/// `tokio-serial` I/O and is only exercised against live Falcon hardware
/// (the BDD / unit suites drive [`MockSerialPortFactory`] instead). The
/// peer drivers `qhy-focuser` and `ppba-driver` apply the same exclusion.
pub struct TokioSerialReader {
    reader: BufReader<ReadHalf<SerialStream>>,
    buffer: String,
}

impl TokioSerialReader {
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn new(reader: ReadHalf<SerialStream>) -> Self {
        Self {
            reader: BufReader::new(reader),
            buffer: String::new(),
        }
    }
}

#[async_trait]
impl SerialReader for TokioSerialReader {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn read_line(&mut self) -> Result<Option<String>> {
        self.buffer.clear();
        match self.reader.read_line(&mut self.buffer).await {
            Ok(0) => Ok(None),
            Ok(_) => {
                let line = self.buffer.trim_end().to_string();
                debug!("Serial read: {}", line);
                Ok(Some(line))
            }
            Err(e) if e.kind() == ErrorKind::TimedOut => {
                Err(FalconRotatorError::Timeout("Serial read timed out".into()))
            }
            Err(e) => Err(FalconRotatorError::Io(e)),
        }
    }
}

/// Serial writer backed by `tokio-serial`. Appends the LF terminator.
///
/// Same coverage rationale as [`TokioSerialReader`] — real-hardware-only path.
pub struct TokioSerialWriter {
    writer: WriteHalf<SerialStream>,
}

impl TokioSerialWriter {
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn new(writer: WriteHalf<SerialStream>) -> Self {
        Self { writer }
    }
}

#[async_trait]
impl SerialWriter for TokioSerialWriter {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn write_message(&mut self, message: &str) -> Result<()> {
        debug!("Serial write: {}", message);
        self.writer
            .write_all(message.as_bytes())
            .await
            .map_err(|e| FalconRotatorError::Communication(format!("Failed to write: {e}")))?;
        self.writer
            .write_all(b"\n")
            .await
            .map_err(|e| FalconRotatorError::Communication(format!("Failed to write LF: {e}")))?;
        self.writer
            .flush()
            .await
            .map_err(|e| FalconRotatorError::Communication(format!("Failed to flush: {e}")))?;
        Ok(())
    }
}

/// Serial port factory backed by `tokio-serial`.
#[derive(Default, Clone)]
pub struct TokioSerialPortFactory;

impl TokioSerialPortFactory {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SerialPortFactory for TokioSerialPortFactory {
    /// Coverage off: the success path requires a real serial device. The
    /// open-failure path is covered by `test_open_nonexistent_port_returns_serial_port_error`
    /// below, which exercises the `map_err` branch.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn open(&self, port: &str, baud_rate: u32, timeout: Duration) -> Result<SerialPair> {
        debug!(
            "Opening serial port {} at {} baud with {:?} timeout",
            port, baud_rate, timeout
        );

        let stream = tokio_serial::new(port, baud_rate)
            .timeout(timeout)
            .open_native_async()
            .map_err(|e| FalconRotatorError::SerialPort(format!("Failed to open {port}: {e}")))?;

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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_factory_constructors() {
        let _new = TokioSerialPortFactory::new();
        let _default = TokioSerialPortFactory;
        let _cloned = TokioSerialPortFactory.clone();
    }

    #[tokio::test]
    async fn test_port_exists_nonexistent() {
        let factory = TokioSerialPortFactory::new();
        assert!(!factory.port_exists("/dev/nonexistent_falcon_12345").await);
    }

    #[tokio::test]
    async fn test_port_exists_known_path() {
        let factory = TokioSerialPortFactory::new();
        let path = std::env::current_exe().unwrap();
        assert!(factory.port_exists(path.to_str().unwrap()).await);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_open_nonexistent_port_returns_serial_port_error() {
        let factory = TokioSerialPortFactory::new();
        let result = factory
            .open(
                "/dev/nonexistent_falcon_12345",
                9600,
                Duration::from_secs(1),
            )
            .await;
        match result {
            Err(FalconRotatorError::SerialPort(msg)) => {
                assert!(
                    msg.contains("/dev/nonexistent_falcon_12345"),
                    "expected port name in error message, got: {msg}"
                );
            }
            Err(other) => panic!("expected SerialPort error, got {other:?}"),
            Ok(_) => panic!("expected error opening nonexistent port"),
        }
    }
}
