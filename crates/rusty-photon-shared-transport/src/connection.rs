//! Per-open-transport request arbitration.
//!
//! [`Connection`] is the internal type that owns one boxed
//! [`crate::FrameTransport`] and one [`crate::Codec`], plus the command
//! lock that serialises request/response pairs across all callers of
//! the same open transport.
//!
//! It's exposed publicly because [`crate::Hooks::handshake`] receives
//! `&Connection<C>` so handshake commands run through the same request
//! arbitration as steady-state commands — `Session` and `WhileOpen` are
//! views of the same underlying `Arc<Connection<C>>`.

use tokio::sync::Mutex;

use crate::codec::Codec;
use crate::error::SessionError;
use crate::transport::FrameTransport;

/// One open transport + its codec + a command lock.
///
/// All wire I/O for a service goes through this single point. The mutex
/// guards the boxed transport so that concurrent callers
/// (handshake, foreground requests, the while-open poll task) take
/// turns end-to-end on the wire instead of interleaving bytes.
pub struct Connection<C: Codec> {
    transport: Mutex<Box<dyn FrameTransport>>,
    codec: C,
}

impl<C: Codec> Connection<C> {
    /// Create a connection that owns `transport` and uses `codec` for
    /// all frame translation. Internal; only [`crate::SharedTransport`]
    /// constructs these.
    pub(crate) fn new(transport: Box<dyn FrameTransport>, codec: C) -> Self {
        Self {
            transport: Mutex::new(transport),
            codec,
        }
    }

    /// Send `cmd` and return the matching typed response.
    ///
    /// Holds the command lock for the entire request/response
    /// exchange: encode → `send_frame` → (read frames until one matches
    /// or `max_skip` is exhausted) → decode. The lock is released when
    /// this future completes (success or error).
    pub async fn request(&self, cmd: C::Command) -> Result<C::Response, SessionError<C::Error>> {
        let bytes = self.codec.encode(&cmd);
        let mut transport = self.transport.lock().await;
        transport.send_frame(&bytes).await?;

        let mut buf = Vec::new();
        let budget = self.codec.max_skip();
        for skipped in 0..=budget {
            transport.recv_frame(&mut buf).await?;
            let resp = self.codec.decode(&buf).map_err(SessionError::Codec)?;
            if self.codec.matches(&cmd, &resp) {
                return Ok(resp);
            }
            let _ = skipped; // Tracing point if we ever want it.
        }
        Err(SessionError::SkipExhausted(budget.saturating_add(1)))
    }
}
