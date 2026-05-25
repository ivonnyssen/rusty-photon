//! Unified service lifecycle for Rusty Photon binaries.
//!
//! Every long-running service in this workspace needs the same three things:
//! a tokio runtime, OS signal handlers for graceful shutdown, and a way to
//! propagate "time to stop" to its workers. This crate is the single
//! workspace-wide replacement for the per-service boilerplate that grew
//! up around those needs.
//!
//! Full design and usage: `docs/crates/rusty-photon-service-lifecycle.md`.
//! Migration plan from the existing per-service shutdown handlers:
//! `docs/plans/archive/service-lifecycle-unification.md` (issue #294).
//!
//! The public surface is small:
//!
//! * [`ServiceRunner`] — builder that owns the tokio runtime, installs signal
//!   handlers (or dispatches to the Windows Service Control Manager when the
//!   `scm` feature is on and `scm_mode(true)` is set), and invokes a user
//!   closure with a [`Shutdown`] handle.
//! * [`Shutdown`] — thin wrapper around [`tokio_util::sync::CancellationToken`].
//!   Hand `shutdown.token()` to spawned subtasks; race `shutdown.cancelled()`
//!   in `tokio::select!`; pass `shutdown.cancelled()` to
//!   `axum::serve(...).with_graceful_shutdown(...)`.
//! * [`ReloadSignal`] — opt-in (via [`ServiceRunner::with_reload`]) reload
//!   notifier, driven by `SIGHUP` on Unix or `ServiceControl::ParamChange` in
//!   SCM mode. On Windows console mode it never fires.
//!
//! ## Minimal example
//!
//! ```no_run
//! use rusty_photon_service_lifecycle::ServiceRunner;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     ServiceRunner::new("example-service").run(|shutdown| async move {
//!         // Race the server against shutdown.
//!         tokio::select! {
//!             _ = std::future::pending::<()>() => {}
//!             _ = shutdown.cancelled() => tracing::debug!("shutdown requested"),
//!         }
//!         Ok(())
//!     })
//! }
//! ```
//!
//! ## Behavioral guarantees
//!
//! * **No panic on signal install failure.** Both `tokio::signal::ctrl_c()`
//!   and `tokio::signal::unix::signal(...)` errors are logged via
//!   `tracing::warn!` and replaced with a never-resolving future. This
//!   matches the pattern established by PR #289.
//! * **Cross-platform.** On Unix: `SIGINT` (Ctrl+C) + `SIGTERM`, plus `SIGHUP`
//!   if `with_reload()` is set. On Windows console: Ctrl+C only. On Windows
//!   SCM mode: `ServiceControl::Stop` + `ServiceControl::ParamChange`.
//! * **Single propagation primitive.** Both the signal-handler task and SCM
//!   control-handler callback feed the same [`CancellationToken`]. Consumers
//!   never need to know which source triggered shutdown.

#![deny(unsafe_code)]

mod reload;
mod runner;
mod shutdown;

pub use reload::ReloadSignal;
pub use runner::ServiceRunner;
pub use shutdown::Shutdown;
