#![allow(dead_code)]
//! Test infrastructure: ppba-driver process management and config helpers.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::debug;

// ---------------------------------------------------------------------------
// ppba-driver process handle
// ---------------------------------------------------------------------------

/// Handle to a running ppba-driver process
#[derive(Debug)]
pub struct PpbaHandle {
    pub child: Option<tokio::process::Child>,
    pub base_url: String,
    pub port: u16,
    pub config_path: String,
    pub stdout_drain: Option<tokio::task::JoinHandle<()>>,
}

impl PpbaHandle {
    /// Start the ppba-driver binary with the given config file.
    ///
    /// The process binds its own port (use `"port": 0` in config for
    /// OS-assigned allocation) and prints the bound address to stdout.
    /// This function parses that output to discover the actual port.
    ///
    /// Binary discovery order:
    /// 1. `PPBA_BINARY` env var — full path to the binary
    /// 2. Look for the binary in `CARGO_TARGET_DIR` / `CARGO_BUILD_TARGET` layout
    /// 3. Fall back to `cargo run --package ppba-driver --features mock`
    pub async fn start(config_path: &str) -> Self {
        let mut child = if let Some(binary) = Self::find_binary() {
            debug!(binary = %binary, "starting ppba-driver from pre-built binary");
            tokio::process::Command::new(&binary)
                .args(["--config", config_path])
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap_or_else(|e| {
                    panic!("failed to start ppba-driver binary '{}': {}", binary, e)
                })
        } else {
            debug!("starting ppba-driver via cargo run");
            tokio::process::Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "ppba-driver",
                    "--features",
                    "mock",
                    "--quiet",
                    "--",
                    "--config",
                    config_path,
                ])
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("failed to start ppba-driver process")
        };

        let stdout = child
            .stdout
            .take()
            .expect("failed to capture ppba-driver stdout");
        let (port, stdout_drain) = parse_bound_port(stdout)
            .await
            .expect("failed to parse bound port from ppba-driver output");

        Self {
            child: Some(child),
            base_url: format!("http://127.0.0.1:{}", port),
            port,
            config_path: config_path.to_string(),
            stdout_drain: Some(stdout_drain),
        }
    }

    /// Find the ppba-driver binary if pre-built.
    fn find_binary() -> Option<String> {
        // 1. Explicit env var
        if let Ok(path) = std::env::var("PPBA_BINARY") {
            return Some(path);
        }

        // 2. Look in target dir, respecting CARGO_TARGET_DIR and CARGO_BUILD_TARGET
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .or_else(|_| std::env::var("CARGO_LLVM_COV_TARGET_DIR"))
            .unwrap_or_else(|_| "target".to_string());

        let binary_name = if cfg!(target_os = "windows") {
            "ppba-driver.exe"
        } else {
            "ppba-driver"
        };

        // With CARGO_BUILD_TARGET: target/<triple>/debug/ppba-driver
        if let Ok(triple) = std::env::var("CARGO_BUILD_TARGET") {
            let path = format!("{}/{}/debug/{}", target_dir, triple, binary_name);
            if std::path::Path::new(&path).exists() {
                return Some(path);
            }
        }

        // Without target: target/debug/ppba-driver
        let path = format!("{}/debug/{}", target_dir, binary_name);
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }

        None
    }

    /// Stop the ppba-driver process gracefully via SIGTERM, falling back to SIGKILL.
    /// Graceful shutdown allows the process to flush coverage data (profraw).
    pub async fn stop(&mut self) {
        if let Some(handle) = self.stdout_drain.take() {
            handle.abort();
        }
        if let Some(mut child) = self.child.take() {
            if let Some(pid) = child.id() {
                // Send SIGTERM for graceful shutdown
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
                let _ = pid; // suppress unused warning on non-unix

                // Wait up to 5 seconds for clean exit
                match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                    Ok(_) => (),
                    Err(_) => {
                        debug!("ppba-driver did not exit after SIGTERM, sending SIGKILL");
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

impl Drop for PpbaHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.stdout_drain.take() {
            handle.abort();
        }
        if let Some(ref mut child) = self.child {
            // Best-effort SIGTERM on drop, fall back to kill
            if let Some(pid) = child.id() {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
                let _ = pid; // suppress unused warning on non-unix
            } else {
                let _ = child.start_kill();
            }
        }
    }
}

/// Parse the bound port from ppba-driver subprocess stdout.
/// Looks for "Bound Alpaca server bound_addr=<host>:<port>".
/// Returns the port and spawns a background task to drain remaining stdout,
/// preventing the server from blocking when the pipe buffer fills.
async fn parse_bound_port(
    stdout: tokio::process::ChildStdout,
) -> Option<(u16, tokio::task::JoinHandle<()>)> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    while reader.read_line(&mut line).await.ok()? > 0 {
        if let Some(addr_str) = line.trim().strip_prefix("Bound Alpaca server bound_addr=") {
            if let Some(port_str) = addr_str.split(':').next_back() {
                if let Ok(port) = port_str.parse::<u16>() {
                    // Drain remaining stdout in background so the server never
                    // blocks on a write to stdout.
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

// ---------------------------------------------------------------------------
// Config helpers
// ---------------------------------------------------------------------------

/// Build a default test config JSON with both devices enabled.
/// Uses port 0 for OS-assigned port and a short polling interval for fast tests.
pub fn default_test_config() -> serde_json::Value {
    serde_json::json!({
        "serial": {
            "port": "/dev/mock",
            "baud_rate": 9600,
            "polling_interval_ms": 200,
            "timeout_seconds": 2
        },
        "server": { "port": 0 },
        "switch": {
            "enabled": true,
            "name": "Pegasus PPBA Switch",
            "unique_id": "ppba-switch-001",
            "description": "Pegasus Astro PPBA Gen2 Power Control",
            "device_number": 0
        },
        "observingconditions": {
            "enabled": true,
            "name": "Pegasus PPBA Weather",
            "unique_id": "ppba-observingconditions-001",
            "description": "Pegasus Astro PPBA Environmental Sensors",
            "device_number": 0,
            "averaging_period_ms": 300000
        }
    })
}

/// Build a test config with only the switch device enabled.
pub fn switch_only_config() -> serde_json::Value {
    let mut config = default_test_config();
    config["observingconditions"]["enabled"] = serde_json::json!(false);
    config
}

/// Build a test config with only the OC device enabled.
pub fn oc_only_config() -> serde_json::Value {
    let mut config = default_test_config();
    config["switch"]["enabled"] = serde_json::json!(false);
    config
}

/// Build a test config with both devices disabled.
pub fn both_disabled_config() -> serde_json::Value {
    let mut config = default_test_config();
    config["switch"]["enabled"] = serde_json::json!(false);
    config["observingconditions"]["enabled"] = serde_json::json!(false);
    config
}
