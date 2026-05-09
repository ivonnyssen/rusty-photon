//! USB-CDC serial transport (tokio-serial).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::config::{Config, TransportConfig, UsbConfig};
use crate::error::{Result, StarAdvError};
use crate::transport::{Transport, TransportFactory};

/// Holds the open serial port and buffers the read side up to `\r`.
pub struct SerialTransport {
    _config: UsbConfig,
}

impl SerialTransport {
    /// Open the configured port and return a transport ready to round-trip.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn connect(_config: UsbConfig) -> Result<Self> {
        unimplemented!("Phase 3: open tokio-serial, set 8N1, raw mode")
    }
}

#[async_trait]
impl Transport for SerialTransport {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn round_trip(&self, _request: &[u8], _timeout: Duration) -> Result<Vec<u8>> {
        unimplemented!("Phase 3: write request, read until '\\r'")
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn close(&self) -> Result<()> {
        unimplemented!("Phase 3: drop port")
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
