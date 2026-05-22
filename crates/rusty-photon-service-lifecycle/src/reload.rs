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
#[derive(Clone, Debug)]
pub struct ReloadSignal {
    notify: Arc<Notify>,
}

impl ReloadSignal {
    #[allow(dead_code)] // wired up by the runner in Phase 1 impl
    pub(crate) fn new() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
        }
    }

    /// Wakes one waiter. Used internally by the SIGHUP watcher / SCM
    /// control handler; not part of the public-consumer API.
    #[allow(dead_code)] // wired up by the runner in Phase 1 impl
    pub(crate) fn notify(&self) {
        self.notify.notify_one();
    }

    /// Future that resolves on the next reload event.
    pub async fn recv(&self) {
        self.notify.notified().await;
    }
}

#[cfg(test)]
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
