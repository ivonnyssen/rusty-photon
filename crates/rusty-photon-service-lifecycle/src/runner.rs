use std::future::Future;

use crate::{ReloadSignal, Shutdown};

type ServiceResult = Result<(), Box<dyn std::error::Error>>;

/// Builder for a Rusty Photon service binary's lifecycle.
///
/// Owns the tokio runtime, installs OS signal handlers (or dispatches to the
/// Windows Service Control Manager when `scm` feature + [`Self::scm_mode`]
/// are enabled), and invokes the user closure with a [`Shutdown`] handle.
///
/// ## Usage
///
/// ```no_run
/// use rusty_photon_service_lifecycle::ServiceRunner;
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     ServiceRunner::new("my-service").run(|shutdown| async move {
///         // build server, race against shutdown.cancelled()
///         let _ = shutdown;
///         Ok(())
///     })
/// }
/// ```
///
/// For a service that also needs reload (filemonitor-style), enable
/// [`Self::with_reload`] and call [`Self::run_with_reload`]:
///
/// ```no_run
/// use rusty_photon_service_lifecycle::ServiceRunner;
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     ServiceRunner::new("my-service")
///         .with_reload()
///         .run_with_reload(|shutdown, reload| async move {
///             let _ = (shutdown, reload);
///             Ok(())
///         })
/// }
/// ```
pub struct ServiceRunner {
    #[allow(dead_code)] // wired up by Phase 1 impl (SCM dispatch needs the name)
    name: &'static str,
    #[allow(dead_code)] // wired up by Phase 1 impl
    reload: bool,
    #[cfg(feature = "scm")]
    #[allow(dead_code)] // wired up by Phase 1 impl
    scm_mode: bool,
}

impl ServiceRunner {
    /// Create a runner with the given service name. The name is used for
    /// SCM registration (when `scm_mode` is on) and is otherwise informational.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            reload: false,
            #[cfg(feature = "scm")]
            scm_mode: false,
        }
    }

    /// Enable the reload signal. Required before [`Self::run_with_reload`].
    ///
    /// When enabled, the runner additionally installs `SIGHUP` handling
    /// (Unix) or accepts `ServiceControl::ParamChange` (Windows SCM mode).
    /// Each event wakes the [`ReloadSignal`] passed to the user closure.
    #[must_use]
    pub fn with_reload(mut self) -> Self {
        self.reload = true;
        self
    }

    /// Windows SCM dispatch toggle. When `enable` is `true`, the runner
    /// registers with the Windows Service Control Manager (translating
    /// `Stop` and `ParamChange` events into shutdown/reload). When `false`,
    /// runs in console mode with OS signal handlers.
    ///
    /// Service binaries typically wire this to a hidden CLI flag passed by
    /// SCM (`--service`).
    #[cfg(feature = "scm")]
    #[must_use]
    pub fn scm_mode(mut self, enable: bool) -> Self {
        self.scm_mode = enable;
        self
    }

    /// Build a multi-thread tokio runtime, install signal handlers (or
    /// dispatch SCM), and invoke `run_fn` with a [`Shutdown`] handle.
    /// Blocks until `run_fn`'s future resolves.
    ///
    /// Returns the error from `run_fn`, if any. Signal-install failures are
    /// logged via `tracing::warn!` rather than returned.
    pub fn run<F, Fut>(self, run_fn: F) -> ServiceResult
    where
        F: FnOnce(Shutdown) -> Fut + Send + 'static,
        Fut: Future<Output = ServiceResult> + Send,
    {
        let _ = (self, run_fn);
        // Phase 1 implementation: see docs/plans/service-lifecycle-unification.md
        // 1. (scm) if scm_mode: dispatch to windows_service::service_dispatcher::start
        //    with ffi_service_main that re-enters this same flow from inside SCM
        // 2. build tokio::runtime::Builder::new_multi_thread() runtime
        // 3. spawn signal watcher task that cancels a CancellationToken
        // 4. block_on(run_fn(Shutdown::from_token(token)))
        todo!("Phase 1 implementation")
    }

    /// Like [`Self::run`] but also passes a [`ReloadSignal`]. Requires
    /// [`Self::with_reload`].
    pub fn run_with_reload<F, Fut>(self, run_fn: F) -> ServiceResult
    where
        F: FnOnce(Shutdown, ReloadSignal) -> Fut + Send + 'static,
        Fut: Future<Output = ServiceResult> + Send,
    {
        let _ = (self, run_fn);
        // Phase 1 implementation: same as run(), plus a SIGHUP / SCM ParamChange
        // watcher that calls reload.notify() per event.
        todo!("Phase 1 implementation")
    }
}
