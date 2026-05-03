//! BDD World struct and helpers for the rp-plate-solver suite.
//!
//! The world accumulates state across Given / When / Then steps:
//! the spawned wrapper handle, the temp directory holding fixtures and
//! configs, and the most recent HTTP response body / status / timing.

use bdd_infra::ServiceHandle;
use cucumber::World;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[derive(Debug, Default, World)]
pub struct PlateSolverWorld {
    /// Handle to the spawned `rp-plate-solver` binary. None until a
    /// Given step starts the wrapper. Stopped in the cucumber `after`
    /// hook in `tests/bdd.rs`.
    pub service_handle: Option<ServiceHandle>,

    /// Per-scenario temp dir holding the config file, FITS placeholder,
    /// and any temp-dir copy of `mock_astap`.
    pub temp_dir: Option<TempDir>,

    /// `astap_binary_path` the wrapper was started with. Some scenarios
    /// rewrite or delete this between steps.
    pub astap_binary_path: Option<PathBuf>,

    /// `astap_db_directory` the wrapper was started with.
    pub astap_db_directory: Option<PathBuf>,

    /// FITS path the most recent solve request used.
    pub fits_path: Option<PathBuf>,

    /// Path that `mock_astap` writes received argv to when
    /// `MOCK_ASTAP_ARGV_OUT` is set on its env.
    pub argv_out_path: Option<PathBuf>,

    /// Mode for the next wrapper spawn (passed via `astap_extra_env`).
    pub mock_astap_mode: Option<String>,

    /// Result of the most recent HTTP request (status + body).
    pub last_response: Option<HttpResponse>,

    /// Elapsed wall time of the most recent request.
    pub last_response_elapsed: Option<Duration>,

    /// Wrapper stderr after exit (configuration scenarios that wait
    /// for the wrapper to exit non-zero).
    pub last_wrapper_stderr: Option<String>,

    /// Wrapper exit status when the scenario waited for the wrapper to
    /// exit (vs. starting it and leaving it running).
    pub last_wrapper_exit_code: Option<i32>,

    /// For the Scenario Outline that POSTs with a single hint set,
    /// step state populated by the "with that fits_path and hint X
    /// set to Y" When step.
    pub pending_hint: Option<(String, f64)>,

    /// Concurrent-request timings for the supervision feature.
    pub concurrent_results: Vec<ConcurrentResult>,

    /// Configuration JSON being accumulated by `configuration.feature`'s
    /// composing Given steps. Materialized to disk by the
    /// `When the wrapper starts` step.
    pub pending_config: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ConcurrentResult {
    pub status: u16,
    pub completed_at: Instant,
}

impl PlateSolverWorld {
    /// Locate the in-tree `mock_astap` binary the same way
    /// `tests/supervision_integration.rs` does.
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

    /// Lazily create the per-scenario temp dir.
    pub fn temp_dir_path(&mut self) -> PathBuf {
        if self.temp_dir.is_none() {
            self.temp_dir = Some(TempDir::new().expect("create temp dir"));
        }
        self.temp_dir.as_ref().unwrap().path().to_path_buf()
    }

    /// Wrapper base URL (e.g., `http://127.0.0.1:11131`). Panics if
    /// the wrapper hasn't been started.
    pub fn wrapper_url(&self) -> String {
        let handle = self
            .service_handle
            .as_ref()
            .expect("wrapper not started — Given step missing?");
        format!("http://127.0.0.1:{}", handle.port)
    }

    /// Build a config pointing at `mock_astap` with the configured
    /// mode (if any), write it under temp_dir, and start the wrapper
    /// via `ServiceHandle`. Stores all paths in the world.
    pub async fn start_wrapper_with_mock(&mut self) {
        let mock_path = Self::mock_astap_path();
        let dir = self.temp_dir_path();
        let db_dir = dir.join("db");
        std::fs::create_dir_all(&db_dir).expect("mkdir db");

        let mut extra_env: HashMap<String, String> = HashMap::new();
        if let Some(mode) = self.mock_astap_mode.clone() {
            extra_env.insert("MOCK_ASTAP_MODE".to_string(), mode);
        }
        if let Some(p) = self.argv_out_path.clone() {
            extra_env.insert("MOCK_ASTAP_ARGV_OUT".to_string(), p.display().to_string());
        }

        self.astap_binary_path = Some(mock_path.clone());
        self.astap_db_directory = Some(db_dir.clone());

        let config_path = write_config(&dir, &mock_path, &db_dir, &extra_env);
        let config_str = config_path.to_string_lossy().into_owned();
        let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_str).await;
        self.service_handle = Some(handle);
    }

    /// Variant for `health.feature` scenarios that need to mutate the
    /// configured paths after startup. Copies `mock_astap` into the
    /// temp dir and points the config at the copy.
    pub async fn start_wrapper_with_mock_copy(&mut self) {
        let dir = self.temp_dir_path();
        let mock_src = Self::mock_astap_path();
        let mock_dst = dir.join("mock_astap_copy");
        std::fs::copy(&mock_src, &mock_dst).expect("copy mock_astap");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&mock_dst, std::fs::Permissions::from_mode(0o755))
                .expect("chmod mock_astap copy");
        }
        let db_dir = dir.join("db");
        std::fs::create_dir_all(&db_dir).expect("mkdir db");

        self.astap_binary_path = Some(mock_dst.clone());
        self.astap_db_directory = Some(db_dir.clone());

        let extra_env: HashMap<String, String> = HashMap::new();
        let config_path = write_config(&dir, &mock_dst, &db_dir, &extra_env);
        let config_str = config_path.to_string_lossy().into_owned();
        let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_str).await;
        self.service_handle = Some(handle);
    }

    /// Run the wrapper to completion (rather than leaving it
    /// running). Used by configuration scenarios that assert on
    /// validation-failure exit codes.
    ///
    /// `ServiceHandle` is wrong for this case because it waits for
    /// `bound_addr=` on stdout, which never arrives if the wrapper
    /// exits during config validation. So we replicate `bdd-infra`'s
    /// binary-discovery logic inline and run the binary to completion
    /// via `tokio::process::Command::output()`.
    pub async fn run_wrapper_to_exit(&mut self, config_path: PathBuf) {
        let binary = find_wrapper_binary().expect(
            "rp-plate-solver binary not found. Run `cargo build -p rp-plate-solver` first.",
        );
        let output = tokio::process::Command::new(binary)
            .arg("--config")
            .arg(&config_path)
            .output()
            .await
            .expect("spawn wrapper");
        self.last_wrapper_exit_code = Some(output.status.code().unwrap_or(-1));
        self.last_wrapper_stderr = Some(String::from_utf8_lossy(&output.stderr).into_owned());
    }
}

/// Locate the `rp-plate-solver` binary. Mirrors `bdd_infra::find_binary`
/// (whose impl is private) including the precedence rules the original
/// uses for cross-compile / coverage / sanitizer builds:
///
/// 1. Explicit `RP_PLATE_SOLVER_BINARY` env var.
/// 2. `CARGO_TARGET_DIR` or `CARGO_LLVM_COV_TARGET_DIR` (whichever
///    is set), with `CARGO_BUILD_TARGET` triple subdir prepended when
///    set. When either is set we honor it *exclusively* — falling
///    through to walk-up could silently pick up a stale,
///    non-instrumented binary at `target/debug/<pkg>` and skip
///    coverage data collection.
/// 3. Walk up from cwd looking for `target/debug/<bin>` (and the
///    `CARGO_BUILD_TARGET`-qualified variant).
///
/// `ServiceHandle::start` waits for `bound_addr=` on stdout, which the
/// configuration-error scenarios never reach — that's why
/// configuration scenarios spawn the wrapper themselves via this
/// helper rather than going through `ServiceHandle`.
fn find_wrapper_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RP_PLATE_SOLVER_BINARY") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    let bin_name = if cfg!(target_os = "windows") {
        "rp-plate-solver.exe"
    } else {
        "rp-plate-solver"
    };
    let triple = std::env::var("CARGO_BUILD_TARGET").ok();

    let candidate = |target_dir: &std::path::Path| -> Option<PathBuf> {
        if let Some(triple) = triple.as_deref() {
            let p = target_dir.join(triple).join("debug").join(bin_name);
            if p.exists() {
                return Some(p);
            }
        }
        let p = target_dir.join("debug").join(bin_name);
        if p.exists() {
            return Some(p);
        }
        None
    };

    // Honor CARGO_TARGET_DIR / CARGO_LLVM_COV_TARGET_DIR exclusively
    // when set (matches bdd_infra::find_binary's behavior).
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR")
        .or_else(|| std::env::var_os("CARGO_LLVM_COV_TARGET_DIR"))
    {
        return candidate(std::path::Path::new(&dir));
    }

    // Walk up from cwd looking for target/debug/<bin>.
    let mut cur = std::env::current_dir().ok()?;
    loop {
        if let Some(p) = candidate(&cur.join("target")) {
            return Some(p);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Write a JSON config to `dir/config.json` and return the path.
fn write_config(
    dir: &std::path::Path,
    binary_path: &std::path::Path,
    db_directory: &std::path::Path,
    extra_env: &HashMap<String, String>,
) -> PathBuf {
    let body = serde_json::json!({
        "bind_address": "127.0.0.1",
        "port": 0,  // OS picks a free port; ServiceHandle parses it from stdout
        "astap_binary_path": binary_path.to_string_lossy(),
        "astap_db_directory": db_directory.to_string_lossy(),
        "astap_extra_env": extra_env,
    })
    .to_string();
    let p = dir.join("config.json");
    std::fs::write(&p, body).expect("write config");
    p
}
