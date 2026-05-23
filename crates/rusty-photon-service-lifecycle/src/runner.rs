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
    name: &'static str,
    reload: bool,
    scm_mode: bool,
}

impl ServiceRunner {
    /// Create a runner with the given service name. The name is used for
    /// SCM registration (when `scm_mode` is on) and is otherwise informational.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            reload: false,
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

    /// Windows SCM dispatch toggle. When `enable` is `true` *and* the
    /// `scm` cargo feature is on *and* the target is Windows, the runner
    /// registers with the Windows Service Control Manager (translating
    /// `Stop` and `ParamChange` events into shutdown/reload). Otherwise
    /// (non-Windows, feature off, or `enable = false`), runs in console
    /// mode with OS signal handlers.
    ///
    /// The method itself is always available across features and platforms
    /// — call sites do not need `cfg` gates. Service binaries typically
    /// wire `enable` to a hidden CLI flag passed by SCM (`--service`).
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
        Fut: Future<Output = ServiceResult> + 'static,
    {
        #[cfg(all(windows, feature = "scm"))]
        if self.scm_mode {
            return scm::dispatch(
                self.name,
                scm::BoxedRunFn::Plain(Box::new(move |s| Box::pin(run_fn(s)))),
            );
        }

        run_console_plain(self.name, run_fn)
    }

    /// Like [`Self::run`] but also passes a [`ReloadSignal`]. Requires
    /// [`Self::with_reload`] to have been set on the builder.
    pub fn run_with_reload<F, Fut>(self, run_fn: F) -> ServiceResult
    where
        F: FnOnce(Shutdown, ReloadSignal) -> Fut + Send + 'static,
        Fut: Future<Output = ServiceResult> + 'static,
    {
        if !self.reload {
            return Err(
                "ServiceRunner::run_with_reload requires .with_reload() on the builder".into(),
            );
        }

        #[cfg(all(windows, feature = "scm"))]
        if self.scm_mode {
            return scm::dispatch(
                self.name,
                scm::BoxedRunFn::WithReload(Box::new(move |s, r| Box::pin(run_fn(s, r)))),
            );
        }

        run_console_with_reload(self.name, run_fn)
    }
}

fn build_runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
}

fn run_console_plain<F, Fut>(name: &'static str, run_fn: F) -> ServiceResult
where
    F: FnOnce(Shutdown) -> Fut + Send + 'static,
    Fut: Future<Output = ServiceResult>,
{
    let rt = build_runtime()?;
    let token = tokio_util::sync::CancellationToken::new();
    rt.spawn(watch_shutdown_signals(name, token.clone()));
    rt.block_on(run_fn(Shutdown::from_token(token)))
}

fn run_console_with_reload<F, Fut>(name: &'static str, run_fn: F) -> ServiceResult
where
    F: FnOnce(Shutdown, ReloadSignal) -> Fut + Send + 'static,
    Fut: Future<Output = ServiceResult>,
{
    let rt = build_runtime()?;
    let token = tokio_util::sync::CancellationToken::new();
    let reload = ReloadSignal::new();
    rt.spawn(watch_shutdown_signals(name, token.clone()));
    #[cfg(unix)]
    rt.spawn(watch_reload_signal(reload.clone()));
    rt.block_on(run_fn(Shutdown::from_token(token), reload))
}

async fn watch_shutdown_signals(name: &'static str, token: tokio_util::sync::CancellationToken) {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("{name}: failed to wait for Ctrl+C: {e}");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!("{name}: failed to install SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::debug!("{name}: received Ctrl+C, shutting down"),
        () = terminate => tracing::debug!("{name}: received SIGTERM, shutting down"),
    }
    tracing::info!("{name}: shutdown signal received, terminating");
    token.cancel();
}

#[cfg(unix)]
async fn watch_reload_signal(reload: ReloadSignal) {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
        Ok(mut sig) => loop {
            sig.recv().await;
            tracing::debug!("received SIGHUP, requesting reload");
            reload.notify();
        },
        Err(e) => {
            tracing::warn!("failed to install SIGHUP handler: {e}");
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(all(windows, feature = "scm"))]
mod scm {
    //! Windows Service Control Manager dispatch.
    //!
    //! Bridges the synchronous SCM entry point (`service_dispatcher::start`)
    //! to the tokio-based runner. The user closure is type-erased into a
    //! `Box<dyn FnOnce(...)>` and stashed in a `OnceLock` so the
    //! `extern "system" fn` SCM entry point can reach it.
    use super::*;
    use std::ffi::OsString;
    use std::pin::Pin;
    use std::sync::{Mutex, OnceLock};
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    type PlainFn = Box<dyn FnOnce(Shutdown) -> Pin<Box<dyn Future<Output = ServiceResult>>> + Send>;
    type WithReloadFn = Box<
        dyn FnOnce(Shutdown, ReloadSignal) -> Pin<Box<dyn Future<Output = ServiceResult>>> + Send,
    >;

    pub(super) enum BoxedRunFn {
        Plain(PlainFn),
        WithReload(WithReloadFn),
    }

    struct ScmConfig {
        name: &'static str,
        run_fn: Mutex<Option<BoxedRunFn>>,
    }

    static SCM_CONFIG: OnceLock<ScmConfig> = OnceLock::new();

    pub(super) fn dispatch(name: &'static str, run_fn: BoxedRunFn) -> ServiceResult {
        SCM_CONFIG
            .set(ScmConfig {
                name,
                run_fn: Mutex::new(Some(run_fn)),
            })
            .map_err(|_| "ServiceRunner SCM config already initialised")?;

        windows_service::service_dispatcher::start(name, ffi_service_main)?;
        Ok(())
    }

    windows_service::define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            tracing::error!("service-runner SCM dispatch failed: {e}");
        }
    }

    fn run_service() -> ServiceResult {
        let cfg = SCM_CONFIG
            .get()
            .ok_or("ServiceRunner SCM config missing; dispatch() must run first")?;

        let run_fn = cfg
            .run_fn
            .lock()
            .map_err(|_| "ServiceRunner SCM run_fn mutex poisoned")?
            .take()
            .ok_or("ServiceRunner SCM run_fn already taken (re-entrant dispatch?)")?;

        let with_reload = matches!(run_fn, BoxedRunFn::WithReload(_));
        let token = tokio_util::sync::CancellationToken::new();
        let reload = ReloadSignal::new();

        let token_for_handler = token.clone();
        let reload_for_handler = reload.clone();

        let status_handle = service_control_handler::register(cfg.name, move |evt| match evt {
            ServiceControl::Stop => {
                token_for_handler.cancel();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::ParamChange if with_reload => {
                reload_for_handler.notify();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })?;

        let controls_accepted = if with_reload {
            ServiceControlAccept::STOP | ServiceControlAccept::PARAM_CHANGE
        } else {
            ServiceControlAccept::STOP
        };

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: std::time::Duration::default(),
            process_id: None,
        })?;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let shutdown = Shutdown::from_token(token);
        let result = match run_fn {
            BoxedRunFn::Plain(f) => rt.block_on(f(shutdown)),
            BoxedRunFn::WithReload(f) => rt.block_on(f(shutdown, reload)),
        };

        // Surface the closure's outcome to SCM. Reporting Win32(0) on
        // every stop made failures look like clean shutdowns to ops
        // tooling (services.msc, supervisors). On Err we report a
        // service-specific non-zero code so the SCM stop record
        // matches reality; the closure's error is also logged by
        // service_main() and returned from run_service() for parity
        // with the console path.
        let exit_code = if result.is_ok() {
            ServiceExitCode::Win32(0)
        } else {
            ServiceExitCode::ServiceSpecific(1)
        };

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code,
            checkpoint: 0,
            wait_hint: std::time::Duration::default(),
            process_id: None,
        })?;

        result
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    unsafe_code
)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // Signal-install tests share global per-process signal state; serialize them
    // so concurrent runs do not steal each other's deliveries.
    static SIGNAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn run_invokes_closure_exactly_once_and_returns_ok() {
        let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_closure = Arc::clone(&calls);

        let result = ServiceRunner::new("test-once").run(move |_shutdown| async move {
            calls_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        result.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn run_propagates_closure_error() {
        let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
        let result = ServiceRunner::new("test-err")
            .run(|_shutdown| async move { Err("closure failed".into()) });

        let err = result.unwrap_err();
        assert_eq!(err.to_string(), "closure failed");
    }

    #[test]
    fn run_with_reload_requires_with_reload_flag() {
        let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
        let result =
            ServiceRunner::new("test-reload-flag").run_with_reload(|_s, _r| async move { Ok(()) });

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("with_reload"),
            "error should mention with_reload, got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn sigterm_cancels_shutdown_token() {
        let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
        let observed_cancel = Arc::new(AtomicU32::new(0));
        let observed_for_closure = Arc::clone(&observed_cancel);

        let result = ServiceRunner::new("test-sigterm").run(move |shutdown| async move {
            // Schedule a self-SIGTERM after the closure starts awaiting.
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                // Safety: raise() on the current process is the documented way to
                // self-signal; libc::raise is unsafe only because it touches global
                // process state.
                unsafe {
                    libc::raise(libc::SIGTERM);
                }
            });

            shutdown.cancelled().await;
            observed_for_closure.store(1, Ordering::SeqCst);
            Ok(())
        });

        result.unwrap();
        assert_eq!(observed_cancel.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[test]
    fn sighup_wakes_reload_signal() {
        let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
        let woke = Arc::new(AtomicU32::new(0));
        let woke_for_closure = Arc::clone(&woke);

        let result = ServiceRunner::new("test-sighup")
            .with_reload()
            .run_with_reload(move |shutdown, reload| async move {
                // Self-raise SIGHUP shortly, then SIGTERM to shut down.
                tokio::spawn(async {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    unsafe {
                        libc::raise(libc::SIGHUP);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    unsafe {
                        libc::raise(libc::SIGTERM);
                    }
                });

                loop {
                    tokio::select! {
                        () = reload.recv() => {
                            woke_for_closure.fetch_add(1, Ordering::SeqCst);
                        }
                        () = shutdown.cancelled() => return Ok(()),
                    }
                }
            });

        result.unwrap();
        assert!(
            woke.load(Ordering::SeqCst) >= 1,
            "reload signal should have fired at least once"
        );
    }
}
