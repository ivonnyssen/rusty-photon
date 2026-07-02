//! Tracing/logging initialization shared by every Rusty Photon service binary.
//!
//! All service binaries log to **stderr**, never stdout, for two standing
//! reasons:
//!
//! 1. Stdout is reserved for the machine-readable `bound_addr=<host>:<port>`
//!    line that `bdd-infra`'s `ServiceHandle` parses to discover a test-spawned
//!    service's port (and, more generally, for any structured handshake a
//!    supervisor might read).
//! 2. The BDD harness drains and *discards* a service's stdout, so anything
//!    logged there is invisible during tests. Stderr is the conventional place
//!    for diagnostics and is inherited by child processes by default, so logs
//!    flow to the same destination as the test binary's own output without extra
//!    wiring.
//!
//! Historically, logging to stdout also produced a flood of
//! `[tracing-subscriber] Unable to write an event ... Broken pipe` noise in CI:
//! the harness aborted its stdout-drain task before the child exited, closing
//! the pipe's read end while the child was still writing shutdown-path logs.
//! That harness defect has since been fixed (the drain stays open until the
//! child exits), so stderr logging is no longer *required* to avoid the EPIPE —
//! but it remains the right destination for reasons 1 and 2.
//!
//! Filtering follows `RUST_LOG` when set, otherwise falls back to the level the
//! binary passes in (typically its `--log-level` flag, defaulting to `info`).
//! This is the same `stderr` + `EnvFilter` pattern `rp` and `plate-solver` used
//! inline, hoisted here so all services share one implementation.
//!
//! The subscriber additionally carries a [`tracing_error::ErrorLayer`] so
//! `SpanTrace::capture()` sees the live span stack. This is what lets the
//! `color-eyre` panic hook installed by [`ServiceRunner`](crate::ServiceRunner)
//! include the active span context in panic reports (ADR-011); without it the
//! report would print "span trace disabled".

use tracing::Level;
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Build the [`EnvFilter`] for [`init_tracing`]: honor `RUST_LOG` when present
/// and parseable, otherwise fall back to `default_level`.
///
/// Split out from [`init_tracing`] so the filter logic is unit-testable without
/// installing a process-global subscriber. `RUST_LOG` takes precedence over
/// `default_level`, matching the convention `rp`/`plate-solver` established.
fn build_env_filter(default_level: Level) -> EnvFilter {
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(LevelFilter::from_level(default_level).to_string()))
}

/// Build the subscriber [`init_tracing`] installs: stderr fmt output filtered
/// by [`build_env_filter`], plus a [`tracing_error::ErrorLayer`] so
/// `SpanTrace::capture()` (used by the `color-eyre` panic hook) observes the
/// live span stack.
///
/// Split out from [`init_tracing`] so the composition is unit-testable via
/// `tracing::subscriber::with_default` without touching the process-global
/// default subscriber.
fn build_subscriber(default_level: Level) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::registry()
        .with(build_env_filter(default_level))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(tracing_error::ErrorLayer::default())
}

/// Initialize the global tracing subscriber for a service binary.
///
/// Logs are written to stderr (see the module docs for why), filtered by
/// `RUST_LOG` if set, otherwise at `default_level`. The subscriber carries a
/// [`tracing_error::ErrorLayer`] so panic reports include span context.
/// Idempotent: a redundant call (e.g. from a test that already installed a
/// subscriber) is ignored rather than panicking, matching
/// [`try_init`](tracing_subscriber::util::SubscriberInitExt::try_init)
/// semantics.
pub fn init_tracing(default_level: Level) {
    // Install the color-eyre hooks here as well as in the runner: services
    // call `init_tracing` before any fallible pre-runner work (config load,
    // identity minting), so errors and panics on that path render as
    // formatted reports too. Idempotent (`Once`-guarded).
    crate::runner::install_error_reporting();
    let _ = build_subscriber(default_level).try_init();
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // `RUST_LOG` is process-global; serialize the tests that read or write it so
    // they don't observe each other's mutations under `cargo test` (which runs
    // tests as threads in one process for the coverage job).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn falls_back_to_default_level_when_rust_log_unset() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var_os("RUST_LOG");
        std::env::remove_var("RUST_LOG");

        let hint = build_env_filter(Level::DEBUG).max_level_hint();

        if let Some(v) = saved {
            std::env::set_var("RUST_LOG", v);
        }
        assert_eq!(hint, Some(LevelFilter::DEBUG));
    }

    #[test]
    fn rust_log_overrides_default_level() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var_os("RUST_LOG");
        std::env::set_var("RUST_LOG", "trace");

        let hint = build_env_filter(Level::INFO).max_level_hint();

        match saved {
            Some(v) => std::env::set_var("RUST_LOG", v),
            None => std::env::remove_var("RUST_LOG"),
        }
        assert_eq!(hint, Some(LevelFilter::TRACE));
    }

    #[test]
    fn subscriber_captures_span_trace_via_error_layer() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var_os("RUST_LOG");
        std::env::remove_var("RUST_LOG");

        let subscriber = build_subscriber(Level::INFO);
        let status = tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("capture_probe");
            let _entered = span.enter();
            tracing_error::SpanTrace::capture().status()
        });

        if let Some(v) = saved {
            std::env::set_var("RUST_LOG", v);
        }
        assert_eq!(status, tracing_error::SpanTraceStatus::CAPTURED);
    }

    #[test]
    fn init_tracing_is_idempotent() {
        // Two calls must not panic — the second is swallowed by `try_init`.
        init_tracing(Level::INFO);
        init_tracing(Level::INFO);
    }
}
