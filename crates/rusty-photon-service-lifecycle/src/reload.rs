use std::sync::Arc;
use tokio::sync::Notify;

/// Reload notifier.
///
/// Wakes one waiter per reload event. Driven by `SIGHUP` on Unix, by
/// `ServiceControl::ParamChange` in Windows SCM mode, and never on Windows
/// console mode. Enable by calling [`crate::ServiceRunner::with_reload`]
/// before [`crate::ServiceRunner::run_with_reload`].
///
/// **Single-consumer.** The underlying [`Notify::notify_one`] wakes a single
/// waiter per event, so two tasks both awaiting [`Self::recv`] will *not*
/// both rebuild on the same reload — one wins. The intended pattern is one
/// coordinator (e.g. `run_server_loop`) racing `recv()` against shutdown in
/// a `tokio::select!`. Cloning is supported (`Notify` lives behind an `Arc`)
/// but exists for handing the signal into a single nested scope, not for
/// fan-out.
#[derive(Clone, Debug, Default)]
pub struct ReloadSignal {
    notify: Arc<Notify>,
}

impl ReloadSignal {
    /// Construct a fresh, un-fired reload signal.
    ///
    /// The canonical producer is [`crate::ServiceRunner::run_with_reload`],
    /// which constructs one and hands it to the user closure. Callers rarely
    /// need this directly; it exists for integration tests that drive a
    /// service's run loop with synthetic reload events, and for callers
    /// that want a reload source not tied to OS signals (e.g. a file
    /// watcher feeding the same primitive).
    pub fn new() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
        }
    }

    /// Wake one waiter.
    ///
    /// Called by the runner's SIGHUP watcher and SCM control handler.
    /// Exposed publicly so tests and non-signal-driven reload sources can
    /// fire events through the same primitive consumers `await` on
    /// [`Self::recv`]; see [`Self::new`] for the canonical-producer note.
    pub fn notify(&self) {
        self.notify.notify_one();
    }

    /// Future that resolves on the next reload event.
    pub async fn recv(&self) {
        self.notify.notified().await;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recv_resolves_after_notify() {
        let reload = ReloadSignal::new();
        let waiter = {
            let reload = reload.clone();
            tokio::spawn(async move { reload.recv().await })
        };
        // Yield once so the spawned task reaches `notified().await` before we notify.
        tokio::task::yield_now().await;
        reload.notify();
        waiter.await.unwrap();
    }
}
