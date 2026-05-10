//! Transport abstraction.
//!
//! A [`Transport`] takes a fully-encoded `:cmd<axis><payload?>\r` request
//! frame and returns a fully-decoded reply frame (still as bytes — the codec
//! layer parses it). Both serial and UDP implementations satisfy this trait.
//!
//! [`TransportFactory`] separates "how do I open a transport from a
//! [`Config`]" from "what does an already-open transport do." This is what
//! lets [`crate::TransportManager`] honour the ref-counted open/close
//! semantics in the design doc: the manager owns the factory and only
//! materialises a [`Transport`] on the 0→1 connect transition. Same pattern
//! as `qhy-focuser::SerialPortFactory`.

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::error::Result;

pub mod serial;
pub mod udp;

#[cfg(feature = "mock")]
pub mod mock;

/// Send-and-receive primitive used by [`crate::transport_manager::TransportManager`].
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send `request` (already framed with `:` prefix and `\r` terminator)
    /// and read one complete reply frame within `timeout`.
    ///
    /// Returns the raw reply bytes including the `=` or `!` prefix and the
    /// trailing `\r`. Decoding is the caller's responsibility.
    async fn round_trip(&self, request: &[u8], timeout: Duration) -> Result<Vec<u8>>;

    /// Tear down the underlying connection. Idempotent.
    async fn close(&self) -> Result<()>;
}

/// Constructs a [`Transport`] from a [`Config`].
///
/// The manager calls [`open`] on the 0→1 connect transition and drops the
/// returned `Arc<dyn Transport>` on the 1→0 disconnect transition. Phase 2
/// ships a usable [`MockTransportFactory`](mock::MockTransportFactory) (when
/// the `mock` feature is on) and Phase-3 stub factories for serial and UDP.
#[async_trait]
pub trait TransportFactory: Send + Sync {
    async fn open(&self, config: &Config) -> Result<Arc<dyn Transport>>;
}
