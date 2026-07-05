//! The engine's outward-facing seams: the tool-call client and the clock.
//!
//! Both are traits so the interpreter is testable without `rp`: unit tests
//! drive the tree with a scripted [`ToolClient`] and a mock [`Clock`]
//! (design: `docs/services/session-runner.md` § Testing Strategy). The real
//! implementations are the `rmcp`-based MCP client (Phase C service wiring)
//! and [`SystemClock`].

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

/// A failed tool call, as the engine distinguishes them.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ToolCallError {
    /// The tool ran and failed, or the call itself failed. Retryable via
    /// the instruction's `retry` policy; catchable by `try`.
    #[error("{0}")]
    Failed(String),
    /// `rp` terminated the MCP session (safety, per the design's § Safety
    /// Behavior). Never retried and never caught: the engine runs
    /// enclosing `finally` blocks best-effort and exits the run without
    /// posting a completion.
    #[error("MCP session terminated: {0}")]
    SessionTerminated(String),
}

/// The engine's view of `rp`'s MCP server.
pub trait ToolClient {
    /// Call `tool` with the given JSON-object arguments and return its
    /// structured result.
    fn call(
        &self,
        tool: &str,
        args: Map<String, Value>,
    ) -> impl Future<Output = Result<Value, ToolCallError>> + Send;
}

/// The engine clock: one seam for both "what time is it" (the expression
/// context's `now`, read by `seconds_until()`) and "pause" (`wait`
/// durations, poll intervals, retry backoff), so engine tests are
/// deterministic and instant.
pub trait Clock {
    fn now(&self) -> DateTime<Utc>;
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// The production clock: `chrono::Utc::now` + `tokio::time::sleep`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}
