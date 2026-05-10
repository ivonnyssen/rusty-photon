//! USB-CDC serial transport (tokio-serial).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::debug;

use crate::config::{Config, TransportConfig, UsbConfig};
use crate::error::{Result, StarAdvError};
use crate::transport::{Transport, TransportFactory};

/// Holds the open serial port and serialises one round-trip at a time.
pub struct SerialTransport {
    /// `tokio-serial` stream wrapped in a [`Mutex`] so concurrent
    /// `round_trip` calls (the polling task vs ad-hoc sends) don't
    /// interleave their bytes on the wire. The
    /// [`crate::TransportManager`] also takes a command lock above this
    /// — the mutex here is a defence-in-depth guard.
    stream: Mutex<SerialStream>,
}

impl SerialTransport {
    /// Open the configured port and return a transport ready to round-trip.
    pub async fn connect(config: UsbConfig) -> Result<Self> {
        debug!(
            port = %config.port,
            baud_rate = config.baud_rate,
            "opening serial port"
        );
        let stream = tokio_serial::new(&config.port, config.baud_rate)
            .timeout(config.command_timeout)
            .open_native_async()
            .map_err(|e| {
                StarAdvError::ConnectionFailed(format!("failed to open {}: {e}", config.port))
            })?;
        Ok(Self {
            stream: Mutex::new(stream),
        })
    }
}

#[async_trait]
impl Transport for SerialTransport {
    async fn round_trip(&self, request: &[u8], deadline: Duration) -> Result<Vec<u8>> {
        let mut stream = self.stream.lock().await;
        // Write the request frame.
        timeout(deadline, stream.write_all(request))
            .await
            .map_err(|_| StarAdvError::Timeout("serial write".to_string()))?
            .map_err(|e| StarAdvError::Transport(format!("serial write: {e}")))?;
        timeout(deadline, stream.flush())
            .await
            .map_err(|_| StarAdvError::Timeout("serial flush".to_string()))?
            .map_err(|e| StarAdvError::Transport(format!("serial flush: {e}")))?;

        // Read until `\r`. Replies are at most 8 bytes (`=XXXXXX\r`), so
        // a small buffer + byte-by-byte read is fine. The mount sometimes
        // emits framing junk between frames; skip leading bytes that are
        // not `=` or `!`.
        let mut buf = Vec::with_capacity(16);
        let mut byte = [0u8; 1];
        loop {
            timeout(deadline, stream.read_exact(&mut byte))
                .await
                .map_err(|_| StarAdvError::Timeout("serial read".to_string()))?
                .map_err(|e| StarAdvError::Transport(format!("serial read: {e}")))?;
            // Drop bytes before the start of a real frame.
            if buf.is_empty() && byte[0] != b'=' && byte[0] != b'!' {
                continue;
            }
            buf.push(byte[0]);
            if byte[0] == b'\r' {
                return Ok(buf);
            }
            // Hard ceiling on reply size to avoid runaway loops on a
            // misbehaving device.
            if buf.len() > 32 {
                return Err(StarAdvError::Transport(
                    "serial reply exceeded 32 bytes without terminator".to_string(),
                ));
            }
        }
    }

    async fn close(&self) -> Result<()> {
        // The serial stream's Drop closes the port; nothing to do here
        // explicitly. Returning Ok keeps the call site's idempotent
        // contract satisfied.
        Ok(())
    }
}

/// [`TransportFactory`] that opens a [`SerialTransport`] from a
/// [`Config`] whose transport block is `usb`.
#[derive(Debug, Default)]
pub struct SerialTransportFactory;

#[async_trait]
impl TransportFactory for SerialTransportFactory {
    async fn open(&self, config: &Config) -> Result<Arc<dyn Transport>> {
        match &config.transport {
            TransportConfig::Usb(usb) => {
                let t = SerialTransport::connect(usb.clone()).await?;
                Ok(Arc::new(t))
            }
            TransportConfig::Udp(_) => Err(StarAdvError::Config(
                "SerialTransportFactory requires transport.kind = \"usb\"".to_string(),
            )),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::config::{TransportConfig, UdpConfig, UsbConfig};

    #[tokio::test]
    async fn factory_open_with_udp_transport_returns_config_error() {
        // SerialTransportFactory only handles USB; passing it a UDP
        // config must produce a Config error explaining the mismatch.
        let cfg = Config {
            transport: TransportConfig::Udp(UdpConfig::default()),
            ..Config::default()
        };
        let err = match SerialTransportFactory.open(&cfg).await {
            Ok(_) => panic!("expected open() to fail"),
            Err(e) => e,
        };
        match err {
            StarAdvError::Config(msg) => {
                assert!(msg.contains("usb"), "message should mention usb: {msg}");
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_with_nonexistent_port_returns_connection_failed() {
        // Opening a path that obviously cannot exist must fail with
        // ConnectionFailed and a message naming the port — same path
        // the factory drives in production.
        let usb = UsbConfig {
            port: "/dev/this-port-does-not-exist-xyzzy".into(),
            ..UsbConfig::default()
        };
        let err = match SerialTransport::connect(usb).await {
            Ok(_) => panic!("expected connect() to fail"),
            Err(e) => e,
        };
        match err {
            StarAdvError::ConnectionFailed(msg) => {
                assert!(msg.contains("xyzzy"), "message should name the port: {msg}");
            }
            other => panic!("expected ConnectionFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn factory_propagates_connect_failure_for_bad_usb_port() {
        let usb = UsbConfig {
            port: "/dev/this-port-does-not-exist-xyzzy".into(),
            ..UsbConfig::default()
        };
        let cfg = Config {
            transport: TransportConfig::Usb(usb),
            ..Config::default()
        };
        let err = match SerialTransportFactory.open(&cfg).await {
            Ok(_) => panic!("expected open() to fail"),
            Err(e) => e,
        };
        assert!(matches!(err, StarAdvError::ConnectionFailed(_)));
    }
}
