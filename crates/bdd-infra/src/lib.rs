#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Shared BDD test infrastructure for rusty-photon services.
//!
//! Provides [`ServiceHandle`] for spawning, managing, and stopping service
//! binaries during BDD and integration tests. Configuration is read from each
//! service's `Cargo.toml` under `[package.metadata.bdd]`.
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
#[macro_export]
macro_rules! bdd_main {
    ($($body:tt)*) => {
        #[cfg(miri)]
        fn main() {}

        #[cfg(not(miri))]
        #[tokio::main]
        async fn main() {
            $($body)*
        }
    };
}

#[cfg(feature = "rp-harness")]
pub mod rp_harness;

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
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

/// Load [`BddConfig`] from `{manifest_dir}/Cargo.toml`.
fn load_config(manifest_dir: &str) -> BddConfig {
    let path = format!("{}/Cargo.toml", manifest_dir);
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

/// Watches a service's stdout for `bound_addr=` lines.
///
/// On each reload the service re-binds and prints a new `bound_addr=<host>:<port>`
/// line. `StdoutWatcher` runs a background task that parses these lines and sends
/// port values through an [`mpsc`] channel so the caller can track port changes.
///
/// All other stdout lines are consumed (drained) to prevent the child process
/// from blocking on a full pipe buffer.
pub struct StdoutWatcher {
    port_rx: mpsc::Receiver<u16>,
    drain_handle: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for StdoutWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdoutWatcher").finish_non_exhaustive()
    }
}

impl StdoutWatcher {
    /// Create a new watcher that reads from `stdout` and sends parsed ports on a channel.
    ///
    /// Returns `(initial_port, watcher)` after parsing the first `bound_addr=` line,
    /// or `None` if stdout closes before a port is found.
    pub async fn new(stdout: tokio::process::ChildStdout) -> Option<(u16, Self)> {
        let (port_tx, port_rx) = mpsc::channel::<u16>(4);
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();

        // Parse the initial port synchronously (before spawning the drain task).
        let initial_port = loop {
            if reader.read_line(&mut line).await.ok()? == 0 {
                return None; // stdout closed
            }
            if let Some(port) = parse_port_from_line(&line) {
                break port;
            }
            line.clear();
        };

        // Spawn background task to drain stdout and report subsequent ports.
        let drain_handle = tokio::spawn(async move {
            let mut buf = String::new();
            while reader.read_line(&mut buf).await.unwrap_or(0) > 0 {
                if let Some(port) = parse_port_from_line(&buf) {
                    // Best-effort: drop on full or closed so the drain never
                    // blocks and lets the child fill its stdout pipe.
                    let _ = port_tx.try_send(port);
                }
                buf.clear();
            }
        });

        Some((
            initial_port,
            Self {
                port_rx,
                drain_handle,
            },
        ))
    }

    /// Wait for the next `bound_addr=` port emission (e.g., after a reload signal).
    ///
    /// Returns `None` if the child process has exited (stdout closed).
    pub async fn next_port(&mut self) -> Option<u16> {
        self.port_rx.recv().await
    }

    /// Abort the background drain task.
    pub fn abort(&self) {
        self.drain_handle.abort();
    }
}

/// Extract a port from a stdout line containing `bound_addr=<host>:<port>`.
fn parse_port_from_line(line: &str) -> Option<u16> {
    let idx = line.find("bound_addr=")?;
    let addr_str = line[idx + "bound_addr=".len()..].trim();
    let port_str = addr_str.split(':').next_back()?;
    port_str.parse::<u16>().ok()
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
    stdout_watcher: Option<StdoutWatcher>,
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
        let config = load_config(manifest_dir);
        let binary = find_binary(&config.env_var, package_name);

        let mut child = spawn_process(&binary, package_name, &config.features, config_path);

        let stdout = child
            .stdout
            .take()
            .unwrap_or_else(|| panic!("failed to capture {} stdout", package_name));
        let (port, watcher) = StdoutWatcher::new(stdout)
            .await
            .unwrap_or_else(|| panic!("failed to parse bound port from {} output", package_name));

        Self {
            child: Some(child),
            port,
            base_url: format!("http://127.0.0.1:{}", port),
            stdout_watcher: Some(watcher),
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
        let config = load_config(manifest_dir);
        let binary = find_binary(&config.env_var, package_name);

        let mut child = spawn_process(&binary, package_name, &config.features, config_path);

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("failed to capture {} stdout", package_name))?;

        match tokio::time::timeout(Duration::from_secs(30), StdoutWatcher::new(stdout)).await {
            Ok(Some((port, watcher))) => Ok(Self {
                child: Some(child),
                port,
                base_url: format!("http://127.0.0.1:{}", port),
                stdout_watcher: Some(watcher),
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
        if let Some(watcher) = self.stdout_watcher.take() {
            watcher.abort();
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

    /// Send a reload signal (SIGHUP / named pipe) and wait for the service to
    /// re-bind on a new port.
    ///
    /// The service's `run_server_loop` handles the signal by re-reading its
    /// config file and restarting the HTTP server. Because the server binds to
    /// port 0, it gets a fresh OS-assigned port on each reload. This method
    /// waits for the new `bound_addr=` line on stdout and updates
    /// [`port`](Self::port) and [`base_url`](Self::base_url) accordingly.
    pub async fn reload(&mut self) -> Result<(), String> {
        let pid = self
            .child
            .as_ref()
            .and_then(|c| c.id())
            .ok_or_else(|| format!("{}: no child process to reload", self.name))?;

        send_reload(pid)
            .map_err(|e| format!("{}: failed to send reload signal: {}", self.name, e))?;

        let watcher = self
            .stdout_watcher
            .as_mut()
            .ok_or_else(|| format!("{}: no stdout watcher", self.name))?;

        match tokio::time::timeout(Duration::from_secs(30), watcher.next_port()).await {
            Ok(Some(port)) => {
                self.port = port;
                self.base_url = format!("http://127.0.0.1:{}", port);
                debug!("{} reloaded on port {}", self.name, port);
                Ok(())
            }
            Ok(None) => Err(format!("{} exited during reload", self.name)),
            Err(_) => Err(format!(
                "{} did not re-bind within 30s after reload",
                self.name
            )),
        }
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        if let Some(watcher) = self.stdout_watcher.take() {
            watcher.abort();
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
    let config = load_config(manifest_dir);
    let binary = find_binary(&config.env_var, package_name);

    let mut cmd = if let Some(binary) = &binary {
        debug!(binary = %binary, "running {} from pre-built binary", package_name);
        let mut cmd = std::process::Command::new(binary);
        cmd.args(args);
        cmd
    } else {
        debug!("running {} via cargo run", package_name);
        let mut full_args = vec!["run", "--package", package_name];
        for feat in &config.features {
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
/// This is a compatibility wrapper around [`StdoutWatcher`]. New code should
/// use `StdoutWatcher::new` directly to also receive port updates after reloads.
pub async fn parse_bound_port(
    stdout: tokio::process::ChildStdout,
) -> Option<(u16, tokio::task::JoinHandle<()>)> {
    let (port, watcher) = StdoutWatcher::new(stdout).await?;
    Some((port, watcher.drain_handle))
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

/// Send a reload signal to a process.
///
/// * **Unix** — sends `SIGHUP`, the standard "re-read configuration" signal.
/// * **Windows** — writes to the named pipe `\\.\pipe\rusty-photon-reload-{pid}`.
///   The service must create this pipe on startup (see each service's `main.rs`).
///
/// Returns an error when the signal transport fails so callers can fail fast
/// instead of waiting for a port that will never arrive (e.g. Windows where
/// the named-pipe reload is not yet implemented on the service side).
fn send_reload(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: libc::kill with a valid pid and SIGHUP is safe.
        let ret = unsafe { libc::kill(pid as i32, libc::SIGHUP) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let pipe_name = format!(r"\\.\pipe\rusty-photon-reload-{}", pid);
        let mut pipe = std::fs::OpenOptions::new().write(true).open(&pipe_name)?;
        use std::io::Write;
        pipe.write_all(b"R")?;
        Ok(())
    }
}

/// A pool of running service processes, keyed by config hash.
///
/// Instead of spawning a fresh server for every BDD scenario, the pool keeps
/// servers alive across scenarios. When a scenario needs a server with a
/// particular config, the pool either reuses an existing one (sending a reload
/// signal to reset state) or starts a new one.
///
/// # Example
///
/// ```rust,ignore
/// static POOL: LazyLock<Mutex<ServerPool>> = LazyLock::new(|| {
///     Mutex::new(ServerPool::new(
///         env!("CARGO_MANIFEST_DIR"),
///         env!("CARGO_PKG_NAME"),
///         vec![vec!["server".into(), "port".into()]],
///     ))
/// });
/// ```
/// Entry in the server pool: a running handle plus the stable config path
/// the server was started with (so reloads re-read from the same location).
struct PoolEntry {
    handle: ServiceHandle,
    config_path: std::path::PathBuf,
}

pub struct ServerPool {
    pool: std::collections::HashMap<u64, PoolEntry>,
    /// Pool-owned temp dir for stable config files that outlive individual scenarios.
    config_dir: std::path::PathBuf,
    manifest_dir: &'static str,
    package_name: &'static str,
    exclude_paths: Vec<Vec<String>>,
}

impl ServerPool {
    /// Create a new empty pool.
    ///
    /// # Arguments
    ///
    /// * `manifest_dir` — pass `env!("CARGO_MANIFEST_DIR")`
    /// * `package_name` — pass `env!("CARGO_PKG_NAME")`
    /// * `exclude_paths` — JSON key paths to ignore when hashing configs
    pub fn new(
        manifest_dir: &'static str,
        package_name: &'static str,
        exclude_paths: Vec<Vec<String>>,
    ) -> Self {
        let config_dir =
            std::env::temp_dir().join(format!("bdd-pool-{}-{}", package_name, std::process::id()));
        std::fs::create_dir_all(&config_dir).expect("failed to create pool config dir");
        Self {
            pool: std::collections::HashMap::new(),
            config_dir,
            manifest_dir,
            package_name,
            exclude_paths,
        }
    }

    /// Get a running server for this config, starting a new one if needed.
    ///
    /// If a server with a matching config hash already exists in the pool, its
    /// config file is overwritten and a reload signal is sent so it picks up
    /// any per-run artifacts (temp file paths, etc.) that differ between
    /// scenarios but are excluded from the hash.
    ///
    /// Returns the port and base URL of the (re)started server.
    pub async fn get_or_start(
        &mut self,
        config: &serde_json::Value,
    ) -> Result<(u16, String), String> {
        let refs: Vec<Vec<&str>> = self
            .exclude_paths
            .iter()
            .map(|p| p.iter().map(String::as_str).collect())
            .collect();
        let slices: Vec<&[&str]> = refs.iter().map(|v| v.as_slice()).collect();
        let hash = config_hash(config, &slices);

        if let Some(entry) = self.pool.get_mut(&hash) {
            if entry.handle.is_running() {
                debug!(
                    hash,
                    old_port = entry.handle.port,
                    "pool: reloading existing server"
                );
                // Overwrite the stable config file so the reloaded server
                // picks up new per-scenario artifacts (temp file paths, etc.).
                std::fs::write(
                    &entry.config_path,
                    serde_json::to_string_pretty(config).unwrap(),
                )
                .map_err(|e| format!("failed to write config: {}", e))?;
                match entry.handle.reload().await {
                    Ok(()) => {
                        debug!(hash, new_port = entry.handle.port, "pool: reload succeeded");
                        return Ok((entry.handle.port, entry.handle.base_url.clone()));
                    }
                    Err(e) => {
                        debug!(hash, error = %e, "pool: reload failed, stopping stale entry");
                        // Remove by value and explicitly stop() so the child
                        // process is waited on before being discarded — Drop
                        // only sends a best-effort SIGTERM without awaiting.
                        if let Some(mut stale) = self.pool.remove(&hash) {
                            stale.handle.stop().await;
                        }
                        // Fall through to start fresh below.
                    }
                }
            } else {
                debug!(hash, "pool: server no longer running, removing stale entry");
                // Server died — remove stale entry and start fresh.
                self.pool.remove(&hash);
            }
        }

        // Create a stable config path owned by the pool (outlives scenarios).
        debug!(hash, "pool: starting new server");
        let config_path = self.config_dir.join(format!("pool-{}.json", hash));
        std::fs::write(&config_path, serde_json::to_string_pretty(config).unwrap())
            .map_err(|e| format!("failed to write config: {}", e))?;
        let config_path_str = config_path
            .to_str()
            .ok_or_else(|| format!("config path is not valid UTF-8: {}", config_path.display()))?;
        let handle =
            ServiceHandle::start(self.manifest_dir, self.package_name, config_path_str).await;
        let port = handle.port;
        let base_url = handle.base_url.clone();
        self.pool.insert(
            hash,
            PoolEntry {
                handle,
                config_path,
            },
        );
        Ok((port, base_url))
    }

    /// Compute the config hash for the given config value.
    pub fn hash_config(&self, config: &serde_json::Value) -> u64 {
        let refs: Vec<Vec<&str>> = self
            .exclude_paths
            .iter()
            .map(|p| p.iter().map(String::as_str).collect())
            .collect();
        let slices: Vec<&[&str]> = refs.iter().map(|v| v.as_slice()).collect();
        config_hash(config, &slices)
    }

    /// Stop all servers in the pool and clean up config files.
    pub async fn stop_all(&mut self) {
        for (_, entry) in self.pool.iter_mut() {
            entry.handle.stop().await;
        }
        self.pool.clear();
        let _ = std::fs::remove_dir_all(&self.config_dir);
    }
}

/// Compute a deterministic hash of a JSON config, ignoring specified fields.
///
/// This is used by [`ServerPool`] to decide whether an existing server can be
/// reused: if two configs produce the same hash (after excluding per-run
/// artifacts like ports and temp paths), they are functionally equivalent.
///
/// The hash walks the [`serde_json::Value`] tree directly and sorts object
/// keys before folding them into the hasher, so it is independent of the
/// backing map's iteration order (defensive against enabling serde_json's
/// `preserve_order` feature via cargo feature unification).
///
/// # Arguments
///
/// * `config` — the full JSON config value
/// * `exclude_paths` — list of JSON key paths to ignore (e.g., `&[&["server", "port"]]`)
pub fn config_hash(config: &serde_json::Value, exclude_paths: &[&[&str]]) -> u64 {
    use std::hash::Hasher;

    let mut cleaned = config.clone();
    for path in exclude_paths {
        remove_json_path(&mut cleaned, path);
    }

    let mut hasher = std::hash::DefaultHasher::new();
    hash_json_canonical(&cleaned, &mut hasher);
    hasher.finish()
}

/// Recursively fold a JSON value into `hasher` in a canonical, order-independent
/// way. Object keys are sorted; variant tags are prefixed so that e.g. the
/// string `"null"` does not collide with `Value::Null`.
fn hash_json_canonical<H: std::hash::Hasher>(value: &serde_json::Value, hasher: &mut H) {
    use std::hash::Hash;

    match value {
        serde_json::Value::Null => 0u8.hash(hasher),
        serde_json::Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        serde_json::Value::Number(n) => {
            2u8.hash(hasher);
            n.to_string().hash(hasher);
        }
        serde_json::Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        serde_json::Value::Array(items) => {
            4u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_json_canonical(item, hasher);
            }
        }
        serde_json::Value::Object(map) => {
            5u8.hash(hasher);
            map.len().hash(hasher);
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort_unstable();
            for key in keys {
                key.hash(hasher);
                if let Some(child) = map.get(key) {
                    hash_json_canonical(child, hasher);
                }
            }
        }
    }
}

/// Remove a nested key from a JSON value by following a path of keys.
///
/// For example, `remove_json_path(val, &["server", "port"])` removes
/// `val["server"]["port"]` if it exists.
fn remove_json_path(value: &mut serde_json::Value, path: &[&str]) {
    if path.is_empty() {
        return;
    }
    if path.len() == 1 {
        if let serde_json::Value::Object(map) = value {
            map.remove(path[0]);
        }
        return;
    }
    if let serde_json::Value::Object(map) = value {
        if let Some(child) = map.get_mut(path[0]) {
            remove_json_path(child, &path[1..]);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::process::Stdio;

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
    // StdoutWatcher tests
    // -----------------------------------------------------------------------

    /// Regression: when more `bound_addr=` lines are emitted than the port
    /// channel capacity (4), the drain task must drop the overflow instead of
    /// blocking. A blocking send would stall the drain and eventually
    /// deadlock the child process once its stdout pipe filled.
    #[tokio::test]
    async fn test_stdout_watcher_drain_does_not_block_when_channel_fills() {
        // 10 bound_addr= lines, one printf call. Matches the portability
        // approach of `test_parse_bound_port_with_preceding_lines`.
        let mut child = tokio::process::Command::new("printf")
            .arg(
                "bound_addr=127.0.0.1:1\n\
                 bound_addr=127.0.0.1:2\n\
                 bound_addr=127.0.0.1:3\n\
                 bound_addr=127.0.0.1:4\n\
                 bound_addr=127.0.0.1:5\n\
                 bound_addr=127.0.0.1:6\n\
                 bound_addr=127.0.0.1:7\n\
                 bound_addr=127.0.0.1:8\n\
                 bound_addr=127.0.0.1:9\n\
                 bound_addr=127.0.0.1:10\n",
            )
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();

        let (initial, watcher) = StdoutWatcher::new(stdout).await.unwrap();
        assert_eq!(initial, 1);

        // Never consume next_port() — subsequent sends must not block even
        // though the bounded channel (capacity 4) will fill.
        tokio::time::timeout(Duration::from_secs(5), watcher.drain_handle)
            .await
            .expect("drain task blocked on full channel")
            .expect("drain task panicked");

        let _ = child.wait().await;
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
    #[should_panic(expected = "failed to read")]
    fn test_load_config_nonexistent_dir() {
        load_config("/nonexistent/path/to/nowhere");
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

    // -----------------------------------------------------------------------
    // config_hash tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_config_hash_is_stable_across_calls() {
        let cfg = serde_json::json!({ "server": { "port": 1234 }, "name": "a" });
        assert_eq!(config_hash(&cfg, &[]), config_hash(&cfg, &[]));
    }

    #[test]
    fn test_config_hash_distinguishes_different_configs() {
        let a = serde_json::json!({ "name": "alpha" });
        let b = serde_json::json!({ "name": "beta" });
        assert_ne!(config_hash(&a, &[]), config_hash(&b, &[]));
    }

    #[test]
    fn test_config_hash_ignores_excluded_top_level_field() {
        let a = serde_json::json!({ "server": { "port": 1 }, "name": "svc" });
        let b = serde_json::json!({ "server": { "port": 9999 }, "name": "svc" });
        let exclude: &[&[&str]] = &[&["server", "port"]];
        assert_eq!(config_hash(&a, exclude), config_hash(&b, exclude));
    }

    #[test]
    fn test_config_hash_ignores_excluded_nested_field() {
        let a = serde_json::json!({ "file": { "path": "/tmp/a", "mode": "ro" } });
        let b = serde_json::json!({ "file": { "path": "/tmp/b", "mode": "ro" } });
        let exclude: &[&[&str]] = &[&["file", "path"]];
        assert_eq!(config_hash(&a, exclude), config_hash(&b, exclude));
    }

    #[test]
    fn test_config_hash_detects_changes_outside_excluded_paths() {
        let a = serde_json::json!({ "server": { "port": 1 }, "name": "alpha" });
        let b = serde_json::json!({ "server": { "port": 1 }, "name": "beta" });
        let exclude: &[&[&str]] = &[&["server", "port"]];
        assert_ne!(config_hash(&a, exclude), config_hash(&b, exclude));
    }

    #[test]
    fn test_config_hash_handles_missing_excluded_path() {
        // Excluding a path that isn't present should be a no-op, not a panic.
        let cfg = serde_json::json!({ "name": "svc" });
        let exclude: &[&[&str]] = &[&["missing", "nested", "key"]];
        let _ = config_hash(&cfg, exclude);
    }

    #[test]
    fn test_config_hash_distinguishes_array_order() {
        // Arrays are order-sensitive; only object keys are canonicalized.
        let a = serde_json::json!({ "items": [1, 2, 3] });
        let b = serde_json::json!({ "items": [3, 2, 1] });
        assert_ne!(config_hash(&a, &[]), config_hash(&b, &[]));
    }

    #[test]
    fn test_config_hash_distinguishes_null_from_missing_key() {
        let a = serde_json::json!({ "k": null });
        let b = serde_json::json!({});
        assert_ne!(config_hash(&a, &[]), config_hash(&b, &[]));
    }

    #[test]
    fn test_config_hash_distinguishes_value_variants() {
        // Ensure the variant tag prefixes prevent cross-type collisions
        // (e.g. string "true" vs bool true).
        let string_true = serde_json::json!({ "k": "true" });
        let bool_true = serde_json::json!({ "k": true });
        assert_ne!(config_hash(&string_true, &[]), config_hash(&bool_true, &[]));

        let string_one = serde_json::json!({ "k": "1" });
        let num_one = serde_json::json!({ "k": 1 });
        assert_ne!(config_hash(&string_one, &[]), config_hash(&num_one, &[]));
    }
}
