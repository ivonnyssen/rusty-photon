//! USB-CDC serial transport (tokio-serial).

use std::time::Duration;

use async_trait::async_trait;

use crate::config::UsbConfig;
use crate::error::Result;
use crate::transport::Transport;

/// Holds the open serial port and buffers the read side up to `\r`.
pub struct SerialTransport {
    _config: UsbConfig,
}

impl SerialTransport {
    /// Open the configured port and return a transport ready to round-trip.
    pub async fn connect(_config: UsbConfig) -> Result<Self> {
        unimplemented!("Phase 3: open tokio-serial, set 8N1, raw mode")
    }
}

#[async_trait]
impl Transport for SerialTransport {
    async fn round_trip(&self, _request: &[u8], _timeout: Duration) -> Result<Vec<u8>> {
        unimplemented!("Phase 3: write request, read until '\\r'")
    }

    async fn close(&self) -> Result<()> {
        unimplemented!("Phase 3: drop port")
    }
}
