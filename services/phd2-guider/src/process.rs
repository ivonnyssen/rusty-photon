//! PHD2 process management for starting and stopping PHD2

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::debug;

use crate::client::Phd2Client;
use crate::config::Phd2Config;
use crate::error::{Phd2Error, Result};
use crate::io::{
    ConnectionFactory, ProcessHandle, ProcessSpawner, TcpConnectionFactory, TokioProcessSpawner,
};

/// Get the default PHD2 executable path for the current platform
///
/// This function checks for PHD2 in the following order:
/// 1. Local build in the external/phd2/tmp directory (relative to workspace root)
/// 2. System-installed PHD2 in standard locations
/// 3. PHD2 in PATH (Linux only)
pub fn get_default_phd2_path() -> Option<PathBuf> {
    // First, try to find the local build relative to the cargo manifest directory
    // The workspace structure is: workspace_root/services/phd2-guider/
    // The local build is at: workspace_root/external/phd2/tmp/phd2.bin
    if let Some(local_path) = get_local_phd2_build_path() {
        if local_path.exists() {
            debug!("Found local PHD2 build at: {}", local_path.display());
            return Some(local_path);
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Check common Linux locations
        let paths = ["/usr/bin/phd2", "/usr/local/bin/phd2"];
        for path in paths {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
        // Try to find in PATH
        if let Ok(output) = std::process::Command::new("which").arg("phd2").output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(PathBuf::from(path));
                }
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    {
        let path = PathBuf::from("/Applications/PHD2.app/Contents/MacOS/PHD2");
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    #[cfg(target_os = "windows")]
    {
        let paths = [
            r"C:\Program Files (x86)\PHDGuiding2\phd2.exe",
            r"C:\Program Files\PHDGuiding2\phd2.exe",
        ];
        for path in paths {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Get the path to the local PHD2 build in the external directory
///
/// Returns the path to external/phd2/tmp/phd2.bin relative to the workspace root.
/// This works by looking for the Cargo.toml workspace file and navigating from there.
fn get_local_phd2_build_path() -> Option<PathBuf> {
    // Try using CARGO_MANIFEST_DIR if available (set during cargo build/test)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        // manifest_dir is services/phd2-guider, go up two levels to workspace root
        let workspace_root = PathBuf::from(manifest_dir)
            .parent()? // services/
            .parent()? // workspace root
            .to_path_buf();
        let local_build = workspace_root.join("external/phd2/tmp/phd2.bin");
        if local_build.exists() {
            return Some(local_build);
        }
    }

    // Try finding workspace root from current directory
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = cwd.as_path();
        // Walk up the directory tree looking for Cargo.toml with [workspace]
        while let Some(parent) = dir.parent() {
            let cargo_toml = dir.join("Cargo.toml");
            if cargo_toml.exists() {
                if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                    if content.contains("[workspace]") {
                        let local_build = dir.join("external/phd2/tmp/phd2.bin");
                        if local_build.exists() {
                            return Some(local_build);
                        }
                    }
                }
            }
            dir = parent;
        }
    }

    None
}

/// PHD2 process manager for starting and stopping PHD2
pub struct Phd2ProcessManager {
    config: Phd2Config,
    process: Arc<Mutex<Option<Box<dyn ProcessHandle>>>>,
    process_spawner: Arc<dyn ProcessSpawner>,
    connection_factory: Arc<dyn ConnectionFactory>,
}

impl Phd2ProcessManager {
    /// Create a new process manager with the given configuration
    ///
    /// Uses the default tokio process spawner and TCP connection factory
    /// for production use.
    pub fn new(config: Phd2Config) -> Self {
        Self::with_spawner(
            config,
            Arc::new(TokioProcessSpawner::new()),
            Arc::new(TcpConnectionFactory::new()),
        )
    }

    /// Create a new process manager with custom spawner and connection factory
    ///
    /// This is useful for testing with mock process spawners and connection factories.
    pub fn with_spawner(
        config: Phd2Config,
        process_spawner: Arc<dyn ProcessSpawner>,
        connection_factory: Arc<dyn ConnectionFactory>,
    ) -> Self {
        Self {
            config,
            process: Arc::new(Mutex::new(None)),
            process_spawner,
            connection_factory,
        }
    }

    /// Check if PHD2 is already running (by attempting to connect)
    pub async fn is_phd2_running(&self) -> bool {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        self.connection_factory.can_connect(&addr).await
    }

    /// Get the PHD2 executable path (from config or default)
    fn get_executable_path(&self) -> Result<PathBuf> {
        if let Some(ref path) = self.config.executable_path {
            if path.exists() {
                return Ok(path.clone());
            }
            return Err(Phd2Error::ExecutableNotFound(path.display().to_string()));
        }

        get_default_phd2_path().ok_or_else(|| {
            Phd2Error::ExecutableNotFound(
                "PHD2 executable not found in default locations".to_string(),
            )
        })
    }

    /// Start PHD2 process
    pub async fn start_phd2(&self) -> Result<()> {
        // Check if already running
        if self.is_phd2_running().await {
            debug!("PHD2 is already running");
            return Ok(());
        }

        // Check if we already have a managed process
        {
            let process = self.process.lock().await;
            if process.is_some() {
                return Err(Phd2Error::ProcessAlreadyRunning);
            }
        }

        let executable = self.get_executable_path()?;
        debug!("Starting PHD2 from: {}", executable.display());

        // Use the process spawner trait to spawn the process
        let child = self
            .process_spawner
            .spawn(&executable, &self.config.spawn_env)
            .await?;

        debug!("PHD2 process started with PID: {:?}", child.id());

        // Store the child process
        {
            let mut process = self.process.lock().await;
            *process = Some(child);
        }

        // Wait for PHD2 to be ready (TCP port available)
        self.wait_for_ready().await?;

        debug!("PHD2 is ready and accepting connections");
        Ok(())
    }

    /// Wait for PHD2 to be ready (TCP port accepting connections)
    async fn wait_for_ready(&self) -> Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let timeout = std::time::Duration::from_secs(self.config.connection_timeout_seconds);
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_millis(500);

        debug!("Waiting for PHD2 to be ready at {}...", addr);

        while start.elapsed() < timeout {
            if self.connection_factory.can_connect(&addr).await {
                return Ok(());
            }

            // Check if process is still running
            {
                let mut process = self.process.lock().await;
                if let Some(ref mut child) = *process {
                    match child.try_wait().await {
                        Ok(Some(status)) => {
                            return Err(Phd2Error::ProcessStartFailed(format!(
                                "PHD2 process exited prematurely with status: {}",
                                status
                            )));
                        }
                        Ok(None) => {
                            // Still running, continue waiting
                        }
                        Err(e) => {
                            return Err(Phd2Error::ProcessStartFailed(format!(
                                "Failed to check process status: {}",
                                e
                            )));
                        }
                    }
                }
            }

            tokio::time::sleep(poll_interval).await;
        }

        Err(Phd2Error::Timeout(format!(
            "PHD2 did not become ready within {} seconds",
            self.config.connection_timeout_seconds
        )))
    }

    /// Stop PHD2 process gracefully
    ///
    /// If a client is provided, it will first try to send the shutdown RPC command.
    /// If that fails or no client is provided, it will kill the process directly.
    pub async fn stop_phd2(&self, client: Option<&Phd2Client>) -> Result<()> {
        // Try graceful shutdown via RPC first
        if let Some(client) = client {
            if client.is_connected().await {
                debug!("Attempting graceful shutdown via RPC...");
                match client.shutdown_phd2().await {
                    Ok(()) => {
                        debug!("Shutdown command sent, waiting for process to exit...");
                        // Wait for process to exit
                        if self.wait_for_exit().await.is_ok() {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        debug!("Graceful shutdown failed: {}, will force kill", e);
                    }
                }
            }
        }

        // Force kill the process
        self.kill_process().await
    }

    /// Wait for the PHD2 process to exit
    async fn wait_for_exit(&self) -> Result<()> {
        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_millis(500);

        while start.elapsed() < timeout {
            {
                let mut process = self.process.lock().await;
                if let Some(ref mut child) = *process {
                    match child.try_wait().await {
                        Ok(Some(_)) => {
                            *process = None;
                            return Ok(());
                        }
                        Ok(None) => {
                            // Still running
                        }
                        Err(e) => {
                            debug!("Error checking process status: {}", e);
                        }
                    }
                } else {
                    // No process being managed
                    return Ok(());
                }
            }
            tokio::time::sleep(poll_interval).await;
        }

        Err(Phd2Error::Timeout(
            "Process did not exit within timeout".to_string(),
        ))
    }

    /// Kill the PHD2 process forcefully
    async fn kill_process(&self) -> Result<()> {
        let mut process = self.process.lock().await;
        if let Some(mut child) = process.take() {
            debug!("Killing PHD2 process...");
            if let Err(e) = child.kill().await {
                debug!("Error killing process: {}", e);
            }
            // Wait for the process to be reaped
            let _ = child.wait().await;
        }
        Ok(())
    }

    /// Check if we are managing a PHD2 process
    pub async fn has_managed_process(&self) -> bool {
        let process = self.process.lock().await;
        process.is_some()
    }
}
