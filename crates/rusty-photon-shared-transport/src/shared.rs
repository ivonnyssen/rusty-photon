//! Core lifecycle: refcounted multi-client sharing of one duplex transport.
//!
//! [`SharedTransport`] owns the open-state lock, the refcount, the slot
//! holding the current [`Connection`], and the optional while-open
//! task's join handle + cancellation token. All transitions go through
//! [`SharedTransport::acquire`] (0→1+ and reuse) and the internal
//! `release_inline` / `release_detached` paths called by
//! [`Session::close`] and `Session::drop`.
//!
//! Two lifecycle modes coexist (see the
//! [`eager-hardware-validation`][lifecycle-plan] plan for the rationale):
//!
//! - **`LazyAcquire`** (default, unchanged from pre-Phase-0a): the port
//!   opens on the 0→1 `acquire()` and closes on the 1→0 `Session::close()`.
//!   `Hooks::on_last_disconnect` runs on 1→0; `Hooks::shutdown` is never
//!   invoked. Services that don't call [`SharedTransport::start`] get
//!   this behaviour.
//! - **`ServiceLifetime`** (opt-in via [`SharedTransport::start`]): the
//!   port opens at `start()` and stays open until [`SharedTransport::shutdown`]
//!   is called. `acquire()` becomes a fast refcount-bump.
//!   `Hooks::on_last_disconnect` runs on every 1→0 and the port stays
//!   open. `Hooks::shutdown` runs once on `shutdown()` before the port
//!   actually closes.
//!
//! The mode is a single [`AtomicBool`] flipped exclusively by `start()`
//! and `shutdown()`; all branches that need to discriminate read it
//! once under the [`acquire_lock`](SharedTransport::acquire_lock) so the
//! observation is consistent for the duration of one lifecycle
//! transition.
//!
//! [lifecycle-plan]: ../../../../docs/plans/eager-hardware-validation.md
//! [`Session::close`]: crate::Session::close

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::codec::Codec;
use crate::connection::Connection;
use crate::error::{SessionError, TransportError};
use crate::session::{Hooks, Session, WhileOpen};
use crate::transport::TransportFactory;

/// Bounded join timeout for the while-open task at teardown.
///
/// If the task ignores cancellation it gets `abort()`-ed and teardown
/// proceeds. The request-arbitration lock the task may have been holding
/// is released by the abort (its connection clone drops).
const WHILE_OPEN_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Refcounted multi-client lifecycle wrapper around a single duplex
/// transport.
///
/// Constructed once at service startup and shared across all device
/// types that need the same wire. Every device calls
/// [`SharedTransport::acquire`] to get a [`Session`]; the 0→1 transition
/// runs [`Hooks::handshake`] and spawns [`Hooks::while_open`]; the 1→0
/// transition cancels the while-open task and runs [`Hooks::teardown`].
pub struct SharedTransport<C: Codec> {
    factory: Arc<dyn TransportFactory>,
    codec: C,
    hooks: Hooks<C>,

    /// External session refcount. Incremented on every [`acquire`], decremented
    /// on every [`Session::close`] / [`Session::drop`]. The while-open task
    /// does NOT participate.
    ///
    /// [`acquire`]: SharedTransport::acquire
    /// [`Session::close`]: crate::Session::close
    /// [`Session::drop`]: crate::Session
    count: AtomicU32,
    /// `true` between handshake-success and teardown-start. Distinct from
    /// `count > 0` because a connect-in-flight has `count == 1` but
    /// `available == false`, and a teardown-in-flight has `count == 0` but
    /// the underlying transport hasn't closed yet.
    available: AtomicBool,
    /// `false` until [`SharedTransport::start`] is called; `true` between
    /// start and [`SharedTransport::shutdown`]. Drives the
    /// `LazyAcquire`-vs-`ServiceLifetime` mode discrimination at the
    /// top of [`acquire`](Self::acquire) and inside
    /// `run_last_disconnect_locked`. See the module-level docstring for
    /// the two modes' behaviour.
    service_lifetime: AtomicBool,
    /// `Some` between the 0→1 open and the 1→0 close. Cloned out for every
    /// new [`Session`]; cleared at teardown so the underlying transport
    /// can drop.
    slot: Mutex<Option<Arc<Connection<C>>>>,
    /// Serialises [`acquire`] against the inline / detached cleanup paths.
    /// Held across the entire 0→1 transition and the entire 1→0 transition;
    /// the fast path (acquire when `count > 0`) takes it just long enough to
    /// increment and read the slot.
    ///
    /// [`acquire`]: SharedTransport::acquire
    acquire_lock: Mutex<()>,
    while_open_state: Mutex<Option<(JoinHandle<()>, CancellationToken)>>,
}

impl<C: Codec> SharedTransport<C> {
    /// Build the shared transport. Returns an [`Arc`] because every
    /// device handle stores one, plus the [`Session`]s the devices hold.
    pub fn new(factory: Arc<dyn TransportFactory>, codec: C, hooks: Hooks<C>) -> Arc<Self> {
        Arc::new(Self {
            factory,
            codec,
            hooks,
            count: AtomicU32::new(0),
            available: AtomicBool::new(false),
            service_lifetime: AtomicBool::new(false),
            slot: Mutex::new(None),
            acquire_lock: Mutex::new(()),
            while_open_state: Mutex::new(None),
        })
    }

    /// Returns `true` between successful handshake and the start of teardown.
    ///
    /// Cheap, non-blocking. A `true` from this method is a moment-in-time
    /// snapshot — by the time the caller acts on it the transport may
    /// have started teardown. Use [`acquire`](Self::acquire) to obtain
    /// a guarantee.
    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    /// Opt in to `ServiceLifetime` mode: open the port, run the
    /// handshake (which is the identity-probe checkpoint), and spawn
    /// the while-open task. The port stays open until
    /// [`shutdown`](Self::shutdown) is called, regardless of how many
    /// clients are connected.
    ///
    /// Intended call site: the service binary's `main`, after config
    /// load and before binding the Alpaca HTTP server. On error,
    /// `main` should map the returned [`SessionError`] to a non-zero
    /// `ExitCode` so systemd / orchestration treats startup as a
    /// failure instead of advertising a broken device.
    ///
    /// Idempotent: a second call observes `service_lifetime == true`
    /// and returns `Ok(())` immediately.
    pub async fn start(&self) -> Result<(), SessionError<C::Error>> {
        let _guard = self.acquire_lock.lock().await;

        if self.service_lifetime.load(Ordering::SeqCst) && self.available.load(Ordering::SeqCst) {
            // Already in ServiceLifetime mode with an open transport
            // — idempotent.
            return Ok(());
        }

        if self.available.load(Ordering::SeqCst) {
            // A client already opened the transport lazily via
            // `acquire()` before `start()` was called. Promote the
            // existing transport to ServiceLifetime mode in place —
            // the slot is populated, while_open is running, the
            // handshake already ran. No I/O needed; just flip the flag
            // so the next 1→0 transition keeps the port open instead
            // of tearing it down.
            self.service_lifetime.store(true, Ordering::SeqCst);
            return Ok(());
        }

        // Cold start covers both first-ever start and start-after-shutdown.
        // Falls through to the open / handshake / publish sequence below.

        // Cold start: open the transport, run the handshake, spawn
        // while_open. Structurally identical to the 0→1 path inside
        // `acquire()` but the refcount stays at 0 — the service holds
        // the transport open via the `service_lifetime` flag, not via
        // a refcount slot.
        let raw_transport = self.factory.open().await.map_err(SessionError::Transport)?;
        let connection = Arc::new(Connection::new(raw_transport, self.codec.clone()));

        (self.hooks.handshake)(&connection)
            .await
            .map_err(SessionError::Codec)?;

        // Build the while_open future BEFORE publishing so a panic in
        // the closure body doesn't leave the slot populated. Mirrors
        // the same precaution in `acquire()`.
        let while_open_pending = match self.hooks.while_open.as_ref() {
            Some(while_open_fn) => {
                let cancel = CancellationToken::new();
                let ctx = WhileOpen::new(connection.clone(), cancel.clone());
                let fut = while_open_fn(ctx);
                Some((fut, cancel))
            }
            None => None,
        };

        *self.slot.lock().await = Some(connection);
        self.available.store(true, Ordering::SeqCst);
        self.service_lifetime.store(true, Ordering::SeqCst);

        if let Some((fut, cancel)) = while_open_pending {
            let handle = tokio::spawn(fut);
            *self.while_open_state.lock().await = Some((handle, cancel));
        }

        Ok(())
    }

    /// Exit `ServiceLifetime` mode: cancel the while-open task, run
    /// [`Hooks::shutdown`], drop the connection (closing the port).
    /// Called from the service's SIGTERM handler.
    ///
    /// Live sessions are not force-closed; their requests will fail
    /// once the underlying transport's last `Arc<Connection<C>>` drops.
    /// The service is responsible for ordering — stop accepting new
    /// HTTP requests and wait for in-flight clients to disconnect
    /// before calling `shutdown()`.
    ///
    /// No-op in `LazyAcquire` mode (returns `Ok(())` immediately).
    /// After a successful `shutdown()` the transport is back in
    /// `Closed` state; a fresh [`start`](Self::start) re-opens it.
    pub async fn shutdown(&self) -> Result<(), TransportError> {
        let _guard = self.acquire_lock.lock().await;

        if !self.service_lifetime.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.available.store(false, Ordering::SeqCst);

        // Cancel while_open BEFORE running the shutdown hook so the
        // poll loop doesn't race the final cleanup commands on the
        // wire (the shutdown hook holds the command lock via its
        // `request` calls; while_open holding the same lock would
        // serialise but could time out depending on the poll cadence).
        let while_open = self.while_open_state.lock().await.take();
        if let Some((mut handle, cancel)) = while_open {
            cancel.cancel();
            match tokio::time::timeout(WHILE_OPEN_TEARDOWN_TIMEOUT, &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(join_err)) => {
                    warn!(
                        error = %join_err,
                        "while_open task panicked or was cancelled before shutdown"
                    );
                }
                Err(_) => {
                    handle.abort();
                    warn!(
                        timeout = ?WHILE_OPEN_TEARDOWN_TIMEOUT,
                        "while_open task did not respond to cancellation; aborted"
                    );
                }
            }
        }

        let conn = self.slot.lock().await.take();
        if let Some(conn) = conn {
            (self.hooks.shutdown)(&conn).await;
            drop(conn);
        }

        // Leave `service_lifetime = true` so subsequent `acquire()` calls
        // observe `service_lifetime && !available` and refuse. The next
        // `start()` will see `!available` and do a fresh cold-start
        // (intended for tests / restart scenarios; production normally
        // exits the process after `shutdown()`).
        Ok(())
    }

    /// Hand out a [`Session`]. In `LazyAcquire` mode (default), the 0→1
    /// transition opens the transport, runs the handshake, and starts
    /// the while-open task. In `ServiceLifetime` mode (after
    /// [`start`](Self::start)), every `acquire` is a fast refcount-bump
    /// and slot-clone — the port is already open.
    ///
    /// In `ServiceLifetime` mode after [`shutdown`](Self::shutdown) has
    /// been called, returns `TransportError::Io("transport has been shut
    /// down")`.
    pub async fn acquire(self: &Arc<Self>) -> Result<Session<C>, SessionError<C::Error>> {
        let _guard = self.acquire_lock.lock().await;
        let prev = self.count.fetch_add(1, Ordering::SeqCst);

        let service_lifetime = self.service_lifetime.load(Ordering::SeqCst);

        if prev == 0 && service_lifetime && !self.available.load(Ordering::SeqCst) {
            // ServiceLifetime mode but `shutdown()` already ran. Roll
            // back the speculative increment and refuse — the service
            // is going down and acquiring a new session would defeat
            // the orderly teardown.
            self.count.fetch_sub(1, Ordering::SeqCst);
            return Err(SessionError::Transport(TransportError::Io(
                io::Error::other("transport has been shut down"),
            )));
        }

        if prev == 0 && !service_lifetime {
            // LazyAcquire 0→1 path: drop-guarded so any error or panic
            // through the handshake rolls the count back.
            let mut rollback = RollbackGuard {
                count: &self.count,
                armed: true,
            };

            let raw_transport = self.factory.open().await.map_err(SessionError::Transport)?;
            let connection = Arc::new(Connection::new(raw_transport, self.codec.clone()));

            (self.hooks.handshake)(&connection)
                .await
                .map_err(SessionError::Codec)?;

            // Build the while-open future BEFORE publishing slot /
            // available — a panic in the user-supplied closure body
            // must roll back without leaving the slot populated. The
            // future itself is `Send` and we keep it local until the
            // publish phase below.
            let while_open_pending = match self.hooks.while_open.as_ref() {
                Some(while_open_fn) => {
                    let cancel = CancellationToken::new();
                    let ctx = WhileOpen::new(connection.clone(), cancel.clone());
                    let fut = while_open_fn(ctx);
                    Some((fut, cancel))
                }
                None => None,
            };

            // Publish phase: from here on every step is infallible
            // (atomic store, async Mutex::lock without poisoning,
            // tokio::spawn inside an established runtime). The
            // rollback can safely be disarmed before these run.
            rollback.armed = false;
            *self.slot.lock().await = Some(connection.clone());
            self.available.store(true, Ordering::SeqCst);

            if let Some((fut, cancel)) = while_open_pending {
                let handle = tokio::spawn(fut);
                *self.while_open_state.lock().await = Some((handle, cancel));
            }

            return Ok(Session::new(Arc::clone(self), connection));
        }

        // Reuse / ServiceLifetime fast path: clone the slot's Arc.
        // Both flavours land here — LazyAcquire when count was already
        // > 0 (another caller already opened the transport), and
        // ServiceLifetime where `start()` populated the slot with
        // count == 0.
        let slot = self.slot.lock().await;
        let Some(connection) = slot.as_ref().cloned() else {
            // In LazyAcquire mode this is impossible by construction —
            // the 0→1 path populates `slot` before releasing
            // `acquire_lock`. In ServiceLifetime mode this can only
            // fire if a buggy caller invoked `acquire()` between
            // `start()` failing and the failure propagating; the
            // rollback path leaves both `available` and
            // `service_lifetime` false, but a successful `start()`
            // would have populated the slot before flipping the flag.
            // Either way, roll back the speculative increment and
            // surface an I/O error rather than panicking.
            drop(slot);
            self.count.fetch_sub(1, Ordering::SeqCst);
            return Err(SessionError::Transport(TransportError::Io(
                io::Error::other("transport refcount > 0 but slot empty"),
            )));
        };
        Ok(Session::new(Arc::clone(self), connection))
    }

    /// Inline release path. Called by [`Session::close`]. If we're the
    /// last live session, runs the cleanup body and returns its result.
    /// Otherwise just decrements and returns.
    ///
    /// [`Session::close`]: crate::Session::close
    pub(crate) async fn release_inline(&self) -> Result<(), TransportError> {
        let _guard = self.acquire_lock.lock().await;
        let prev = self.count.fetch_sub(1, Ordering::SeqCst);
        if prev > 1 {
            return Ok(());
        }
        // prev == 1 → we were the last; or prev == 0 → bug (caller
        // released twice). The latter is asserted out so it fails fast
        // in development; in release builds a stray fetch_sub on 0
        // wraps to u32::MAX and we'd still pass the prev > 1 check above.
        debug_assert_eq!(prev, 1, "release_inline called with refcount=0");
        self.run_cleanup_locked().await
    }

    /// Detached release path. Called by [`Session::drop`]. Spawns the
    /// inline release as a fire-and-forget task on the current tokio
    /// runtime. If no runtime is available the refcount is **not**
    /// decremented and teardown is **not** run; this matches the
    /// documented Drop-is-fallback contract.
    ///
    /// [`Session::drop`]: crate::Session
    pub(crate) fn release_detached(self: Arc<Self>) {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(e) = self.release_inline().await {
                        warn!(
                            error = %e,
                            "detached transport cleanup failed (Session::drop fallback path)"
                        );
                    }
                });
            }
            Err(_) => {
                warn!(
                    "no tokio runtime available for Session::drop cleanup; \
                     refcount stuck and teardown skipped — call Session::close().await \
                     from inside a tokio runtime instead"
                );
            }
        }
    }

    /// 1→0 cleanup body. The caller must hold [`acquire_lock`](Self::acquire_lock).
    ///
    /// Always runs [`Hooks::on_last_disconnect`] against the live slot
    /// connection. Then, **only in `LazyAcquire` mode**, does the full
    /// transport teardown (cancels while-open, drops the slot's
    /// connection so the port closes). In `ServiceLifetime` mode the
    /// port stays open across this transition; the supervisor /
    /// service binary owns transport teardown via [`shutdown`](Self::shutdown).
    async fn run_cleanup_locked(&self) -> Result<(), TransportError> {
        // Re-check count: a new acquire could not have raced in while we
        // held the lock, but the symmetric check makes the invariant
        // explicit and is a cheap sanity net.
        if self.count.load(Ordering::SeqCst) > 0 {
            return Ok(());
        }

        // Safety teardown: run `on_last_disconnect` against the live
        // connection. Borrow rather than take so the slot stays
        // populated in ServiceLifetime mode and the hook can issue
        // wire commands.
        {
            let slot_guard = self.slot.lock().await;
            if let Some(conn) = slot_guard.as_ref() {
                (self.hooks.on_last_disconnect)(conn).await;
            }
        }

        if self.service_lifetime.load(Ordering::SeqCst) {
            // ServiceLifetime mode: port stays open. while_open keeps
            // running. The next client's `acquire()` will reuse the
            // existing slot connection. Done.
            return Ok(());
        }

        // LazyAcquire mode: full transport teardown.
        self.available.store(false, Ordering::SeqCst);

        let while_open = self.while_open_state.lock().await.take();
        if let Some((mut handle, cancel)) = while_open {
            cancel.cancel();
            match tokio::time::timeout(WHILE_OPEN_TEARDOWN_TIMEOUT, &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(join_err)) => {
                    warn!(
                        error = %join_err,
                        "while_open task panicked or was cancelled before teardown"
                    );
                }
                Err(_) => {
                    handle.abort();
                    warn!(
                        timeout = ?WHILE_OPEN_TEARDOWN_TIMEOUT,
                        "while_open task did not respond to cancellation; aborted"
                    );
                }
            }
        }

        let conn = self.slot.lock().await.take();
        if let Some(conn) = conn {
            // `conn` is now the only Arc holding the Connection (the
            // while_open task's clone dropped when its future ended;
            // sessions all dropped before we got here). When `conn`
            // drops at the end of this scope the inner FrameTransport
            // drops with it and the OS-level conduit closes.
            drop(conn);
        }
        Ok(())
    }
}

/// On drop, decrements `count` unless explicitly disarmed. Used by
/// [`SharedTransport::acquire`] so an error or panic between
/// `count.fetch_add(1)` and a successful `Session` return rolls the
/// count back to its prior value.
struct RollbackGuard<'a> {
    count: &'a AtomicU32,
    armed: bool,
}

impl Drop for RollbackGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.count.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
