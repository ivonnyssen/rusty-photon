//! Transport abstraction.
//!
//! A [`Transport`] takes a fully-encoded `:cmd<axis><payload?>\r` request
//! frame and returns a fully-decoded reply frame (still as bytes — the codec
//! layer parses it). Both serial and UDP implementations satisfy this trait.

use async_trait::async_trait;
use std::time::Duration;

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
