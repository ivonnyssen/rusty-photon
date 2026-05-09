//! UDP transport (mount in WiFi AP mode, port 11880).

use std::time::Duration;

use async_trait::async_trait;

use crate::config::UdpConfig;
use crate::error::Result;
use crate::transport::Transport;

/// Tokio `UdpSocket` bound to the configured local address.
///
/// Binding to the explicit `bind_address` is mandatory: the mount silently
/// drops packets it can't reply to, which happens whenever the kernel picks
/// a source IP outside the 192.168.4.0/24 subnet.
pub struct UdpTransport {
    _config: UdpConfig,
}

impl UdpTransport {
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn connect(_config: UdpConfig) -> Result<Self> {
        unimplemented!("Phase 3: bind UdpSocket to (bind_address, 0), connect to (address, port)")
    }
}

#[async_trait]
impl Transport for UdpTransport {
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn round_trip(&self, _request: &[u8], _timeout: Duration) -> Result<Vec<u8>> {
        unimplemented!("Phase 3: send_to + recv with timeout, validate single-frame UDP rule")
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn close(&self) -> Result<()> {
        unimplemented!("Phase 3: drop socket")
    }
}
