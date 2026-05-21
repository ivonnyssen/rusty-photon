//! [`TransportFactory`] for the FP2's USB-CDC serial port.
//!
//! Wraps a fresh `tokio_serial::SerialStream` in a
//! [`SerialFrameTransport`] configured to read until the FP2's `)`
//! response terminator. The factory captures port path / baud rate /
//! timeout at startup; each `open()` call corresponds to one
//! `SharedTransport` 0→1 connect transition.

use std::time::Duration;

use async_trait::async_trait;
use rusty_photon_shared_transport::{
    FrameTransport, SerialFrameTransport, TransportError, TransportFactory,
};
use tokio_serial::SerialPortBuilderExt;
use tracing::debug;

/// Upper bound on a single FP2 response frame. The firmware identification
/// string is the longest real response and sits well under 64 bytes; a 256
/// cap is generous and still ensures runaway peers can't grow the read
/// buffer unboundedly (the cap is enforced by `SerialFrameTransport`).
const MAX_FRAME_SIZE: usize = 256;

/// Factory that opens the FP2 over `tokio-serial`.
#[derive(Debug, Clone)]
pub struct Fp2SerialTransportFactory {
    port: String,
    baud_rate: u32,
    timeout: Duration,
}

impl Fp2SerialTransportFactory {
    pub fn new(port: impl Into<String>, baud_rate: u32, timeout: Duration) -> Self {
        Self {
            port: port.into(),
            baud_rate,
            timeout,
        }
    }
}

#[async_trait]
impl TransportFactory for Fp2SerialTransportFactory {
    async fn open(&self) -> Result<Box<dyn FrameTransport>, TransportError> {
        debug!(
            "Opening FP2 serial port {} at {} baud (timeout {:?})",
            self.port, self.baud_rate, self.timeout
        );

        let stream = tokio_serial::new(&self.port, self.baud_rate)
            .timeout(self.timeout)
            .open_native_async()
            .map_err(|e| {
                TransportError::Open(std::io::Error::other(format!(
                    "failed to open {}: {e}",
                    self.port
                )))
            })?;

        debug!("FP2 serial port {} opened", self.port);

        let transport = SerialFrameTransport::new(stream, b')', MAX_FRAME_SIZE)
            .with_read_timeout(self.timeout)
            .with_write_timeout(self.timeout);

        Ok(Box::new(transport))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn factory_clone_preserves_config() {
        let factory =
            Fp2SerialTransportFactory::new("/dev/ttyACM0", 115_200, Duration::from_secs(3));
        let cloned = factory.clone();
        assert_eq!(cloned.port, "/dev/ttyACM0");
        assert_eq!(cloned.baud_rate, 115_200);
        assert_eq!(cloned.timeout, Duration::from_secs(3));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // tokio-serial uses syscalls Miri doesn't model
    async fn open_returns_transport_open_error_for_missing_device() {
        let factory = Fp2SerialTransportFactory::new(
            "/dev/nonexistent_dsd_fp2_99999",
            115_200,
            Duration::from_millis(100),
        );
        match factory.open().await {
            Err(TransportError::Open(io)) => {
                assert!(io.to_string().contains("/dev/nonexistent_dsd_fp2_99999"));
            }
            Err(other) => panic!("expected Open error, got {other:?}"),
            Ok(_) => panic!("expected open to fail for nonexistent device"),
        }
    }
}
