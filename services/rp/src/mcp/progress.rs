//! MCP `notifications/progress` emission from long-running blocking
//! helpers.
//!
//! ## Why this exists
//!
//! rmcp 1.7's `LocalSessionManager` constructs its sessions with
//! `SessionConfig::default()`, whose `keep_alive` is
//! `Some(Duration::from_secs(300))`. The session worker selects on the
//! client-event receiver, the handler-event receiver, and a 300 s
//! `tokio::time::sleep` (`transport/streamable_http_server/session/local.rs`
//! around line 1011). The keep-alive timer is reset only when one of
//! the *other* arms fires, i.e. when the session sees activity. A
//! tool body that runs close to its own 300 s deadline without
//! emitting anything races the keep-alive: when both fire near the
//! same instant the SSE response stream EOFs and the client's
//! `call_tool` future never resolves.
//!
//! rp's blocking helpers in [`super::internals`] all have deadlines
//! that approach or match 300 s:
//!
//! - `do_slew_blocking` and the inner `poll_slewing_until_idle` —
//!   300 s slew + settle.
//! - `do_park_blocking` — 300 s `AtPark` poll.
//! - `do_capture` — exposure `duration` plus `CAPTURE_READOUT_GRACE`
//!   (120 s).
//! - `do_move_focuser_blocking` — 120 s focuser-settle deadline.
//!
//! Emitting `notifications/progress` every [`PROGRESS_INTERVAL`] from
//! each poll loop keeps the session worker's handler-event arm active
//! comfortably ahead of the 300 s timer, so the keep-alive never
//! fires for a legitimate long tool.
//!
//! ## Token plumbing
//!
//! Progress notifications are only meaningful to clients that send a
//! `progressToken` under `_meta` on the request.
//! [`ProgressSink::from_request_context`] returns `None` when no
//! token is present (or in unit tests that construct an `McpHandler`
//! without an MCP transport at all); helpers treat a `None` sink as a
//! no-op, so the emission path is purely additive.
//!
//! Tests construct a [`CountingProgressEmitter`] to inspect the
//! number and shape of emissions without instantiating a real rmcp
//! peer.

use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{Meta, ProgressNotificationParam, ProgressToken};
use rmcp::service::{Peer, RequestContext};
use rmcp::RoleServer;
use tracing::debug;

/// Cadence at which long-running helpers fire `notifications/progress`.
/// Picked well under rmcp's 300 s `SessionConfig::keep_alive` default so
/// the session worker sees session activity many times over before the
/// keep-alive timer could fire even on the longest legitimate tool run.
pub(crate) const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);

/// Abstraction over progress emission. Implemented by the real
/// [`ProgressSink`] (which actually sends notifications via
/// `Peer<RoleServer>::notify_progress`) and by test doubles that
/// record calls without a live MCP transport.
///
/// Helpers in [`super::internals`] accept `Option<&dyn ProgressEmitter>`;
/// `None` means "no client wants progress" and the helper skips the
/// emit step entirely.
#[async_trait]
pub(crate) trait ProgressEmitter: Send + Sync {
    async fn emit(&self, progress: f64, total: Option<f64>, message: Option<String>);
}

/// Live progress sink: bundles the per-request `Peer<RoleServer>` and
/// the client-supplied `ProgressToken` so a helper can emit
/// `notifications/progress` without re-fetching either every tick.
pub(crate) struct ProgressSink {
    peer: Peer<RoleServer>,
    token: ProgressToken,
}

impl ProgressSink {
    /// Construct a sink from the request's `Peer` + `_meta`. Returns
    /// `None` when the client did not supply a `progressToken` —
    /// helpers treat the missing sink as "skip emission" rather than
    /// failing the tool (most BDD clients and many real consumers do
    /// not send a token).
    pub(crate) fn from_peer_and_meta(peer: Peer<RoleServer>, meta: &Meta) -> Option<Self> {
        meta.get_progress_token().map(|token| Self { peer, token })
    }

    /// Convenience: pull both inputs off a `RequestContext`. Equivalent
    /// to calling [`Self::from_peer_and_meta`] with the context's
    /// `peer` and `meta` fields.
    pub(crate) fn from_request_context(ctx: &RequestContext<RoleServer>) -> Option<Self> {
        Self::from_peer_and_meta(ctx.peer.clone(), &ctx.meta)
    }
}

#[async_trait]
impl ProgressEmitter for ProgressSink {
    async fn emit(&self, progress: f64, total: Option<f64>, message: Option<String>) {
        let param = ProgressNotificationParam {
            progress_token: self.token.clone(),
            progress,
            total,
            message,
        };
        // A closed transport (client went away mid-tool) is a normal
        // case — surfacing it would abort the tool body for no
        // operational reason. Drop the error after a debug log.
        if let Err(e) = self.peer.notify_progress(param).await {
            debug!(error = %e, "notify_progress failed; client likely disconnected");
        }
    }
}

/// Test doubles and helpers for unit tests in this crate. Gated to
/// `#[cfg(test)]` so the production binary doesn't carry them; the
/// `#[allow]` attributes match the convention used for sibling
/// `#[cfg(test)]` blocks under `super` (e.g. `super::tests`).
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::type_complexity
)]
pub(crate) mod test_support {
    use super::ProgressEmitter;
    use async_trait::async_trait;

    /// Test double: counts emissions and stores their arguments so
    /// unit tests can assert "at least N progress notifications were
    /// sent during this run".
    pub(crate) struct CountingProgressEmitter {
        count: std::sync::atomic::AtomicUsize,
        records: std::sync::Mutex<Vec<(f64, Option<f64>, Option<String>)>>,
    }

    impl Default for CountingProgressEmitter {
        fn default() -> Self {
            Self {
                count: std::sync::atomic::AtomicUsize::new(0),
                records: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl CountingProgressEmitter {
        pub(crate) fn count(&self) -> usize {
            self.count.load(std::sync::atomic::Ordering::SeqCst)
        }

        /// Snapshot of every `(progress, total, message)` tuple emitted
        /// so far, for tests that assert on the *content* of a
        /// notification (e.g. the phase label) rather than just the count.
        pub(crate) fn records(&self) -> Vec<(f64, Option<f64>, Option<String>)> {
            self.records
                .lock()
                .expect("CountingProgressEmitter records lock poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl ProgressEmitter for CountingProgressEmitter {
        async fn emit(&self, progress: f64, total: Option<f64>, message: Option<String>) {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.records
                .lock()
                .expect("CountingProgressEmitter records lock poisoned")
                .push((progress, total, message));
        }
    }
}
