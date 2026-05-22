use std::sync::Arc;
use tokio::sync::Notify;

/// Reload notifier.
///
/// Wakes once per reload event. Driven by `SIGHUP` on Unix, by
/// `ServiceControl::ParamChange` in Windows SCM mode, and never on Windows
/// console mode. Enable by calling [`crate::ServiceRunner::with_reload`]
/// before [`crate::ServiceRunner::run_with_reload`].
///
/// Cloning shares the underlying [`Notify`] so multiple consumers can
/// receive the same reload event; today the only consumer pattern is a
/// single `run_server_loop`-style coordinator awaiting on it inside
/// `tokio::select!`.
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
