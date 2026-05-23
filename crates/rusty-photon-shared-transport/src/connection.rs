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
//!
//! Each connection optionally carries an `Arc<Notify>` (set by
//! [`crate::SharedTransport`] when it constructs the connection) that
//! fires once per `TransportError` observed in `request`. The
//! reconnect supervisor listens on this notify to react to mid-stream
//! transport loss — codec errors and skip-budget exhaustion do **not**
//! fire it, since those are protocol mismatches that a reconnect
//! cannot fix.

use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

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
    /// Notify fired on every `TransportError` from `request`. `None`
    /// for ad-hoc connections (tests, the initial-handshake path inside
    /// `SharedTransport::start` before the supervisor is wired);
    /// `Some(_)` for connections handed out via `acquire()` once the
    /// supervisor is running.
    reconnect_signal: Option<Arc<Notify>>,
}

impl<C: Codec> Connection<C> {
    /// Create a connection that owns `transport` and uses `codec` for
    /// all frame translation. Internal; only [`crate::SharedTransport`]
    /// constructs these.
    pub(crate) fn new(transport: Box<dyn FrameTransport>, codec: C) -> Self {
        Self {
            transport: Mutex::new(transport),
            codec,
            reconnect_signal: None,
        }
    }

    /// Attach a reconnect signal. Called by
    /// [`crate::SharedTransport`] right after constructing a connection
    /// destined for the slot, before it's published to clients.
    pub(crate) fn with_reconnect_signal(mut self, signal: Arc<Notify>) -> Self {
        self.reconnect_signal = Some(signal);
        self
    }

    /// Send `cmd` and return the matching typed response.
    ///
    /// Holds the command lock for the entire request/response
    /// exchange: encode → `send_frame` → (read frames until one matches
    /// or `max_skip` is exhausted) → decode. The lock is released when
    /// this future completes (success or error).
    ///
    /// On a [`crate::TransportError`], also fires the attached
    /// `reconnect_signal` (if any). Codec errors and skip-budget
    /// exhaustion do not signal — those are protocol mismatches, not
    /// hardware loss.
    pub async fn request(&self, cmd: C::Command) -> Result<C::Response, SessionError<C::Error>> {
        let bytes = self.codec.encode(&cmd);
        let mut transport = self.transport.lock().await;
        match transport.send_frame(&bytes).await {
            Ok(()) => {}
            Err(e) => {
                self.signal_reconnect();
                return Err(SessionError::Transport(e));
            }
        }

        let mut buf = Vec::new();
        let budget = self.codec.max_skip();
        for skipped in 0..=budget {
            if let Err(e) = transport.recv_frame(&mut buf).await {
                self.signal_reconnect();
                return Err(SessionError::Transport(e));
            }
            let resp = self.codec.decode(&buf).map_err(SessionError::Codec)?;
            if self.codec.matches(&cmd, &resp) {
                return Ok(resp);
            }
            let _ = skipped; // Tracing point if we ever want it.
        }
        Err(SessionError::SkipExhausted(budget.saturating_add(1)))
    }

    fn signal_reconnect(&self) {
        if let Some(sig) = self.reconnect_signal.as_ref() {
            sig.notify_one();
        }
    }
}
