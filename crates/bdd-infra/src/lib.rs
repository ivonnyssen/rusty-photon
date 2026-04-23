#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Shared BDD test infrastructure for rusty-photon services.
//!
//! Provides [`ServiceHandle`] for spawning, managing, and stopping service
//! binaries during BDD and integration tests.
//!
//! # Binary discovery
//!
//! BDD tests require a pre-built service binary. Discovery order:
//!
//! 1. The conventional env var `{PACKAGE_UPPER_SNAKE}_BINARY`
//!    (e.g. `RP_BINARY` for `rp`, `PPBA_DRIVER_BINARY` for `ppba-driver`).
//!    Bazel sets this; Cargo tests can too for explicit overrides.
//! 2. `$CARGO_TARGET_DIR/debug/<pkg>` (or `$CARGO_LLVM_COV_TARGET_DIR/debug/<pkg>`
//!    under `cargo llvm-cov`) when either env var is set. If `CARGO_BUILD_TARGET`
//!    is also set, the triple segment is inserted: `.../<triple>/debug/<pkg>`.
//!    When one of these env vars is set we look *only* there — falling through
//!    to the ancestor walk below could silently pick up a stale, non-instrumented
//!    binary and skip coverage data collection.
//! 3. Walking up from the current directory looking for `target/debug/<pkg>`.
//!    `cargo test -p <pkg>` runs tests with the cwd at the package dir, so
//!    the workspace `target/` is typically one level up.
//!
//! If the binary is not found, the spawn call panics with a diagnostic.
//! Services with feature-gated mock hardware (`ppba-driver`, `qhy-focuser`)
//! must be built with `--all-features` — which is what CI does and what the
//! Bazel `*_mock` binaries encode.
//!
//! # `rp-harness` feature
//!
//! Enabling the `rp-harness` cargo feature exposes the [`rp_harness`] module
//! with higher-level helpers for tests that spawn rp alongside OmniSim and/or
//! an orchestrator plugin: `OmniSimHandle`, `RpConfigBuilder`, `start_rp`,
//! `WebhookReceiver`, `TestOrchestrator`, and `McpTestClient`. Services whose
//! tests only need `ServiceHandle` should leave the feature off so they don't
//! pull in axum, reqwest, or rmcp transitively.
//!
//! # Usage
//!
//! ```rust,ignore
//! use bdd_infra::ServiceHandle;
//!
//! let handle = ServiceHandle::start(
//!     env!("CARGO_PKG_NAME"),
//!     "path/to/config.json",
//! ).await;
//!
//! // handle.port, handle.base_url are available
//! // ...
//! handle.stop().await;
//! ```
//!
//! # Miri compatibility
//!
//! BDD tests that spawn child processes cannot run under Miri (`pidfd_spawnp`
//! is unsupported). Use the [`bdd_main!`] macro in your `bdd.rs` entry point
//! to automatically skip the test under Miri:
//!
//! ```rust,ignore
//! bdd_infra::bdd_main! {
//!     use cucumber::World as _;
//!     use world::MyWorld;
//!
//!     MyWorld::cucumber()
//!         .run_and_exit("tests/features")
//!         .await;
//! }
//! ```

/// Entry-point macro for BDD tests that spawn child processes.
///
/// Under Miri the macro expands to an empty `fn main() {}`, because Miri does
/// not support `pidfd_spawnp` and other process-spawning FFI. Under normal
/// compilation it expands to `#[tokio::main] async fn main() { ... }`.
///
/// If `BDD_PACKAGE_DIR` is set in the environment, the macro chdirs there
/// before running the body. This lets Bazel run BDD tests where the cwd is
/// the runfiles tree rather than the package directory so that relative
/// paths like `"tests/features"` and `"./Cargo.toml"` behave the same way
/// they do under `cargo test`. Any `*_BINARY` env vars that hold relative
/// paths are rewritten to absolute paths before chdir so binary discovery
/// still resolves against the runfiles root.
#[macro_export]
macro_rules! bdd_main {
    ($($body:tt)*) => {
        #[cfg(miri)]
        fn main() {}

        #[cfg(not(miri))]
        #[tokio::main]
        async fn main() {
            $crate::__bdd_bazel_chdir();
            $($body)*
        }
    };
}

#[doc(hidden)]
pub fn __bdd_bazel_chdir() {
    let Ok(dir) = std::env::var("BDD_PACKAGE_DIR") else {
        return;
    };
    let cwd = std::env::current_dir().expect("bdd_main: current_dir");
    let to_absolutize: Vec<(String, String)> = std::env::vars()
        .filter(|(k, v)| k.ends_with("_BINARY") && std::path::Path::new(v).is_relative())
        .collect();
    for (k, v) in to_absolutize {
        std::env::set_var(&k, cwd.join(v));
    }
    std::env::set_current_dir(&dir).unwrap_or_else(|e| panic!("bdd_main: chdir to {}: {}", dir, e));
}

#[cfg(feature = "rp-harness")]
pub mod rp_harness;

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::debug;

/// Derive the conventional env var name for a package's binary override.
///
/// `ppba-driver` → `PPBA_DRIVER_BINARY`, `rp` → `RP_BINARY`, and so on.
fn binary_env_var(package_name: &str) -> String {
    format!("{}_BINARY", package_name.to_uppercase().replace('-', "_"))
}

/// Handle to a running service process.
///
/// Manages the full lifecycle: binary discovery, spawning with stdout capture,
/// port parsing, graceful shutdown signaling, and stdout draining.
///
/// On [`Drop`], sends a best-effort graceful-shutdown signal (SIGTERM on Unix,
/// `CTRL_BREAK_EVENT` on Windows) before the child handle is dropped. Callers
/// should use [`stop`](ServiceHandle::stop) when they need an explicit
/// graceful shutdown path, because dropping the handle may still force the
/// process to terminate if it has not already exited.
#[derive(Debug)]
pub struct ServiceHandle {
    child: Option<tokio::process::Child>,
    /// The port the service bound to (parsed from stdout).
    pub port: u16,
    /// The base URL of the running service (e.g., `http://127.0.0.1:12345`).
    pub base_url: String,
    stdout_drain: Option<tokio::task::JoinHandle<()>>,
    /// Service name (for log/error messages).
    name: String,
}

impl ServiceHandle {
    /// Start a service binary with the given config file.
    ///
    /// # Arguments
    ///
    /// * `package_name` — pass `env!("CARGO_PKG_NAME")` from the calling crate
    /// * `config_path` — path to the service config file (typically a temp file)
    ///
    /// # Binary discovery
    ///
    /// See the module-level docs. Panics with a clear diagnostic if the binary
    /// is not found — BDD binaries must be pre-built (e.g.
    /// `cargo build --all-features --all-targets`).
    pub async fn start(package_name: &str, config_path: &str) -> Self {
        let binary = require_binary(package_name);

        let mut child = spawn_process(&binary, package_name, config_path);

        let stdout = child
            .stdout
            .take()
            .unwrap_or_else(|| panic!("failed to capture {} stdout", package_name));
        let (port, stdout_drain) = parse_bound_port(stdout)
            .await
            .unwrap_or_else(|| panic!("failed to parse bound port from {} output", package_name));

        Self {
            child: Some(child),
            port,
            base_url: format!("http://127.0.0.1:{}", port),
            stdout_drain: Some(stdout_drain),
            name: package_name.to_string(),
        }
    }

    /// Try to start the service, returning an error instead of panicking on failure.
    ///
    /// Times out after 10 seconds if the service does not print its bound address.
    /// Still panics if the binary itself cannot be located — that's a setup
    /// error, not a runtime condition to recover from.
    pub async fn try_start(package_name: &str, config_path: &str) -> Result<Self, String> {
        let binary = require_binary(package_name);

        let mut child = spawn_process(&binary, package_name, config_path);

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("failed to capture {} stdout", package_name))?;

        match tokio::time::timeout(Duration::from_secs(30), parse_bound_port(stdout)).await {
            Ok(Some((port, stdout_drain))) => Ok(Self {
                child: Some(child),
                port,
                base_url: format!("http://127.0.0.1:{}", port),
                stdout_drain: Some(stdout_drain),
                name: package_name.to_string(),
            }),
            Ok(None) => {
                let status = child.wait().await;
                Err(format!(
                    "{} exited without binding: {:?}",
                    package_name, status
                ))
            }
            Err(_) => {
                let _ = child.kill().await;
                Err(format!("timeout waiting for {} to bind", package_name))
            }
        }
    }

    /// Returns `true` if the service process is currently running.
    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    /// Stop the service gracefully via SIGTERM, falling back to SIGKILL after 5 seconds.
    ///
    /// Graceful shutdown allows the process to flush coverage data (profraw files).
    pub async fn stop(&mut self) {
        if let Some(handle) = self.stdout_drain.take() {
            handle.abort();
        }
        if let Some(mut child) = self.child.take() {
            if let Some(pid) = child.id() {
                send_sigterm(pid);

                match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                    Ok(_) => (),
                    Err(_) => {
                        debug!("{} did not exit after SIGTERM, sending SIGKILL", self.name);
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }
                }
            } else {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.stdout_drain.take() {
            handle.abort();
        }
        if let Some(ref mut child) = self.child {
            if let Some(pid) = child.id() {
                send_sigterm(pid);
            } else {
                let _ = child.start_kill();
            }
        }
    }
}

/// Run a service binary once with the given arguments and wait for it to exit.
///
/// Uses the same binary discovery as [`ServiceHandle::start`]. Panics if the
/// binary cannot be found.
///
/// Use this for one-shot commands like `rp init-tls` that are not
/// long-running servers. When `stdin_data` is `Some`, the data is piped to the
/// process's stdin.
pub fn run_once(
    package_name: &str,
    args: &[&str],
    stdin_data: Option<&[u8]>,
) -> std::process::Output {
    let binary = require_binary(package_name);
    debug!(binary = %binary, "running {} from pre-built binary", package_name);

    let mut cmd = std::process::Command::new(&binary);
    cmd.args(args);

    if let Some(data) = stdin_data {
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn {}: {}", package_name, e));

        use std::io::Write;
        if let Some(ref mut stdin) = child.stdin {
            stdin
                .write_all(data)
                .unwrap_or_else(|e| panic!("failed to write stdin for {}: {}", package_name, e));
        }
        drop(child.stdin.take());

        child
            .wait_with_output()
            .unwrap_or_else(|e| panic!("failed to wait on {}: {}", package_name, e))
    } else {
        cmd.output()
            .unwrap_or_else(|e| panic!("failed to run {}: {}", package_name, e))
    }
}

/// Find a pre-built service binary, or return `None`.
///
/// Discovery order:
/// 1. The conventional env var `{PACKAGE_UPPER_SNAKE}_BINARY`
///    (e.g., `FILEMONITOR_BINARY=/path/to/bin`).
/// 2. `$CARGO_TARGET_DIR/debug/<pkg>` (or `$CARGO_TARGET_DIR/$CARGO_BUILD_TARGET/debug/<pkg>`
///    when the latter is set). `CARGO_LLVM_COV_TARGET_DIR` is also honored.
/// 3. Walking up from the current directory, probe `<ancestor>/target/debug/<pkg>` (and
///    the `CARGO_BUILD_TARGET`-qualified variant). Cargo's `cargo test -p <pkg>` sets
///    the cwd to the package dir; the workspace `target/` is then one level up.
fn find_binary(package_name: &str) -> Option<String> {
    if let Ok(path) = std::env::var(binary_env_var(package_name)) {
        return Some(path);
    }

    let binary_name = if cfg!(target_os = "windows") {
        format!("{}.exe", package_name)
    } else {
        package_name.to_string()
    };
    let triple = std::env::var("CARGO_BUILD_TARGET").ok();

    let candidate = |target_dir: &std::path::Path| -> Option<String> {
        if let Some(triple) = triple.as_deref() {
            let path = target_dir.join(triple).join("debug").join(&binary_name);
            if path.exists() {
                return Some(path.to_string_lossy().into_owned());
            }
        }
        let path = target_dir.join("debug").join(&binary_name);
        if path.exists() {
            return Some(path.to_string_lossy().into_owned());
        }
        None
    };

    // When `CARGO_TARGET_DIR` (or `CARGO_LLVM_COV_TARGET_DIR`) is set, honor
    // it exclusively. Walking up afterwards could silently pick up a stale
    // non-instrumented binary at `target/debug/<pkg>` and skip coverage
    // data collection for it.
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR")
        .or_else(|| std::env::var_os("CARGO_LLVM_COV_TARGET_DIR"))
    {
        return candidate(std::path::Path::new(&dir));
    }

    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors() {
            if let Some(path) = candidate(&ancestor.join("target")) {
                return Some(path);
            }
        }
    }

    None
}

/// [`find_binary`] or panic with a diagnostic pointing the user at the fix.
fn require_binary(package_name: &str) -> String {
    find_binary(package_name).unwrap_or_else(|| {
        panic!(
            "bdd-infra: binary for package `{pkg}` not found. \
             BDD tests require a pre-built binary — run \
             `cargo build -p {pkg} --all-features` (or `cargo build --all-features --all-targets` \
             for the whole workspace), or set `{env}` to an explicit binary path.",
            pkg = package_name,
            env = binary_env_var(package_name),
        )
    })
}

/// Windows process creation flag: place the child in its own process group so
/// that `CTRL_BREAK_EVENT` can target it without affecting the test runner.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Spawn the service process from a pre-built binary.
///
/// On Windows the child is spawned with [`CREATE_NEW_PROCESS_GROUP`] so that
/// [`send_sigterm`] can deliver `CTRL_BREAK_EVENT` only to the child's group
/// without affecting the test runner.
fn spawn_process(binary: &str, package_name: &str, config_path: &str) -> tokio::process::Child {
    debug!(binary = %binary, "starting {} from pre-built binary", package_name);
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(["--config", config_path])
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().unwrap_or_else(|e| {
        panic!(
            "failed to start {} binary '{}': {}",
            package_name, binary, e
        )
    })
}

/// Parse the bound port from a service's stdout.
///
/// Scans each line for `bound_addr=<host>:<port>` and extracts the port.
/// After finding it, spawns a background task to drain remaining stdout so
/// the service process never blocks on a full pipe buffer.
///
/// This is a universal parser — it works regardless of what human-readable
/// text precedes `bound_addr=` in the output line.
pub async fn parse_bound_port(
    stdout: tokio::process::ChildStdout,
) -> Option<(u16, tokio::task::JoinHandle<()>)> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    while reader.read_line(&mut line).await.ok()? > 0 {
        if let Some(idx) = line.find("bound_addr=") {
            let addr_str = &line[idx + "bound_addr=".len()..];
            let addr_str = addr_str.trim();
            if let Some(port_str) = addr_str.split(':').next_back() {
                if let Ok(port) = port_str.parse::<u16>() {
                    let drain_handle = tokio::spawn(async move {
                        let mut buf = String::new();
                        while reader.read_line(&mut buf).await.unwrap_or(0) > 0 {
                            buf.clear();
                        }
                    });
                    return Some((port, drain_handle));
                }
            }
        }
        line.clear();
    }
    None
}

/// Send a graceful-shutdown signal to a process.
///
/// * **Unix** — sends `SIGTERM`.
/// * **Windows** — sends `CTRL_BREAK_EVENT` via `GenerateConsoleCtrlEvent`.
///   The child must have been spawned with `CREATE_NEW_PROCESS_GROUP` so the
///   event targets only its process group (see [`spawn_process`]).
fn send_sigterm(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: libc::kill with a valid pid and SIGTERM is safe.
        let ret = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if ret != 0 {
            debug!(
                "failed to send SIGTERM to pid {}: {}",
                pid,
                std::io::Error::last_os_error()
            );
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: GenerateConsoleCtrlEvent with CTRL_BREAK_EVENT and a valid
        // process-group id is the documented way to request graceful shutdown
        // of a console process on Windows.
        #[allow(non_snake_case)]
        extern "system" {
            fn GenerateConsoleCtrlEvent(dw_ctrl_event: u32, dw_process_group_id: u32) -> i32;
        }
        const CTRL_BREAK_EVENT: u32 = 1;
        let ret = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) };
        if ret == 0 {
            debug!(
                "failed to send CTRL_BREAK_EVENT to process group {}: {}",
                pid,
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// Guard for tests that call `set_current_dir`. cargo-nextest runs each
    /// test in its own process so this is a no-op there, but `cargo test`
    /// (used by the coverage job) runs tests as threads in a single process.
    /// The mutex serializes cwd-changing tests so they don't stomp each other.
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // -----------------------------------------------------------------------
    // parse_bound_port tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_parse_bound_port_alpaca_prefix() {
        let mut child = tokio::process::Command::new("echo")
            .arg("Bound Alpaca server bound_addr=0.0.0.0:54321")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();

        let (port, drain) = parse_bound_port(stdout).await.unwrap();
        assert_eq!(port, 54321);
        drain.abort();
    }

    #[tokio::test]
    async fn test_parse_bound_port_rp_prefix() {
        let mut child = tokio::process::Command::new("echo")
            .arg("Bound rp server bound_addr=127.0.0.1:9999")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();

        let (port, drain) = parse_bound_port(stdout).await.unwrap();
        assert_eq!(port, 9999);
        drain.abort();
    }

    #[tokio::test]
    async fn test_parse_bound_port_arbitrary_prefix() {
        let mut child = tokio::process::Command::new("echo")
            .arg("some future service bound_addr=10.0.0.1:8080")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();

        let (port, drain) = parse_bound_port(stdout).await.unwrap();
        assert_eq!(port, 8080);
        drain.abort();
    }

    #[tokio::test]
    async fn test_parse_bound_port_with_preceding_lines() {
        // printf outputs multiple lines; the port line comes after noise
        let mut child = tokio::process::Command::new("printf")
            .arg("starting up...\nloading config\nBound Alpaca server bound_addr=0.0.0.0:11111\n")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();

        let (port, drain) = parse_bound_port(stdout).await.unwrap();
        assert_eq!(port, 11111);
        drain.abort();
    }

    #[tokio::test]
    async fn test_parse_bound_port_no_match_returns_none() {
        let mut child = tokio::process::Command::new("echo")
            .arg("no port info here")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();

        let result = parse_bound_port(stdout).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_parse_bound_port_empty_output_returns_none() {
        let mut child = tokio::process::Command::new("true")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();

        let result = parse_bound_port(stdout).await;
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // __bdd_bazel_chdir tests
    //
    // All tests that call set_current_dir hold CWD_LOCK to prevent
    // interference when `cargo test` runs them as threads (coverage job).
    // -----------------------------------------------------------------------

    #[test]
    fn test_bdd_bazel_chdir_noop_when_env_unset() {
        let _lock = CWD_LOCK.lock().unwrap();
        std::env::remove_var("BDD_PACKAGE_DIR");
        let before = std::env::current_dir().unwrap();
        __bdd_bazel_chdir();
        assert_eq!(std::env::current_dir().unwrap(), before);
    }

    #[test]
    fn test_bdd_bazel_chdir_changes_directory() {
        let _lock = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("subdir");
        std::fs::create_dir_all(&target).unwrap();

        let previous = std::env::current_dir().unwrap();
        std::env::set_var("BDD_PACKAGE_DIR", &target);
        __bdd_bazel_chdir();
        let after = std::env::current_dir().unwrap();
        std::env::set_current_dir(&previous).unwrap();
        std::env::remove_var("BDD_PACKAGE_DIR");

        // Canonicalize both sides: on macOS /var → /private/var.
        assert_eq!(
            after.canonicalize().unwrap(),
            target.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_bdd_bazel_chdir_absolutizes_binary_env_vars() {
        let _lock = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("pkg");
        std::fs::create_dir_all(&target).unwrap();

        let previous = std::env::current_dir().unwrap();
        let unique_var = "TEST_CHDIR_ABS_BINARY";
        std::env::set_var(unique_var, "relative/path/to/bin");
        std::env::set_var("BDD_PACKAGE_DIR", &target);

        __bdd_bazel_chdir();

        let absolutized = std::env::var(unique_var).unwrap();
        std::env::set_current_dir(&previous).unwrap();
        std::env::remove_var("BDD_PACKAGE_DIR");
        std::env::remove_var(unique_var);

        assert_eq!(
            std::path::PathBuf::from(&absolutized),
            previous.join("relative/path/to/bin")
        );
    }

    #[test]
    fn test_bdd_bazel_chdir_skips_absolute_binary_env_vars() {
        let _lock = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("pkg");
        std::fs::create_dir_all(&target).unwrap();

        // Use the tempdir's own path as the absolute binary value — it's
        // guaranteed absolute on every platform (including the correct
        // drive letter on Windows).
        let abs_path = tmp.path().join("bin").to_string_lossy().to_string();

        let previous = std::env::current_dir().unwrap();
        let unique_var = "TEST_CHDIR_SKIP_BINARY";
        std::env::set_var(unique_var, &abs_path);
        std::env::set_var("BDD_PACKAGE_DIR", &target);

        __bdd_bazel_chdir();

        let value = std::env::var(unique_var).unwrap();
        std::env::set_current_dir(&previous).unwrap();
        std::env::remove_var("BDD_PACKAGE_DIR");
        std::env::remove_var(unique_var);

        assert_eq!(value, abs_path);
    }

    #[test]
    fn test_bdd_bazel_chdir_ignores_non_binary_env_vars() {
        let _lock = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("pkg");
        std::fs::create_dir_all(&target).unwrap();

        let previous = std::env::current_dir().unwrap();
        let unique_var = "TEST_CHDIR_NOT_A_BINARY_SUFFIX";
        std::env::set_var(unique_var, "relative/path");
        std::env::set_var("BDD_PACKAGE_DIR", &target);

        __bdd_bazel_chdir();

        let value = std::env::var(unique_var).unwrap();
        std::env::set_current_dir(&previous).unwrap();
        std::env::remove_var("BDD_PACKAGE_DIR");
        std::env::remove_var(unique_var);

        // Var does NOT end with _BINARY, so it should not be absolutized.
        assert_eq!(value, "relative/path");
    }

    // -----------------------------------------------------------------------
    // binary_env_var tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_binary_env_var_uppercases_and_replaces_dashes() {
        assert_eq!(binary_env_var("rp"), "RP_BINARY");
        assert_eq!(binary_env_var("ppba-driver"), "PPBA_DRIVER_BINARY");
        assert_eq!(binary_env_var("qhy-focuser"), "QHY_FOCUSER_BINARY");
    }

    // -----------------------------------------------------------------------
    // find_binary tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_binary_from_env_var() {
        // Use a package name whose derived env var is unique to this test.
        let package = "bdd-infra-test-find-env";
        std::env::set_var("BDD_INFRA_TEST_FIND_ENV_BINARY", "/some/path/to/binary");
        let result = find_binary(package);
        std::env::remove_var("BDD_INFRA_TEST_FIND_ENV_BINARY");

        assert_eq!(result, Some("/some/path/to/binary".to_string()));
    }

    #[test]
    fn test_find_binary_returns_none_when_nothing_found() {
        let package = "bdd-infra-test-find-none";
        std::env::remove_var("BDD_INFRA_TEST_FIND_NONE_BINARY");
        let result = find_binary(package);
        assert!(result.is_none());
    }

    #[test]
    fn test_find_binary_in_target_dir() {
        // Mutates CARGO_TARGET_DIR; serialize with sibling tests that read
        // it (e.g. `ensure_rp_binary`) via CWD_LOCK.
        let _lock = CWD_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let debug_dir = dir.path().join("debug");
        std::fs::create_dir_all(&debug_dir).unwrap();

        let binary_name = if cfg!(target_os = "windows") {
            "my-service.exe"
        } else {
            "my-service"
        };
        let binary_path = debug_dir.join(binary_name);
        std::fs::write(&binary_path, "fake binary").unwrap();

        // Make sure the derived env var isn't set, so we exercise the
        // target-dir branch.
        std::env::remove_var("MY_SERVICE_BINARY");
        let old_target = std::env::var("CARGO_TARGET_DIR").ok();
        std::env::set_var("CARGO_TARGET_DIR", dir.path());

        let result = find_binary("my-service");

        match old_target {
            Some(v) => std::env::set_var("CARGO_TARGET_DIR", v),
            None => std::env::remove_var("CARGO_TARGET_DIR"),
        }

        // Compare as PathBuf so we don't trip on Windows' mixed separators
        // (Path::join produces `C:\…\debug\my-service.exe`, but a
        // `format!("{}/debug/…")` expected string would have a forward
        // slash in the suffix).
        assert_eq!(
            result.map(std::path::PathBuf::from),
            Some(debug_dir.join(binary_name))
        );
    }

    /// Covers the `CARGO_BUILD_TARGET` triple branch of the `candidate`
    /// closure in `find_binary`.
    #[test]
    fn test_find_binary_in_target_dir_with_triple() {
        // CARGO_BUILD_TARGET is process-global; serialize with other tests
        // that mutate cwd/env via CWD_LOCK so concurrent test threads don't
        // stomp each other under `cargo test` (coverage job).
        let _lock = CWD_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let triple = "x86_64-unknown-linux-gnu";
        let debug_dir = dir.path().join(triple).join("debug");
        std::fs::create_dir_all(&debug_dir).unwrap();

        let binary_name = if cfg!(target_os = "windows") {
            "bdd-infra-test-triple.exe"
        } else {
            "bdd-infra-test-triple"
        };
        let binary_path = debug_dir.join(binary_name);
        std::fs::write(&binary_path, "fake binary").unwrap();

        let old_target = std::env::var("CARGO_TARGET_DIR").ok();
        let old_triple = std::env::var("CARGO_BUILD_TARGET").ok();
        std::env::remove_var("BDD_INFRA_TEST_TRIPLE_BINARY");
        std::env::set_var("CARGO_TARGET_DIR", dir.path());
        std::env::set_var("CARGO_BUILD_TARGET", triple);

        let result = find_binary("bdd-infra-test-triple");

        match old_target {
            Some(v) => std::env::set_var("CARGO_TARGET_DIR", v),
            None => std::env::remove_var("CARGO_TARGET_DIR"),
        }
        match old_triple {
            Some(v) => std::env::set_var("CARGO_BUILD_TARGET", v),
            None => std::env::remove_var("CARGO_BUILD_TARGET"),
        }

        assert_eq!(
            result.map(std::path::PathBuf::from),
            Some(debug_dir.join(binary_name))
        );
    }

    /// Covers the cwd-ancestors walk branch of `find_binary` — the path taken
    /// when no `CARGO_TARGET_DIR` / `CARGO_LLVM_COV_TARGET_DIR` is set, which
    /// is how `cargo test -p <pkg>` from a package directory finds the
    /// workspace `target/`.
    #[test]
    fn test_find_binary_via_ancestor_walk() {
        let _lock = CWD_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let debug_dir = dir.path().join("target").join("debug");
        std::fs::create_dir_all(&debug_dir).unwrap();

        let binary_name = if cfg!(target_os = "windows") {
            "bdd-infra-test-walk.exe"
        } else {
            "bdd-infra-test-walk"
        };
        let binary_path = debug_dir.join(binary_name);
        std::fs::write(&binary_path, "fake binary").unwrap();

        let subdir = dir.path().join("pkg");
        std::fs::create_dir_all(&subdir).unwrap();

        let previous = std::env::current_dir().unwrap();
        let old_target = std::env::var("CARGO_TARGET_DIR").ok();
        let old_llvm_cov = std::env::var("CARGO_LLVM_COV_TARGET_DIR").ok();
        let old_triple = std::env::var("CARGO_BUILD_TARGET").ok();
        std::env::remove_var("BDD_INFRA_TEST_WALK_BINARY");
        std::env::remove_var("CARGO_TARGET_DIR");
        std::env::remove_var("CARGO_LLVM_COV_TARGET_DIR");
        std::env::remove_var("CARGO_BUILD_TARGET");
        std::env::set_current_dir(&subdir).unwrap();

        let result = find_binary("bdd-infra-test-walk");

        std::env::set_current_dir(&previous).unwrap();
        match old_target {
            Some(v) => std::env::set_var("CARGO_TARGET_DIR", v),
            None => std::env::remove_var("CARGO_TARGET_DIR"),
        }
        match old_llvm_cov {
            Some(v) => std::env::set_var("CARGO_LLVM_COV_TARGET_DIR", v),
            None => std::env::remove_var("CARGO_LLVM_COV_TARGET_DIR"),
        }
        match old_triple {
            Some(v) => std::env::set_var("CARGO_BUILD_TARGET", v),
            None => std::env::remove_var("CARGO_BUILD_TARGET"),
        }

        // Canonicalize both sides: on macOS /var → /private/var.
        assert_eq!(
            result
                .map(std::path::PathBuf::from)
                .map(|p| p.canonicalize().unwrap()),
            Some(binary_path.canonicalize().unwrap())
        );
    }

    #[test]
    #[should_panic(expected = "binary for package `bdd-infra-test-require-missing` not found")]
    fn test_require_binary_panics_with_diagnostic() {
        // Mutates CARGO_TARGET_DIR; serialize with sibling tests via CWD_LOCK.
        // `catch_unwind` below requires the lock to be released before the
        // panic propagates, so we drop it explicitly at the end.
        let lock = CWD_LOCK.lock().unwrap();

        std::env::remove_var("BDD_INFRA_TEST_REQUIRE_MISSING_BINARY");
        let old_target = std::env::var("CARGO_TARGET_DIR").ok();
        // Point at an empty dir so the target-dir branch misses too.
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("CARGO_TARGET_DIR", tmp.path());

        let result = std::panic::catch_unwind(|| require_binary("bdd-infra-test-require-missing"));

        match old_target {
            Some(v) => std::env::set_var("CARGO_TARGET_DIR", v),
            None => std::env::remove_var("CARGO_TARGET_DIR"),
        }
        drop(lock);

        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    // -----------------------------------------------------------------------
    // run_once tests
    //
    // These exercise rp's one-shot subcommands (`init-tls`, `hash-password`).
    // rp must be pre-built — we either rely on the conventional env var being
    // set (Bazel path) or build it ourselves for the Cargo path.
    // -----------------------------------------------------------------------

    /// Ensure `RP_BINARY` points at an `rp` binary. Builds it with Cargo if
    /// not already set (e.g. when running under `cargo test`).
    ///
    /// Delegates to [`find_binary`] after the build so we share the same
    /// target-dir / `CARGO_BUILD_TARGET` / ancestor-walk logic that real
    /// callers hit — no hidden assumption that the binary lives at
    /// `<target_dir>/debug/rp`. Takes `CWD_LOCK` to serialize against
    /// sibling tests that mutate env vars [`find_binary`] reads.
    fn ensure_rp_binary() {
        let _lock = CWD_LOCK.lock().unwrap();
        if std::env::var_os("RP_BINARY").is_some() {
            return;
        }
        let build = std::process::Command::new("cargo")
            .args(["build", "--package", "rp", "--quiet"])
            .status()
            .expect("cargo build failed");
        assert!(build.success(), "cargo build --package rp failed");

        let binary = find_binary("rp").expect(
            "rp binary not found after `cargo build --package rp` — check target dir layout",
        );
        std::env::set_var("RP_BINARY", &binary);
    }

    #[test]
    fn test_run_once_successful_command() {
        ensure_rp_binary();
        let dir = tempfile::tempdir().unwrap();
        let output = run_once(
            "rp",
            &["init-tls", "--output-dir", dir.path().to_str().unwrap()],
            None,
        );
        assert!(output.status.success(), "init-tls should succeed");
        assert!(dir.path().join("ca.pem").exists(), "CA cert should exist");
    }

    #[test]
    fn test_run_once_captures_stderr_on_failure() {
        ensure_rp_binary();
        let output = run_once("rp", &["serve"], None);
        // serve without --config should fail
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.is_empty(), "stderr should contain error message");
    }

    #[test]
    fn test_run_once_with_stdin() {
        ensure_rp_binary();
        let output = run_once(
            "rp",
            &["hash-password", "--stdin"],
            Some(b"test-password\n"),
        );

        assert!(
            output.status.success(),
            "hash-password should succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let hash = String::from_utf8(output.stdout).unwrap();
        assert!(
            hash.trim().starts_with("$argon2id$"),
            "expected Argon2id hash, got: {hash}"
        );
    }
}
