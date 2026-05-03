//! BDD World struct and helpers for the rp-plate-solver suite.
//!
//! The world accumulates state across Given / When / Then steps:
//! the spawned wrapper handle, the temp directory holding fixtures and
//! configs, and the most recent HTTP response body / status / timing.
//!
//! All scenarios under `tests/features/` are tagged `@wip` for Phase 3
//! per `docs/plans/rp-plate-solver.md`; the `@wip` filter in
//! `tests/bdd.rs` skips them at runtime so the default suite stays
//! green until Phase 4 wires the wrapper binary.

use bdd_infra::ServiceHandle;
use cucumber::World;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

#[allow(dead_code)]
// Phase 3 stubs touch every field via `todo!()`-bodied
// step definitions but never *use* them at runtime.
// Phase 4 wires the bodies and these allows go away.
#[derive(Debug, Default, World)]
pub struct PlateSolverWorld {
    /// Handle to the spawned `rp-plate-solver` binary. None until a
    /// Given step starts the wrapper. Stopped in the cucumber `after`
    /// hook in `tests/bdd.rs`.
    pub service_handle: Option<ServiceHandle>,

    /// Per-scenario temp dir holding the config file, FITS placeholder,
    /// and any temp-dir copy of `mock_astap`. Dropped at scenario end
    /// (cleans up via `tempfile::TempDir`).
    pub temp_dir: Option<TempDir>,

    /// Path to the `astap_binary_path` the wrapper was started with.
    /// Some scenarios rewrite or delete this between steps (e.g.,
    /// `health.feature`'s "I delete the configured binary").
    pub astap_binary_path: Option<PathBuf>,

    /// Path to the `astap_db_directory` the wrapper was started with.
    /// Same lifecycle as above.
    pub astap_db_directory: Option<PathBuf>,

    /// Path that `mock_astap` writes received argv to when
    /// `MOCK_ASTAP_ARGV_OUT` is set on its env. Read by argv-flow
    /// assertions in `solve_request.feature`.
    pub argv_out_path: Option<PathBuf>,

    /// Result of the most recent HTTP request (status + body).
    pub last_response: Option<HttpResponse>,

    /// Elapsed wall time of the most recent request, used by
    /// `subprocess_supervision.feature`'s timing assertions.
    pub last_response_elapsed: Option<Duration>,

    /// Wrapper stderr after exit — populated by configuration
    /// scenarios that probe non-zero-exit error messages.
    pub last_wrapper_stderr: Option<String>,

    /// Wrapper exit status, when the scenario waited for the wrapper
    /// to exit (vs. starting it and leaving it running).
    pub last_wrapper_exit_code: Option<i32>,
}

#[allow(dead_code)] // Phase 3 stubs; populated and read in Phase 4.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

impl PlateSolverWorld {
    /// Locate the in-tree `mock_astap` binary the same way
    /// `tests/supervision_integration.rs` does: `MOCK_ASTAP_BINARY`
    /// env var (Bazel) → `option_env!("CARGO_BIN_EXE_mock_astap")`
    /// (Cargo). Panics with a diagnostic if neither resolves —
    /// matches the supervision integration test's hard-failure
    /// posture.
    #[allow(dead_code)] // Used by Phase 4 step bodies.
    pub fn mock_astap_path() -> PathBuf {
        if let Ok(p) = std::env::var("MOCK_ASTAP_BINARY") {
            let path = PathBuf::from(p);
            if path.exists() {
                return path;
            }
        }
        if let Some(p) = option_env!("CARGO_BIN_EXE_mock_astap") {
            let path = PathBuf::from(p);
            if path.exists() {
                return path;
            }
        }
        panic!(
            "mock_astap binary not found. Tried MOCK_ASTAP_BINARY env var, then \
             CARGO_BIN_EXE_mock_astap. Run `cargo build --tests -p rp-plate-solver`."
        )
    }
}
