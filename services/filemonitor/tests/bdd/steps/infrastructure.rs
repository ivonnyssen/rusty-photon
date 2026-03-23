//! Test infrastructure: filemonitor process management.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::debug;

/// Handle to a running filemonitor process
#[derive(Debug)]
pub struct FilemonitorHandle {
    pub child: Option<tokio::process::Child>,
    pub port: u16,
    pub stdout_drain: Option<tokio::task::JoinHandle<()>>,
}

impl FilemonitorHandle {
    /// Start the filemonitor binary with the given config file.
    ///
    /// Binary discovery order:
    /// 1. `FILEMONITOR_BINARY` env var — full path to the binary
    /// 2. Look for the binary in `CARGO_TARGET_DIR` / `CARGO_BUILD_TARGET` layout
    /// 3. Fall back to `cargo run --package filemonitor`
    pub async fn start(config_path: &str) -> Self {
        let mut child = if let Some(binary) = Self::find_binary() {
            debug!(binary = %binary, "starting filemonitor from pre-built binary");
            tokio::process::Command::new(&binary)
                .args(["--config", config_path])
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap_or_else(|e| {
                    panic!("failed to start filemonitor binary '{}': {}", binary, e)
                })
        } else {
            debug!("starting filemonitor via cargo run");
            tokio::process::Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "filemonitor",
                    "--quiet",
                    "--",
                    "--config",
                    config_path,
                ])
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("failed to start filemonitor process")
        };

        let stdout = child
            .stdout
            .take()
            .expect("failed to capture filemonitor stdout");
        let (port, stdout_drain) = parse_bound_port(stdout)
            .await
            .expect("failed to parse bound port from filemonitor output");

        Self {
            child: Some(child),
            port,
            stdout_drain: Some(stdout_drain),
        }
    }

    /// Try to start the filemonitor binary, returning an error if it fails.
    pub async fn try_start(config_path: &str) -> Result<Self, String> {
        let mut child = if let Some(binary) = Self::find_binary() {
            tokio::process::Command::new(&binary)
                .args(["--config", config_path])
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("failed to spawn: {}", e))?
        } else {
            tokio::process::Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "filemonitor",
                    "--quiet",
                    "--",
                    "--config",
                    config_path,
                ])
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("failed to spawn: {}", e))?
        };

        let stdout = child
            .stdout
            .take()
            .expect("failed to capture filemonitor stdout");

        match tokio::time::timeout(Duration::from_secs(10), parse_bound_port(stdout)).await {
            Ok(Some((port, stdout_drain))) => Ok(Self {
                child: Some(child),
                port,
                stdout_drain: Some(stdout_drain),
            }),
            Ok(None) => {
                let status = child.wait().await;
                Err(format!("filemonitor exited without binding: {:?}", status))
            }
            Err(_) => {
                let _ = child.kill().await;
                Err("timeout waiting for filemonitor to bind".to_string())
            }
        }
    }

    /// Find the filemonitor binary if pre-built.
    fn find_binary() -> Option<String> {
        if let Ok(path) = std::env::var("FILEMONITOR_BINARY") {
            return Some(path);
        }

        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .or_else(|_| std::env::var("CARGO_LLVM_COV_TARGET_DIR"))
            .unwrap_or_else(|_| "target".to_string());

        let binary_name = if cfg!(target_os = "windows") {
            "filemonitor.exe"
        } else {
            "filemonitor"
        };

        if let Ok(triple) = std::env::var("CARGO_BUILD_TARGET") {
            let path = format!("{}/{}/debug/{}", target_dir, triple, binary_name);
            if std::path::Path::new(&path).exists() {
                return Some(path);
            }
        }

        let path = format!("{}/debug/{}", target_dir, binary_name);
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }

        None
    }

    /// Stop the filemonitor process gracefully via SIGTERM, falling back to SIGKILL.
    /// Graceful shutdown allows the process to flush coverage data (profraw).
    pub async fn stop(&mut self) {
        if let Some(handle) = self.stdout_drain.take() {
            handle.abort();
        }
        if let Some(mut child) = self.child.take() {
            if let Some(pid) = child.id() {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
                let _ = pid;

                match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                    Ok(_) => (),
                    Err(_) => {
                        debug!("filemonitor did not exit after SIGTERM, sending SIGKILL");
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

impl Drop for FilemonitorHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.stdout_drain.take() {
            handle.abort();
        }
        if let Some(ref mut child) = self.child {
            if let Some(pid) = child.id() {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
                let _ = pid;
            } else {
                let _ = child.start_kill();
            }
        }
    }
}

/// Parse the bound port from filemonitor subprocess stdout.
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
