//! UDP/WiFi (mount in AP mode, port 11880) [`TransportFactory`].
//!
//! Binds a local socket — **mandatory** in the configured bind subnet
//! because the mount silently drops replies that can't be routed back
//! — `connect()`s it to the peer, and wraps it in a
//! [`UdpFrameTransport`]. Datagram boundaries become frame boundaries
//! by construction; framing strictness (exactly one well-formed
//! `:cmd<axis><payload>\r` per packet) is preserved.

use std::io;
use std::net::SocketAddr;

use async_trait::async_trait;
use rusty_photon_shared_transport::{
    FrameTransport, TransportError, TransportFactory, UdpFrameTransport,
};
use tokio::net::UdpSocket;
use tracing::debug;

use crate::config::UdpConfig;

/// Maximum size of a single UDP frame.
///
/// The Sky-Watcher protocol's longest documented reply payload is
/// 8 bytes; 256 bytes is a generous bound that still catches a
/// runaway peer's malformed jumbo frame.
const MAX_FRAME_SIZE: usize = 256;

/// Real-hardware factory for the Sky-Watcher UDP transport.
#[derive(Debug, Clone)]
pub struct UdpTransportFactory {
    config: UdpConfig,
}

impl UdpTransportFactory {
    /// Capture the per-call configuration (peer address, bind address,
    /// timeout) at factory-construction time so [`TransportFactory::open`]
    /// can be retried by the shared-transport core without rethreading
    /// parameters.
    pub fn new(config: UdpConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl TransportFactory for UdpTransportFactory {
    async fn open(&self) -> Result<Box<dyn FrameTransport>, TransportError> {
        let bind_addr = SocketAddr::new(self.config.bind_address, 0);
        let mount_addr = SocketAddr::new(self.config.address, self.config.port);
        debug!(
            bind = %bind_addr,
            mount = %mount_addr,
            timeout = ?self.config.command_timeout,
            "opening Sky-Watcher UDP transport"
        );

        let socket = UdpSocket::bind(bind_addr).await.map_err(|e| {
            TransportError::Open(io::Error::other(format!("UDP bind {bind_addr}: {e}")))
        })?;
        socket.connect(mount_addr).await.map_err(|e| {
            TransportError::Open(io::Error::other(format!("UDP connect {mount_addr}: {e}")))
        })?;

        let transport = UdpFrameTransport::new(socket, MAX_FRAME_SIZE)
            .with_read_timeout(self.config.command_timeout)
            .with_write_timeout(self.config.command_timeout);
        Ok(Box::new(transport))
    }
}
