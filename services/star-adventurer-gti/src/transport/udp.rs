//! UDP transport (mount in WiFi AP mode, port 11880).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use skywatcher_motor_protocol::codec::validate_response_frame;
use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::debug;

use crate::config::{Config, TransportConfig, UdpConfig};
use crate::error::{Result, StarAdvError};
use crate::transport::{Transport, TransportFactory};

/// Tokio `UdpSocket` bound to the configured local address.
///
/// Binding to the explicit `bind_address` is mandatory: the mount silently
/// drops packets it can't reply to, which happens whenever the kernel picks
/// a source IP outside the 192.168.4.0/24 subnet.
pub struct UdpTransport {
    socket: UdpSocket,
}

impl UdpTransport {
    pub async fn connect(config: UdpConfig) -> Result<Self> {
        let bind_addr = SocketAddr::new(config.bind_address, 0);
        debug!(
            bind = %bind_addr,
            mount = %config.address,
            port = config.port,
            "opening UDP transport"
        );
        let socket = UdpSocket::bind(bind_addr)
            .await
            .map_err(|e| StarAdvError::ConnectionFailed(format!("UDP bind {bind_addr}: {e}")))?;
        let mount_addr = SocketAddr::new(config.address, config.port);
        socket.connect(mount_addr).await.map_err(|e| {
            StarAdvError::ConnectionFailed(format!("UDP connect {mount_addr}: {e}"))
        })?;
        Ok(Self { socket })
    }
}

#[async_trait]
impl Transport for UdpTransport {
    async fn round_trip(&self, request: &[u8], deadline: Duration) -> Result<Vec<u8>> {
        // Send the entire frame as a single datagram. The protocol's UDP
        // mode rejects anything other than exactly one well-formed
        // `:cmd<axis><payload?>\r` frame per packet.
        timeout(deadline, self.socket.send(request))
            .await
            .map_err(|_| StarAdvError::Timeout("UDP send".to_string()))?
            .map_err(|e| StarAdvError::Transport(format!("UDP send: {e}")))?;
        // Receive the reply. Buffer is small — replies are at most ~9
        // bytes (`=XXXXXX\r` or `!XX\r`). The mount sometimes appends
        // an extra `\n`, which we tolerate.
        let mut buf = [0u8; 32];
        let n = timeout(deadline, self.socket.recv(&mut buf))
            .await
            .map_err(|_| StarAdvError::Timeout("UDP recv".to_string()))?
            .map_err(|e| StarAdvError::Transport(format!("UDP recv: {e}")))?;
        let mut frame = buf[..n].to_vec();
        // Strip a trailing `\n` if the mount appended one (some firmware
        // revisions do; the reference doc lists this as tolerated).
        if frame.last() == Some(&b'\n') {
            frame.pop();
        }
        validate_response_frame(&frame).map_err(StarAdvError::from)?;
        Ok(frame)
    }

    async fn close(&self) -> Result<()> {
        // UdpSocket drops the underlying fd when Self is dropped; nothing
        // to do here. Idempotent.
        Ok(())
    }
}

/// [`TransportFactory`] that opens a [`UdpTransport`] from a [`Config`]
/// whose transport block is `udp`.
#[derive(Debug, Default)]
pub struct UdpTransportFactory;

#[async_trait]
impl TransportFactory for UdpTransportFactory {
    async fn open(&self, config: &Config) -> Result<Arc<dyn Transport>> {
        match &config.transport {
            TransportConfig::Udp(udp) => {
                let t = UdpTransport::connect(udp.clone()).await?;
                Ok(Arc::new(t))
            }
            TransportConfig::Usb(_) => Err(StarAdvError::Config(
                "UdpTransportFactory requires transport.kind = \"udp\"".to_string(),
            )),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::net::IpAddr;

    /// Spawn a localhost UDP echo server that returns canned replies for
    /// any incoming datagram.
    async fn spawn_echo_server(reply: Vec<u8>) -> SocketAddr {
        let server = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 64];
            loop {
                let (n, peer) = match server.recv_from(&mut buf).await {
                    Ok(p) => p,
                    Err(_) => return,
                };
                let _ = (n, &buf[..n]); // silence unused — body intentional
                let _ = server.send_to(&reply, peer).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn round_trip_succeeds_against_localhost_echo() {
        let server_addr = spawn_echo_server(b"=000080\r".to_vec()).await;
        let cfg = UdpConfig {
            address: IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            port: server_addr.port(),
            bind_address: IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            command_timeout: Duration::from_secs(1),
            polling_interval: Duration::from_millis(200),
        };
        let t = UdpTransport::connect(cfg).await.unwrap();
        let reply = t
            .round_trip(b":j1\r", Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(reply, b"=000080\r");
    }

    #[tokio::test]
    async fn round_trip_strips_trailing_newline() {
        let server_addr = spawn_echo_server(b"=000080\r\n".to_vec()).await;
        let cfg = UdpConfig {
            address: IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            port: server_addr.port(),
            bind_address: IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            command_timeout: Duration::from_secs(1),
            polling_interval: Duration::from_millis(200),
        };
        let t = UdpTransport::connect(cfg).await.unwrap();
        let reply = t
            .round_trip(b":j1\r", Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(reply, b"=000080\r");
    }

    #[tokio::test]
    async fn round_trip_rejects_malformed_reply() {
        // Server replies without a `\r` terminator → response frame
        // validation rejects it.
        let server_addr = spawn_echo_server(b"=000080".to_vec()).await;
        let cfg = UdpConfig {
            address: IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            port: server_addr.port(),
            bind_address: IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            command_timeout: Duration::from_secs(1),
            polling_interval: Duration::from_millis(200),
        };
        let t = UdpTransport::connect(cfg).await.unwrap();
        let err = t.round_trip(b":j1\r", Duration::from_secs(1)).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn round_trip_times_out_when_server_is_silent() {
        // Bind a server but don't reply.
        let server = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = server.local_addr().unwrap();
        // Prevent the socket from being dropped immediately; just keep
        // the binding alive without consuming datagrams.
        let _keepalive = std::thread::spawn(move || {
            // Sleep long enough that the test completes before this thread.
            std::thread::sleep(Duration::from_secs(5));
            drop(server);
        });
        let cfg = UdpConfig {
            address: IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            port: addr.port(),
            bind_address: IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            command_timeout: Duration::from_millis(100),
            polling_interval: Duration::from_millis(200),
        };
        let t = UdpTransport::connect(cfg).await.unwrap();
        let err = t
            .round_trip(b":j1\r", Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(matches!(err, StarAdvError::Timeout(_)));
    }
}
