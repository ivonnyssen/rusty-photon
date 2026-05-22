//! Core lifecycle: refcounted multi-client sharing of one duplex transport.
//!
//! [`SharedTransport`] owns the open-state lock, the refcount, the slot
//! holding the current [`Connection`], and the optional while-open
//! task's join handle + cancellation token. All transitions go through
//! [`SharedTransport::acquire`] (0→1+ and reuse) and the internal
//! `release_inline` / `release_detached` paths called by
//! [`Session::close`] and `Session::drop`.
//!
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

    /// Hand out a [`Session`]. On the 0→1 transition opens the transport,
    /// runs the handshake, and starts the while-open task. On a reuse
    /// (count was already ≥1) clones the existing connection Arc and
    /// hands it out.
    pub async fn acquire(self: &Arc<Self>) -> Result<Session<C>, SessionError<C::Error>> {
        let _guard = self.acquire_lock.lock().await;
        let prev = self.count.fetch_add(1, Ordering::SeqCst);

        if prev == 0 {
            // Drop-guarded 0→1 path: if any step from here through the
            // handshake either errors or panics, the rollback runs.
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

        // Reuse path: another caller already opened the transport. Clone
        // the slot's Arc.
        let slot = self.slot.lock().await;
        let Some(connection) = slot.as_ref().cloned() else {
            // Impossible by construction: the 0→1 path populates `slot`
            // before releasing `acquire_lock`, so any subsequent
            // `acquire()` that sees `count > 0` must observe `Some` here.
            // Roll back our pre-emptive `fetch_add` and surface an I/O
            // error rather than panicking so we satisfy the workspace's
            // no-panic policy.
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
    async fn run_cleanup_locked(&self) -> Result<(), TransportError> {
        // Re-check count: a new acquire could not have raced in while we
        // held the lock, but the symmetric check makes the invariant
        // explicit and is a cheap sanity net.
        if self.count.load(Ordering::SeqCst) > 0 {
            return Ok(());
        }
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
            (self.hooks.teardown)(&conn).await;
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
