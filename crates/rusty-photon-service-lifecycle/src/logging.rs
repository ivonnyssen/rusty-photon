//! Tracing/logging initialization shared by every Rusty Photon service binary.
//!
//! All service binaries log to **stderr**, never stdout. Stdout is reserved for
//! the machine-readable `bound_addr=<host>:<port>` line that `bdd-infra`'s
//! `ServiceHandle` parses to discover a test-spawned service's port (and, more
//! generally, for any structured handshake a supervisor might read). Routing
//! logs to stdout meant the BDD harness's stdout-drain task silently swallowed
//! every line, and — because the drain's read end is closed during shutdown —
//! produced a flood of `[tracing-subscriber] Unable to write an event ... Broken
//! pipe` noise in CI. Stderr is the conventional place for diagnostics and is
//! inherited by child processes by default, so logs flow to the same place as
//! the test binary's own output without extra wiring.
//!
//! Filtering follows `RUST_LOG` when set, otherwise falls back to the level the
//! binary passes in (typically its `--log-level` flag, defaulting to `info`).
//! This is the same `stderr` + `EnvFilter` pattern `rp` and `plate-solver` used
//! inline, hoisted here so all services share one implementation.

use tracing::Level;
use tracing_subscriber::filter::{EnvFilter, LevelFilter};

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

/// Initialize the global tracing subscriber for a service binary.
///
/// Logs are written to stderr (see the module docs for why), filtered by
/// `RUST_LOG` if set, otherwise at `default_level`. Idempotent: a redundant call
/// (e.g. from a test that already installed a subscriber) is ignored rather than
/// panicking, matching [`try_init`](tracing_subscriber::fmt::SubscriberBuilder::try_init)
/// semantics.
pub fn init_tracing(default_level: Level) {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(build_env_filter(default_level))
        .try_init();
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
    fn init_tracing_is_idempotent() {
        // Two calls must not panic — the second is swallowed by `try_init`.
        init_tracing(Level::INFO);
        init_tracing(Level::INFO);
    }
}
