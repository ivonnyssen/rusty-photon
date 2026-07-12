use std::future::Future;

use crate::{ReloadSignal, Shutdown};

/// Result type every service binary's `main` returns.
///
/// The error side is [`color_eyre::Report`], so a startup failure or fatal
/// exit escaping `main` prints a readable multi-line report with the full
/// `source()` chain instead of a single-line `Debug` dump.
///
/// Per ADR-011, this crate is the *only* place `color-eyre` enters the
/// workspace: services name this alias (and [`Report`](color_eyre::Report))
/// but never construct ad-hoc `eyre!` errors — errors stay `thiserror`-typed
/// everywhere below the binary boundary.
pub type ServiceResult = Result<(), color_eyre::Report>;

/// Boxed error the closures passed to [`ServiceRunner::run`] /
/// [`ServiceRunner::run_with_reload`] return.
///
/// Any typed `thiserror` error converts into it via `?`, as do plain string
/// errors (`"...".into()`). The runner converts it into the
/// [`color_eyre::Report`] that `main` returns, preserving the full `source()`
/// chain (see [`report_from_boxed`]). `Send + Sync` is required for that
/// conversion — `Report` only wraps thread-safe errors.
pub type RunError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Result type the run closures return. Converted to [`ServiceResult`] at
/// the runner's boundary.
pub type RunResult = Result<(), RunError>;

/// Adapter that gives an already-boxed [`RunError`] the Sized
/// `std::error::Error` impl [`color_eyre::Report::new`] needs, delegating
/// `Display`/`Debug`/`source()` to the inner error so the report renders the
/// original message and chain unchanged.
struct BoxedRunError(RunError);

impl std::fmt::Display for BoxedRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::fmt::Debug for BoxedRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

impl std::error::Error for BoxedRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// Convert a boxed [`RunError`] into the [`color_eyre::Report`] that `main`
/// returns, preserving the `source()` chain.
///
/// The runner applies this at its own boundary; `?` cannot do it implicitly
/// because `Report` has no `From` impl for boxed trait objects. Public for
/// the rare fallible step that must run *before* [`ServiceRunner::run`]
/// (config-path resolution, identity minting) when the helper returns a
/// boxed error. It only converts an error that already exists — it is not a
/// substitute for `eyre!`-style ad-hoc error construction, which stays out
/// of service code per ADR-011.
///
/// `#[track_caller]` so the report's `Location:` section names the call
/// site (the service's `main`, or the runner boundary) rather than this
/// function.
#[track_caller]
pub fn report_from_boxed(e: RunError) -> color_eyre::Report {
    color_eyre::Report::new(BoxedRunError(e))
}

/// Install the `color-eyre` error/panic hooks exactly once per process.
///
/// `color_eyre::install()` is process-global and errors on a second call;
/// the `Once` guard makes repeated [`ServiceRunner`] invocations (the crate's
/// own tests run many per process) safe. An install failure is logged rather
/// than propagated — the service must still start even if another component
/// already claimed the hooks.
///
/// Called from both [`init_tracing`](crate::init_tracing) (so failures and
/// panics *before* the runner — config load, identity minting — render the
/// same formatted report) and the runner (so a service skipping
/// `init_tracing` still gets the hooks).
pub(crate) fn install_error_reporting() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        if let Err(e) = color_eyre::install() {
            tracing::warn!("failed to install color-eyre error/panic hooks: {e}");
        }
    });
}

/// Builder for a Rusty Photon service binary's lifecycle.
///
/// Owns the tokio runtime, installs OS signal handlers (or dispatches to the
/// Windows Service Control Manager when `scm` feature + [`Self::scm_mode`]
/// are enabled), and invokes the user closure with a [`Shutdown`] handle.
///
/// ## Usage
///
/// ```no_run
/// use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
///
/// fn main() -> ServiceResult {
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
/// use rusty_photon_service_lifecycle::{ServiceResult, ServiceRunner};
///
/// fn main() -> ServiceResult {
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
    /// Also installs the process-global `color-eyre` error/panic hooks
    /// (once per process), so every service gets formatted panic reports —
    /// with span context when [`init_tracing`](crate::init_tracing) is in
    /// use — without any per-service wiring.
    ///
    /// Returns the error from `run_fn`, if any. Signal-install failures are
    /// logged via `tracing::warn!` rather than returned.
    pub fn run<F, Fut>(self, run_fn: F) -> ServiceResult
    where
        F: FnOnce(Shutdown) -> Fut + Send + 'static,
        Fut: Future<Output = RunResult> + 'static,
    {
        install_error_reporting();

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
        Fut: Future<Output = RunResult> + 'static,
    {
        install_error_reporting();

        if !self.reload {
            return Err(color_eyre::eyre::eyre!(
                "ServiceRunner::run_with_reload requires .with_reload() on the builder"
            ));
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
    Fut: Future<Output = RunResult>,
{
    let rt = build_runtime()?;
    let token = tokio_util::sync::CancellationToken::new();
    rt.spawn(watch_shutdown_signals(name, token.clone()));
    rt.block_on(run_fn(Shutdown::from_token(token)))
        .map_err(report_from_boxed)
}

fn run_console_with_reload<F, Fut>(name: &'static str, run_fn: F) -> ServiceResult
where
    F: FnOnce(Shutdown, ReloadSignal) -> Fut + Send + 'static,
    Fut: Future<Output = RunResult>,
{
    let rt = build_runtime()?;
    let token = tokio_util::sync::CancellationToken::new();
    let reload = ReloadSignal::new();
    rt.spawn(watch_shutdown_signals(name, token.clone()));
    #[cfg(unix)]
    rt.spawn(watch_reload_signal(reload.clone()));
    rt.block_on(run_fn(Shutdown::from_token(token), reload))
        .map_err(report_from_boxed)
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

    type PlainFn = Box<dyn FnOnce(Shutdown) -> Pin<Box<dyn Future<Output = RunResult>>> + Send>;
    type WithReloadFn =
        Box<dyn FnOnce(Shutdown, ReloadSignal) -> Pin<Box<dyn Future<Output = RunResult>>> + Send>;

    pub(super) enum BoxedRunFn {
        Plain(PlainFn),
        WithReload(WithReloadFn),
    }

    struct ScmConfig {
        name: &'static str,
        run_fn: Mutex<Option<BoxedRunFn>>,
    }

    static SCM_CONFIG: OnceLock<ScmConfig> = OnceLock::new();

    /// The run closure's error, captured by the SCM service thread
    /// ([`run_service`]) so [`dispatch`] can return it from the main thread
    /// once `service_dispatcher::start` unblocks. Keeps `ServiceRunner::run`'s
    /// "returns the error from `run_fn`" contract identical in SCM and console
    /// modes (non-zero process exit code, `Report` rendered from `main`).
    static SCM_RUN_ERROR: Mutex<Option<RunError>> = Mutex::new(None);

    pub(super) fn dispatch(name: &'static str, run_fn: BoxedRunFn) -> ServiceResult {
        SCM_CONFIG
            .set(ScmConfig {
                name,
                run_fn: Mutex::new(Some(run_fn)),
            })
            .map_err(|_| color_eyre::eyre::eyre!("ServiceRunner SCM config already initialised"))?;

        windows_service::service_dispatcher::start(name, ffi_service_main)?;

        // The service thread stores the closure's error rather than
        // returning it through the `extern "system"` boundary; surface it
        // here so SCM mode matches the console path's contract.
        if let Some(e) = SCM_RUN_ERROR
            .lock()
            .map_err(|_| color_eyre::eyre::eyre!("ServiceRunner SCM run-error mutex poisoned"))?
            .take()
        {
            let report = report_from_boxed(e);
            // Returning the Report renders it to stderr from `main` — a dead
            // handle under SCM. Emit the full rendered source() chain through
            // tracing too, so it lands in the rolling log file while the
            // service's TracingGuard is still held (it flushes on exit).
            tracing::error!("{name}: service run failed:\n{report:?}");
            return Err(report);
        }
        Ok(())
    }

    windows_service::define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            tracing::error!("service-runner SCM dispatch failed: {e}");
        }
    }

    fn run_service() -> ServiceResult {
        let cfg = SCM_CONFIG.get().ok_or_else(|| {
            color_eyre::eyre::eyre!("ServiceRunner SCM config missing; dispatch() must run first")
        })?;

        let run_fn = cfg
            .run_fn
            .lock()
            .map_err(|_| color_eyre::eyre::eyre!("ServiceRunner SCM run_fn mutex poisoned"))?
            .take()
            .ok_or_else(|| {
                color_eyre::eyre::eyre!(
                    "ServiceRunner SCM run_fn already taken (re-entrant dispatch?)"
                )
            })?;

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

        // Surface the closure's outcome to SCM. This is the failure-visibility
        // mechanism ADR-015 / windows-packaging W1 pins: on Err we still
        // report SERVICE_STOPPED, but with a non-zero exit code —
        // dwWin32ExitCode = ERROR_SERVICE_SPECIFIC_ERROR with
        // dwServiceSpecificExitCode = 1. The installer configures restart
        // failure actions *and* sets SERVICE_CONFIG_FAILURE_ACTIONS_FLAG
        // (failure actions on non-crash failures), so SCM counts a stop with
        // a non-zero exit code as a failure and runs the configured restart —
        // restoring the systemd `Restart=on-failure` contract the serial
        // drivers' eager-validation exits rely on. Reporting Win32(0) on
        // every stop would make failures look like clean shutdowns (no
        // restart, and ops tooling like services.msc shown a clean stop).
        let run_error = result.err();
        if let Some(e) = &run_error {
            // Under SCM the rolling log file is often the only place this
            // failure is visible besides the SCM stop record.
            tracing::error!("{}: service run failed: {e}", cfg.name);
        }
        let exit_code = if run_error.is_none() {
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

        // Stash the closure's error for dispatch() (on the main thread) to
        // return once the dispatcher unblocks, mirroring the console path.
        if let Some(e) = run_error {
            *SCM_RUN_ERROR.lock().map_err(|_| {
                color_eyre::eyre::eyre!("ServiceRunner SCM run-error mutex poisoned")
            })? = Some(e);
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
    fn repeated_runs_install_error_reporting_without_error() {
        // `color_eyre::install()` errors on a second call; the Once guard in
        // `install_error_reporting` must make back-to-back runs clean.
        let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
        for _ in 0..2 {
            ServiceRunner::new("test-install-once")
                .run(|_shutdown| async move { Ok(()) })
                .unwrap();
        }
    }

    #[derive(Debug)]
    struct RootCause;

    impl std::fmt::Display for RootCause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "port 11119 already in use")
        }
    }

    impl std::error::Error for RootCause {}

    #[derive(Debug)]
    struct StartupError(RootCause);

    impl std::fmt::Display for StartupError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "server startup failed")
        }
    }

    impl std::error::Error for StartupError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn run_error_renders_as_multi_line_report_with_source_chain() {
        // The whole point of returning `Report` from `main`: a typed error's
        // full `source()` chain must render over multiple lines, not as a
        // single `Debug` line.
        let _guard = SIGNAL_TEST_LOCK.lock().unwrap();
        let result = ServiceRunner::new("test-report")
            .run(|_shutdown| async move { Err(Box::new(StartupError(RootCause)) as RunError) });

        let rendered = format!("{:?}", result.unwrap_err());
        assert!(
            rendered.contains("server startup failed"),
            "report should contain the outer error, got:\n{rendered}"
        );
        assert!(
            rendered.contains("port 11119 already in use"),
            "report should contain the root cause, got:\n{rendered}"
        );
        assert!(
            rendered.lines().count() > 1,
            "report should span multiple lines, got:\n{rendered}"
        );
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
