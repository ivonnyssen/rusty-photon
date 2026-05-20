//! Per-client session handle, while-open context, and lifecycle hooks.
//!
//! * [`Session`] is the handle every client holds. Acquired from
//!   [`crate::SharedTransport::acquire`]. Closed via
//!   [`Session::close`] (primary) or dropped (fallback).
//! * [`WhileOpen`] is the context handed to the [`Hooks::while_open`]
//!   task. Same `request` API as `Session` but does **not** participate
//!   in the external refcount — it's infrastructure, not a client.
//! * [`Hooks`] is the per-service plug for connect-side / disconnect-side
//!   / while-open work.

use std::future::Future;
use std::sync::Arc;

use derive_more::Debug;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

use crate::codec::Codec;
use crate::connection::Connection;
use crate::error::{SessionError, TransportError};
use crate::shared::SharedTransport;
use crate::BoxFuture;

/// A live, refcounted handle to the shared transport.
///
/// While at least one `Session` is alive the transport stays open and
/// any [`Hooks::while_open`] task keeps running. Acquired exclusively
/// through [`crate::SharedTransport::acquire`].
#[derive(Debug)]
#[debug("Session {{ closed: {}, .. }}", transport.is_none())]
pub struct Session<C: Codec> {
    transport: Option<Arc<SharedTransport<C>>>,
    connection: Option<Arc<Connection<C>>>,
}

impl<C: Codec> Session<C> {
    pub(crate) fn new(transport: Arc<SharedTransport<C>>, connection: Arc<Connection<C>>) -> Self {
        Self {
            transport: Some(transport),
            connection: Some(connection),
        }
    }

    /// Send `cmd` and return the matching typed response.
    ///
    /// Forwards to [`Connection::request`] — the request arbitration
    /// lock makes this call safe to run concurrently from multiple
    /// `Session`s (and the while-open task) sharing the same transport.
    pub async fn request(&self, cmd: C::Command) -> Result<C::Response, SessionError<C::Error>> {
        let connection = self
            .connection
            .as_ref()
            .expect("session.request after close/drop");
        connection.request(cmd).await
    }

    /// Primary teardown path.
    ///
    /// On the last live session, awaits while-open cancellation, runs
    /// [`Hooks::teardown`], closes the transport — all inline. Callers
    /// see today's observable behaviour: rollback is complete before
    /// this future resolves.
    ///
    /// On a non-last session, decrements the refcount and returns
    /// `Ok(())` immediately.
    pub async fn close(mut self) -> Result<(), TransportError> {
        let transport = self
            .transport
            .take()
            .expect("session.close after close/drop");
        // Drop our connection clone before the cleanup runs so the
        // refcount on the inner Arc<Connection<C>> reaches 1 (only the
        // slot's clone remains) by the time `run_cleanup` takes the
        // slot's connection out.
        self.connection.take();
        transport.release_inline().await
    }
}

impl<C: Codec> Drop for Session<C> {
    /// Detached cleanup safety net.
    ///
    /// If [`Session::close`] hasn't already consumed `transport`, this
    /// decrements the refcount and — if we're the last out — spawns
    /// the cleanup body on the current tokio runtime. The spawn is
    /// fire-and-forget; if no runtime is available or the runtime is
    /// shutting down, teardown commands may not run. See the design
    /// plan for the explicit-close-is-primary rationale.
    fn drop(&mut self) {
        if let Some(transport) = self.transport.take() {
            self.connection.take();
            transport.release_detached();
        }
    }
}

/// Non-refcounted context handed to the [`Hooks::while_open`] closure.
///
/// Shares the inner `Arc<Connection<C>>` with the primary `acquire()`
/// path, so [`WhileOpen::request`] goes through the same request
/// arbitration lock as the [`Session`]s' requests. Dropping a
/// `WhileOpen` does **not** touch the external refcount.
#[derive(Debug)]
#[debug("WhileOpen {{ cancelled: {}, .. }}", cancel.is_cancelled())]
pub struct WhileOpen<C: Codec> {
    connection: Arc<Connection<C>>,
    cancel: CancellationToken,
}

impl<C: Codec> WhileOpen<C> {
    pub(crate) fn new(connection: Arc<Connection<C>>, cancel: CancellationToken) -> Self {
        Self { connection, cancel }
    }

    /// Send `cmd` and return the matching typed response.
    pub async fn request(&self, cmd: C::Command) -> Result<C::Response, SessionError<C::Error>> {
        self.connection.request(cmd).await
    }

    /// Future that resolves when the surrounding [`SharedTransport`]
    /// begins teardown. Poll loops should `tokio::select!` between
    /// this and their interval tick so cancellation interrupts the
    /// next sleep promptly.
    pub fn cancelled(&self) -> WaitForCancellationFuture<'_> {
        self.cancel.cancelled()
    }

    /// Returns `true` once teardown has fired the cancellation token.
    /// Non-blocking; useful for `if ctx.is_cancelled() { break; }`.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// Closure type for the [`Hooks::handshake`] hook.
pub type HandshakeFn<C> = Box<
    dyn for<'a> Fn(&'a Connection<C>) -> BoxFuture<'a, Result<(), <C as Codec>::Error>>
        + Send
        + Sync,
>;

/// Closure type for the [`Hooks::teardown`] hook.
pub type TeardownFn<C> = Box<dyn for<'a> Fn(&'a Connection<C>) -> BoxFuture<'a, ()> + Send + Sync>;

/// Closure type for the [`Hooks::while_open`] hook.
pub type WhileOpenFn<C> = Box<dyn Fn(WhileOpen<C>) -> BoxFuture<'static, ()> + Send + Sync>;

/// Service-specific plug for the three lifecycle phases.
///
/// * `handshake` runs on the 0→1 connect transition, **before** any
///   [`Session`] escapes. On error: rollback (count→0, available→false,
///   transport dropped), error propagated to the [`crate::SharedTransport::acquire`]
///   caller.
/// * `teardown` runs on the 1→0 disconnect transition, **after** any
///   while-open task has been cancelled and joined. Best-effort: errors
///   that need to reach the caller surface through [`Session::close`]'s
///   `Result<(), TransportError>`.
/// * `while_open` (optional) spawns after `handshake` succeeds and is
///   driven to completion (with a bounded 5-second join, then abort)
///   before `teardown` runs.
pub struct Hooks<C: Codec> {
    pub handshake: HandshakeFn<C>,
    pub teardown: TeardownFn<C>,
    pub while_open: Option<WhileOpenFn<C>>,
}

impl<C: Codec> Hooks<C> {
    /// Hooks that do nothing on either transition and have no
    /// background poll task. Useful as a base for `.handshake = ...`
    /// chains in tests, and as a sane default for services that don't
    /// need any of the three.
    pub fn noop() -> Self {
        Self {
            handshake: Box::new(|_| Box::pin(async { Ok(()) })),
            teardown: Box::new(|_| Box::pin(async {})),
            while_open: None,
        }
    }
}

impl<C: Codec> Default for Hooks<C> {
    fn default() -> Self {
        Self::noop()
    }
}

// Type-check that the closures' futures are Send so they can cross
// tokio's task boundary. If a closure captures a non-Send value (e.g.
// Rc, RefCell) this will fail at the call site, which is the desired
// developer experience.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<BoxFuture<'static, ()>>();
};

// Silence unused: the Future bound via WaitForCancellationFuture isn't
// referenced directly by name elsewhere — explicit import keeps the
// public API surface clear in docs.
const _: fn() = || {
    fn _assert_future<F: Future<Output = ()>>() {}
};
