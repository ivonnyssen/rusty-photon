#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Shared BDD test infrastructure for rusty-photon services.
//!
//! Provides [`ServiceHandle`] for spawning, managing, and stopping service
//! binaries during BDD and integration tests. Configuration is read from each
//! service's `Cargo.toml` under `[package.metadata.bdd]`.
//!
//! # Cargo.toml metadata
//!
//! Each service that uses this crate should add:
//!
//! ```toml
//! [package.metadata.bdd]
//! env_var = "MY_SERVICE_BINARY"   # env var for explicit binary path
//! # features = ["mock"]           # optional: cargo features for fallback `cargo run`
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use bdd_infra::ServiceHandle;
//!
//! let handle = ServiceHandle::start(
//!     env!("CARGO_MANIFEST_DIR"),
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

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::debug;

/// BDD configuration read from `[package.metadata.bdd]` in a service's Cargo.toml.
#[derive(Debug, Deserialize)]
struct BddConfig {
    /// Environment variable name for an explicit binary path.
    env_var: String,
    /// Extra cargo features for the `cargo run` fallback (e.g., `["mock"]`).
    #[serde(default)]
    features: Vec<String>,
}

/// Resolve the binary-discovery env var and cargo-features for `package_name`.
///
/// Two paths:
///
/// 1. **Hermetic-build path (Bazel).** If the conventional env var
///    `{PACKAGE_UPPER_SNAKE}_BINARY` is already set (e.g. `FILEMONITOR_BINARY`
///    for `filemonitor`, `PPBA_BINARY` for `ppba-driver`), use that
///    name and skip Cargo.toml entirely. Bazel tests take this path: they
///    cannot rely on `CARGO_MANIFEST_DIR` being valid at runtime, but they
///    can set the conventional env var to the pre-built binary path.
/// 2. **Cargo path.** Otherwise, read `[package.metadata.bdd]` from
///    `{manifest_dir}/Cargo.toml` (the traditional flow). `env_var` and
///    `features` come from there.
fn resolve_bdd_config(manifest_dir: &str, package_name: &str) -> (String, Vec<String>) {
    let conventional = format!("{}_BINARY", package_name.to_uppercase().replace('-', "_"));
    if std::env::var_os(&conventional).is_some() {
        return (conventional, Vec::new());
    }
    let config = load_config(manifest_dir);
    (config.env_var, config.features)
}

/// Load [`BddConfig`] from `{manifest_dir}/Cargo.toml`.
///
/// Falls back to `./Cargo.toml` if the primary path doesn't exist — this
/// covers Bazel, where `env!("CARGO_MANIFEST_DIR")` bakes in an ephemeral
/// sandbox path that's torn down before the test runs. `bdd_main!` chdirs
/// into the package directory under Bazel, so `./Cargo.toml` is the right
/// fallback.
fn load_config(manifest_dir: &str) -> BddConfig {
    let primary = format!("{}/Cargo.toml", manifest_dir);
    let path = if std::path::Path::new(&primary).exists() {
        primary
    } else if std::path::Path::new("Cargo.toml").exists() {
        "Cargo.toml".to_string()
    } else {
        panic!(
            "bdd-infra: cannot locate Cargo.toml (tried {} and ./)",
            primary
        );
    };
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {}", path, e));
    let toml: toml::Table = content
        .parse()
        .unwrap_or_else(|e| panic!("failed to parse {}: {}", path, e));
    let bdd = toml
        .get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("bdd"))
        .unwrap_or_else(|| panic!("missing [package.metadata.bdd] in {}", path));
    BddConfig::deserialize(bdd.clone())
        .unwrap_or_else(|e| panic!("invalid [package.metadata.bdd] in {}: {}", path, e))
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
    /// Reads `[package.metadata.bdd]` from the calling package's Cargo.toml to
    /// determine binary discovery and cargo-run fallback settings.
    ///
    /// # Arguments
    ///
    /// * `manifest_dir` — pass `env!("CARGO_MANIFEST_DIR")` from the calling crate
    /// * `package_name` — pass `env!("CARGO_PKG_NAME")` from the calling crate
    /// * `config_path` — path to the service config file (typically a temp file)
    ///
    /// # Binary discovery order
    ///
    /// 1. Explicit env var (from `[package.metadata.bdd] env_var`)
    /// 2. Pre-built binary in `CARGO_TARGET_DIR` / `CARGO_BUILD_TARGET` layout
    /// 3. Fallback to `cargo run --package <name>`
    pub async fn start(manifest_dir: &str, package_name: &str, config_path: &str) -> Self {
        let (env_var, features) = resolve_bdd_config(manifest_dir, package_name);
        let binary = find_binary(&env_var, package_name);

        let mut child = spawn_process(&binary, package_name, &features, config_path);

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
    pub async fn try_start(
        manifest_dir: &str,
        package_name: &str,
        config_path: &str,
    ) -> Result<Self, String> {
        let (env_var, features) = resolve_bdd_config(manifest_dir, package_name);
        let binary = find_binary(&env_var, package_name);

        let mut child = spawn_process(&binary, package_name, &features, config_path);

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
/// Uses the same binary discovery logic as [`ServiceHandle::start`]:
/// env var, `CARGO_TARGET_DIR`, or `cargo run` fallback. Returns the
/// process output (stdout, stderr, exit status).
///
/// Use this for one-shot commands like `rp init-tls` that are not
/// long-running servers.
/// Run a service binary once and return its output.
///
/// When `stdin_data` is `Some`, the data is piped to the process's stdin.
pub fn run_once(
    manifest_dir: &str,
    package_name: &str,
    args: &[&str],
    stdin_data: Option<&[u8]>,
) -> std::process::Output {
    let (env_var, features) = resolve_bdd_config(manifest_dir, package_name);
    let binary = find_binary(&env_var, package_name);

    let mut cmd = if let Some(binary) = &binary {
        debug!(binary = %binary, "running {} from pre-built binary", package_name);
        let mut cmd = std::process::Command::new(binary);
        cmd.args(args);
        cmd
    } else {
        debug!("running {} via cargo run", package_name);
        let mut full_args = vec!["run", "--package", package_name];
        for feat in &features {
            full_args.push("--features");
            full_args.push(feat);
        }
        full_args.push("--quiet");
        full_args.push("--");
        full_args.extend_from_slice(args);

        let mut cmd = std::process::Command::new("cargo");
        cmd.args(&full_args);
        cmd
    };

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

/// Find a pre-built service binary, or return `None` to fall back to `cargo run`.
///
/// Discovery order:
/// 1. Explicit env var (e.g., `FILEMONITOR_BINARY=/path/to/bin`)
/// 2. `CARGO_TARGET_DIR` (or `CARGO_LLVM_COV_TARGET_DIR`) + optional `CARGO_BUILD_TARGET` triple
/// 3. `target/debug/<binary_name>`
fn find_binary(env_var: &str, package_name: &str) -> Option<String> {
    if let Ok(path) = std::env::var(env_var) {
        return Some(path);
    }

    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .or_else(|_| std::env::var("CARGO_LLVM_COV_TARGET_DIR"))
        .unwrap_or_else(|_| "target".to_string());

    let binary_name = if cfg!(target_os = "windows") {
        format!("{}.exe", package_name)
    } else {
        package_name.to_string()
    };

    // With CARGO_BUILD_TARGET: target/<triple>/debug/<binary>
    if let Ok(triple) = std::env::var("CARGO_BUILD_TARGET") {
        let path = format!("{}/{}/debug/{}", target_dir, triple, binary_name);
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }

    // Without target: target/debug/<binary>
    let path = format!("{}/debug/{}", target_dir, binary_name);
    if std::path::Path::new(&path).exists() {
        return Some(path);
    }

    None
}

/// Windows process creation flag: place the child in its own process group so
/// that `CTRL_BREAK_EVENT` can target it without affecting the test runner.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Spawn the service process, either from a pre-built binary or via `cargo run`.
///
/// On Windows the child is spawned with [`CREATE_NEW_PROCESS_GROUP`] so that
/// [`send_sigterm`] can deliver `CTRL_BREAK_EVENT` only to the child's group
/// without affecting the test runner.
fn spawn_process(
    binary: &Option<String>,
    package_name: &str,
    features: &[String],
    config_path: &str,
) -> tokio::process::Child {
    if let Some(binary) = binary {
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
    } else {
        debug!("starting {} via cargo run", package_name);
        let mut args = vec![
            "run".to_string(),
            "--package".to_string(),
            package_name.to_string(),
        ];
        for feat in features {
            args.push("--features".to_string());
            args.push(feat.clone());
        }
        args.extend([
            "--quiet".to_string(),
            "--".to_string(),
            "--config".to_string(),
            config_path.to_string(),
        ]);

        let mut cmd = tokio::process::Command::new("cargo");
        cmd.args(&args).stdout(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        {
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }
        cmd.spawn()
            .unwrap_or_else(|e| panic!("failed to start {} via cargo run: {}", package_name, e))
    }
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

        let previous = std::env::current_dir().unwrap();
        let unique_var = "TEST_CHDIR_SKIP_BINARY";
        std::env::set_var(unique_var, "/absolute/path/to/bin");
        std::env::set_var("BDD_PACKAGE_DIR", &target);

        __bdd_bazel_chdir();

        let value = std::env::var(unique_var).unwrap();
        std::env::set_current_dir(&previous).unwrap();
        std::env::remove_var("BDD_PACKAGE_DIR");
        std::env::remove_var(unique_var);

        assert_eq!(value, "/absolute/path/to/bin");
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
    // load_config tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_config_with_env_var_only() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_toml = dir.path().join("Cargo.toml");
        std::fs::write(
            &cargo_toml,
            r#"
[package]
name = "test-service"
version = "0.1.0"
edition = "2021"

[package.metadata.bdd]
env_var = "TEST_SERVICE_BINARY"
"#,
        )
        .unwrap();

        let config = load_config(dir.path().to_str().unwrap());
        assert_eq!(config.env_var, "TEST_SERVICE_BINARY");
        assert!(config.features.is_empty());
    }

    #[test]
    fn test_load_config_with_features() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_toml = dir.path().join("Cargo.toml");
        std::fs::write(
            &cargo_toml,
            r#"
[package]
name = "test-service"
version = "0.1.0"
edition = "2021"

[package.metadata.bdd]
env_var = "MY_BINARY"
features = ["mock", "test-helpers"]
"#,
        )
        .unwrap();

        let config = load_config(dir.path().to_str().unwrap());
        assert_eq!(config.env_var, "MY_BINARY");
        assert_eq!(config.features, vec!["mock", "test-helpers"]);
    }

    #[test]
    #[should_panic(expected = "missing [package.metadata.bdd]")]
    fn test_load_config_missing_bdd_section() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_toml = dir.path().join("Cargo.toml");
        std::fs::write(
            &cargo_toml,
            r#"
[package]
name = "test-service"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();

        load_config(dir.path().to_str().unwrap());
    }

    #[test]
    #[should_panic(expected = "invalid [package.metadata.bdd]")]
    fn test_load_config_missing_required_field() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_toml = dir.path().join("Cargo.toml");
        std::fs::write(
            &cargo_toml,
            r#"
[package]
name = "test-service"
version = "0.1.0"
edition = "2021"

[package.metadata.bdd]
features = ["mock"]
"#,
        )
        .unwrap();

        load_config(dir.path().to_str().unwrap());
    }

    #[test]
    #[should_panic(expected = "cannot locate Cargo.toml")]
    fn test_load_config_nonexistent_dir() {
        // The fallback to ./Cargo.toml would silently succeed if cwd were
        // the workspace root (which has a Cargo.toml). chdir into a temp
        // directory so neither the primary nor the fallback path exists.
        let _lock = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        let previous = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(tmp.path()).expect("chdir into tmp");
        let result = std::panic::catch_unwind(|| load_config("/nonexistent/path/to/nowhere"));
        std::env::set_current_dir(previous).expect("chdir back");
        // Propagate the panic so the test's should_panic assertion fires.
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    // -----------------------------------------------------------------------
    // find_binary tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_binary_from_env_var() {
        let unique_var = "BDD_INFRA_TEST_FIND_BINARY_12345";
        std::env::set_var(unique_var, "/some/path/to/binary");
        let result = find_binary(unique_var, "irrelevant");
        std::env::remove_var(unique_var);

        assert_eq!(result, Some("/some/path/to/binary".to_string()));
    }

    #[test]
    fn test_find_binary_returns_none_when_nothing_found() {
        let unique_var = "BDD_INFRA_TEST_FIND_BINARY_NONE";
        std::env::remove_var(unique_var);
        let result = find_binary(unique_var, "nonexistent-binary-xyz");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_binary_in_target_dir() {
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

        let unique_var = "BDD_INFRA_TEST_FIND_BINARY_TARGET";
        std::env::remove_var(unique_var);
        // Temporarily override CARGO_TARGET_DIR
        let old_target = std::env::var("CARGO_TARGET_DIR").ok();
        std::env::set_var("CARGO_TARGET_DIR", dir.path());

        let result = find_binary(unique_var, "my-service");

        // Restore
        match old_target {
            Some(v) => std::env::set_var("CARGO_TARGET_DIR", v),
            None => std::env::remove_var("CARGO_TARGET_DIR"),
        }

        assert_eq!(
            result,
            Some(format!("{}/debug/{}", dir.path().display(), binary_name))
        );
    }

    // -----------------------------------------------------------------------
    // run_once tests
    // -----------------------------------------------------------------------

    /// Use `rp` as the test subject since it has a one-shot `init-tls` subcommand.
    /// The rp manifest dir is one level up from bdd-infra.
    fn rp_manifest_dir() -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("services")
            .join("rp")
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn test_run_once_successful_command() {
        let dir = tempfile::tempdir().unwrap();
        let output = run_once(
            &rp_manifest_dir(),
            "rp",
            &["init-tls", "--output-dir", dir.path().to_str().unwrap()],
            None,
        );
        assert!(output.status.success(), "init-tls should succeed");
        assert!(dir.path().join("ca.pem").exists(), "CA cert should exist");
    }

    #[test]
    fn test_run_once_captures_stderr_on_failure() {
        let output = run_once(&rp_manifest_dir(), "rp", &["serve"], None);
        // serve without --config should fail
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.is_empty(), "stderr should contain error message");
    }

    /// Resolve the absolute path to the rp binary in target/debug.
    /// Builds it first if necessary.
    fn rp_binary_path() -> String {
        let build = std::process::Command::new("cargo")
            .args(["build", "--package", "rp", "--quiet"])
            .status()
            .expect("cargo build failed");
        assert!(build.success(), "cargo build --package rp failed");

        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();

        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| workspace_root.join("target"));

        let binary_name = if cfg!(target_os = "windows") {
            "rp.exe"
        } else {
            "rp"
        };
        let binary = target_dir.join("debug").join(binary_name);
        assert!(
            binary.exists(),
            "rp binary not found at {}",
            binary.display()
        );
        binary.to_string_lossy().to_string()
    }

    #[test]
    fn test_run_once_uses_prebuilt_binary_via_env_var() {
        let binary = rp_binary_path();
        std::env::set_var("RP_BINARY", &binary);

        let dir = tempfile::tempdir().unwrap();
        let output = run_once(
            &rp_manifest_dir(),
            "rp",
            &["init-tls", "--output-dir", dir.path().to_str().unwrap()],
            None,
        );

        std::env::remove_var("RP_BINARY");

        assert!(
            output.status.success(),
            "init-tls via pre-built binary should succeed"
        );
        assert!(dir.path().join("ca.pem").exists(), "CA cert should exist");
    }

    #[test]
    fn test_run_once_with_stdin_via_prebuilt_binary() {
        let binary = rp_binary_path();
        std::env::set_var("RP_BINARY", &binary);

        let output = run_once(
            &rp_manifest_dir(),
            "rp",
            &["hash-password", "--stdin"],
            Some(b"test-password\n"),
        );

        std::env::remove_var("RP_BINARY");

        assert!(
            output.status.success(),
            "hash-password via pre-built binary should succeed"
        );
        let hash = String::from_utf8(output.stdout).unwrap();
        assert!(
            hash.trim().starts_with("$argon2id$"),
            "expected Argon2id hash, got: {hash}"
        );
    }
}
