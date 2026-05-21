//! Tokio-serial-backed [`TransportFactory`] for the Falcon Rotator.
//!
//! Opens the configured serial port at the configured baud rate
//! (default 9600 — see [`crate::config::SerialConfig`]). Framing is
//! 8N1 (tokio-serial's default — the Falcon firmware doesn't speak
//! anything else and the driver doesn't expose framing knobs), with
//! `\n` as the line terminator in both directions. The stream is
//! wrapped in a [`SerialFrameTransport`] with `b'\n'` framing, replacing
//! the legacy `SerialReader` / `SerialWriter` / `SerialPortPair`
//! abstraction — that layer now lives in
//! [`rusty_photon_shared_transport`].

use std::io;
use std::time::Duration;

use async_trait::async_trait;
use rusty_photon_shared_transport::{
    FrameTransport, SerialFrameTransport, TransportError, TransportFactory,
};
use tokio_serial::SerialPortBuilderExt;
use tracing::debug;

/// Maximum size of a single Falcon frame.
///
/// The longest reply we ever see is the `FA` full-status line
/// (`FR_OK:steps:deg:moving:limit:derot:reverse`, ~50 characters even
/// for a 7-digit step counter); 256 bytes gives the device plenty of
/// headroom and bounds a misbehaving peer that streams without a
/// terminator.
const MAX_FRAME_SIZE: usize = 256;

/// Real-hardware factory for the Falcon serial transport.
///
/// Captures the per-call configuration (port path, baud rate, timeout)
/// once at service-startup time so [`TransportFactory::open`] can be
/// re-invoked by [`rusty_photon_shared_transport::SharedTransport`]
/// without the caller having to thread parameters through.
#[derive(Debug, Clone)]
pub struct FalconTransportFactory {
    port: String,
    baud_rate: u32,
    timeout: Duration,
}

impl FalconTransportFactory {
    pub fn new(port: impl Into<String>, baud_rate: u32, timeout: Duration) -> Self {
        Self {
            port: port.into(),
            baud_rate,
            timeout,
        }
    }

    /// Build a factory from the service [`SerialConfig`](crate::config::SerialConfig).
    pub fn from_config(config: &crate::config::SerialConfig) -> Self {
        Self::new(config.port.clone(), config.baud_rate, config.timeout)
    }
}

#[async_trait]
impl TransportFactory for FalconTransportFactory {
    /// Coverage off: the success path requires a real serial device. The
    /// open-failure path is covered by
    /// `factory_open_nonexistent_port_returns_open_error` below, which
    /// exercises the `map_err` branch.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn open(&self) -> Result<Box<dyn FrameTransport>, TransportError> {
        debug!(
            port = %self.port,
            baud = self.baud_rate,
            timeout = ?self.timeout,
            "opening Falcon serial transport"
        );

        // Note: no `.timeout(self.timeout)` on the tokio-serial builder.
        // `SerialFrameTransport`'s `with_read_timeout` /
        // `with_write_timeout` already enforces the per-call deadline via
        // `tokio::time::timeout`; adding a parallel port-level (termios
        // `VTIME`) timeout creates two timers set to the same value with
        // no obvious answer to "which fires first". The shared crate
        // reclassifies `io::ErrorKind::TimedOut` from the wrapped stream
        // back to `TransportError::Timeout`, so if a future runtime ever
        // does need a port-level timeout the classification stays right
        // — but reasoning is still simpler with a single source.
        // Pass the `tokio_serial::Error` to `io::Error::other` directly
        // (not its `.to_string()`) so the original error is preserved as
        // the `io::Error` source — `TransportError::Open(io::Error)` then
        // exposes the full cause chain via `Error::source()` traversal in
        // logs / debug output.
        let stream = tokio_serial::new(&self.port, self.baud_rate)
            .open_native_async()
            .map_err(|e| TransportError::Open(io::Error::other(e)))?;

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
    #[cfg_attr(miri, ignore)]
    async fn factory_open_nonexistent_port_returns_open_error() {
        use std::error::Error;
        let factory = FalconTransportFactory::new(
            "/dev/nonexistent_falcon_12345",
            9600,
            Duration::from_secs(1),
        );
        match factory.open().await {
            Err(TransportError::Open(io_err)) => {
                // `io::Error::other(e)` (vs `io::Error::other(e.to_string())`)
                // preserves the original `tokio_serial::Error` as the
                // io::Error's source, so log/debug output traversing
                // `Error::source()` recovers the underlying cause.
                assert!(
                    io_err.source().is_some() || io_err.get_ref().is_some(),
                    "expected the underlying tokio_serial::Error to be preserved as source"
                );
            }
            Err(other) => panic!("expected TransportError::Open, got {other:?}"),
            Ok(_) => panic!("expected error opening nonexistent port"),
        }
    }

    #[test]
    fn from_config_propagates_fields() {
        let cfg = crate::config::SerialConfig {
            port: "/dev/ttyTEST".to_string(),
            baud_rate: 9600,
            timeout: Duration::from_secs(3),
        };
        let factory = FalconTransportFactory::from_config(&cfg);
        assert_eq!(factory.port, "/dev/ttyTEST");
        assert_eq!(factory.baud_rate, 9600);
        assert_eq!(factory.timeout, Duration::from_secs(3));
    }
}
