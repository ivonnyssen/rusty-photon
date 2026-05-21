//! Tokio-serial-backed [`TransportFactory`] for the PPBA.
//!
//! Opens the configured serial port at the configured baud rate and wraps
//! the resulting stream in a [`SerialFrameTransport`] with `\n` as the
//! frame terminator (PPBA Gen2 replies are LF-terminated ASCII lines).

use std::io;
use std::time::Duration;

use async_trait::async_trait;
use rusty_photon_shared_transport::{
    FrameTransport, SerialFrameTransport, TransportError, TransportFactory,
};
use tokio_serial::SerialPortBuilderExt;
use tracing::debug;

/// Maximum size of a single PPBA frame.
///
/// The longest reply we ever see is the PA status line (~80 characters);
/// 256 bytes gives the device plenty of headroom and bounds a misbehaving
/// peer that streams without a terminator.
const MAX_FRAME_SIZE: usize = 256;

/// Real-hardware factory for the PPBA serial transport.
///
/// Captures the per-call configuration (port path, baud rate, timeout)
/// once at service-startup time so [`TransportFactory::open`] can be
/// retried by [`rusty_photon_shared_transport::SharedTransport`] without
/// the caller having to thread parameters through.
#[derive(Debug, Clone)]
pub struct PpbaTransportFactory {
    port: String,
    baud_rate: u32,
    timeout: Duration,
}

impl PpbaTransportFactory {
    pub fn new(port: impl Into<String>, baud_rate: u32, timeout: Duration) -> Self {
        Self {
            port: port.into(),
            baud_rate,
            timeout,
        }
    }
}

#[async_trait]
impl TransportFactory for PpbaTransportFactory {
    async fn open(&self) -> Result<Box<dyn FrameTransport>, TransportError> {
        debug!(
            port = %self.port,
            baud = self.baud_rate,
            timeout = ?self.timeout,
            "opening PPBA serial transport"
        );

        // Note: no `.timeout(self.timeout)` on the tokio-serial builder.
        // That would set the port-level (termios `VTIME`) timeout, which
        // surfaces as `io::ErrorKind::TimedOut` and is propagated by
        // `SerialFrameTransport` as `TransportError::Io(TimedOut)` rather
        // than `TransportError::Timeout(d)`. We rely on
        // `with_read_timeout` / `with_write_timeout` instead so the
        // single classified timeout fires and yields the right variant.
        let stream = tokio_serial::new(&self.port, self.baud_rate)
            .open_native_async()
            .map_err(|e| TransportError::Open(io::Error::other(e.to_string())))?;

        let transport = SerialFrameTransport::new(stream, b'\n', MAX_FRAME_SIZE)
            .with_read_timeout(self.timeout)
            .with_write_timeout(self.timeout);
        Ok(Box::new(transport))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn factory_open_nonexistent_port_returns_open_error() {
        let factory =
            PpbaTransportFactory::new("/dev/nonexistent_port_12345", 9600, Duration::from_secs(1));
        match factory.open().await {
            Err(TransportError::Open(_)) => {}
            Err(other) => panic!("expected TransportError::Open, got {other:?}"),
            Ok(_) => panic!("expected error opening nonexistent port"),
        }
    }
}
