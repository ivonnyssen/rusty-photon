#![allow(dead_code)]
//! Test infrastructure: OmniSim process management, rp process management,
//! test webhook receiver, and test orchestrator.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::debug;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use serde_json::Value;
use tokio::sync::{OnceCell, RwLock};

use crate::world::{OrchestratorInvocation, ReceivedEvent};

// ---------------------------------------------------------------------------
// OmniSim native process handle (shared singleton)
// ---------------------------------------------------------------------------

/// Shared OmniSim info returned to each scenario
#[derive(Debug, Clone)]
pub struct OmniSimHandle {
    pub base_url: String,
    pub port: u16,
}

/// Singleton that owns the OmniSim child process for the entire test run
struct OmniSimProcess {
    _child: tokio::process::Child,
    base_url: String,
    port: u16,
}

/// Global singleton — one OmniSim process shared by all scenarios
static OMNISIM: OnceCell<OmniSimProcess> = OnceCell::const_new();

impl OmniSimHandle {
    /// Get or start the shared OmniSim process. Returns a lightweight handle.
    ///
    /// Binary discovery order:
    /// 1. `OMNISIM_PATH` env var — full path to the binary
    /// 2. `OMNISIM_DIR` env var — directory containing the binary
    /// 3. `ascom.alpaca.simulators` (or `.exe` on Windows) on `PATH`
    pub async fn start() -> Self {
        let process = OMNISIM
            .get_or_init(|| async { OmniSimProcess::spawn().await })
            .await;
        Self {
            base_url: process.base_url.clone(),
            port: process.port,
        }
    }
}

impl OmniSimProcess {
    async fn spawn() -> Self {
        let binary = Self::find_binary();
        let port = Self::find_free_port().await;

        // Clear sanitizer-related env vars so the .NET runtime isn't broken
        // by LD_PRELOAD injection from ASAN/LSAN.
        let child = tokio::process::Command::new(&binary)
            .args(["--urls", &format!("http://127.0.0.1:{}", port)])
            .env_remove("LD_PRELOAD")
            .env_remove("ASAN_OPTIONS")
            .env_remove("LSAN_OPTIONS")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|e| panic!("failed to start OmniSim binary '{}': {}", binary, e));

        let process = Self {
            _child: child,
            base_url: format!("http://127.0.0.1:{}", port),
            port,
        };
        process.wait_healthy().await;
        process
    }

    /// Find the OmniSim binary using env vars or PATH
    fn find_binary() -> String {
        // 1. OMNISIM_PATH — full path to the binary
        if let Ok(path) = std::env::var("OMNISIM_PATH") {
            return path;
        }

        let binary_name = if cfg!(target_os = "windows") {
            "ascom.alpaca.simulators.exe"
        } else {
            "ascom.alpaca.simulators"
        };

        // 2. OMNISIM_DIR — directory containing the binary
        if let Ok(dir) = std::env::var("OMNISIM_DIR") {
            let path = std::path::Path::new(&dir).join(binary_name);
            return path.to_string_lossy().to_string();
        }

        // 3. Assume it's on PATH
        binary_name.to_string()
    }

    /// Allocate a free TCP port by binding to :0 and releasing
    async fn find_free_port() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind to find free port for OmniSim");
        listener.local_addr().unwrap().port()
    }

    /// Wait for OmniSim to respond to HTTP requests
    async fn wait_healthy(&self) {
        let client = reqwest::Client::new();
        let url = format!("{}/api/v1/camera/0/connected", self.base_url);
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(resp) = client.get(&url).send().await {
                if resp.status().is_success() {
                    return;
                }
            }
        }
        panic!("OmniSim did not become healthy within 30 seconds");
    }
}

// ---------------------------------------------------------------------------
// rp process handle
// ---------------------------------------------------------------------------

/// Handle to a running rp process
#[derive(Debug)]
pub struct RpHandle {
    pub child: Option<tokio::process::Child>,
    pub base_url: String,
    pub port: u16,
    pub config_path: String,
    pub stdout_drain: Option<tokio::task::JoinHandle<()>>,
}

impl RpHandle {
    /// Start the rp binary with the given config file.
    ///
    /// The rp process binds its own port (use `"port": 0` in config for
    /// OS-assigned allocation) and prints the bound address to stdout.
    /// This function parses that output to discover the actual port,
    /// eliminating the TOCTOU race of pre-allocating a port externally.
    ///
    /// Binary discovery order:
    /// 1. `RP_BINARY` env var — full path to the binary
    /// 2. Look for the binary in `CARGO_TARGET_DIR` / `CARGO_BUILD_TARGET` layout
    /// 3. Fall back to `cargo run --package rp`
    pub async fn start(config_path: &str) -> Self {
        let mut child = if let Some(binary) = Self::find_binary() {
            debug!(binary = %binary, "starting rp from pre-built binary");
            tokio::process::Command::new(&binary)
                .args(["--config", config_path])
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap_or_else(|e| panic!("failed to start rp binary '{}': {}", binary, e))
        } else {
            debug!("starting rp via cargo run");
            tokio::process::Command::new("cargo")
                .args([
                    "run",
                    "--package",
                    "rp",
                    "--quiet",
                    "--",
                    "--config",
                    config_path,
                ])
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("failed to start rp process")
        };

        let stdout = child.stdout.take().expect("failed to capture rp stdout");
        let (port, stdout_drain) = parse_bound_port(stdout)
            .await
            .expect("failed to parse bound port from rp output");

        Self {
            child: Some(child),
            base_url: format!("http://127.0.0.1:{}", port),
            port,
            config_path: config_path.to_string(),
            stdout_drain: Some(stdout_drain),
        }
    }

    /// Find the rp binary if pre-built.
    fn find_binary() -> Option<String> {
        // 1. Explicit env var
        if let Ok(path) = std::env::var("RP_BINARY") {
            return Some(path);
        }

        // 2. Look in target dir, respecting CARGO_TARGET_DIR and CARGO_BUILD_TARGET
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .or_else(|_| std::env::var("CARGO_LLVM_COV_TARGET_DIR"))
            .unwrap_or_else(|_| "target".to_string());

        let binary_name = if cfg!(target_os = "windows") {
            "rp.exe"
        } else {
            "rp"
        };

        // With CARGO_BUILD_TARGET: target/<triple>/debug/rp
        if let Ok(triple) = std::env::var("CARGO_BUILD_TARGET") {
            let path = format!("{}/{}/debug/{}", target_dir, triple, binary_name);
            if std::path::Path::new(&path).exists() {
                return Some(path);
            }
        }

        // Without target: target/debug/rp
        let path = format!("{}/debug/{}", target_dir, binary_name);
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }

        None
    }

    /// Stop the rp process gracefully via SIGTERM, falling back to SIGKILL.
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
                    Ok(_) => return,
                    Err(_) => {
                        debug!("rp did not exit after SIGTERM, sending SIGKILL");
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

impl Drop for RpHandle {
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

/// Parse the bound port from rp subprocess stdout.
/// Looks for "Bound rp server bound_addr=<host>:<port>".
/// Returns the port and spawns a background task to drain remaining stdout,
/// preventing the server from blocking when the pipe buffer fills.
async fn parse_bound_port(
    stdout: tokio::process::ChildStdout,
) -> Option<(u16, tokio::task::JoinHandle<()>)> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    while reader.read_line(&mut line).await.ok()? > 0 {
        if let Some(addr_str) = line.trim().strip_prefix("Bound rp server bound_addr=") {
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
// Test webhook receiver
// ---------------------------------------------------------------------------

/// Shared state for the webhook receiver
#[derive(Debug, Clone)]
pub struct WebhookReceiverState {
    pub events: Arc<RwLock<Vec<ReceivedEvent>>>,
    pub ack_estimated_secs: u64,
    pub ack_max_secs: u64,
}

/// In-process HTTP server that acts as an event plugin
#[derive(Debug)]
pub struct WebhookReceiver {
    pub url: String,
    pub port: u16,
    pub events: Arc<RwLock<Vec<ReceivedEvent>>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl WebhookReceiver {
    /// Start the webhook receiver on a random port
    pub async fn start(
        events: Arc<RwLock<Vec<ReceivedEvent>>>,
        ack_estimated_secs: u64,
        ack_max_secs: u64,
    ) -> Self {
        let state = WebhookReceiverState {
            events: events.clone(),
            ack_estimated_secs,
            ack_max_secs,
        };

        let app = Router::new()
            .route("/webhook", post(webhook_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind webhook receiver");
        let port = listener.local_addr().unwrap().port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("webhook receiver failed");
        });

        Self {
            url: format!("http://127.0.0.1:{}/webhook", port),
            port,
            events,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Stop the webhook receiver
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for WebhookReceiver {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn webhook_handler(
    State(state): State<WebhookReceiverState>,
    axum::Json(body): axum::Json<Value>,
) -> (StatusCode, axum::Json<Value>) {
    let event = ReceivedEvent {
        event_id: body
            .get("event_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        event_type: body
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        timestamp: body
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        payload: body.get("payload").cloned().unwrap_or(Value::Null),
        received_at: std::time::Instant::now(),
    };

    state.events.write().await.push(event);

    let ack = serde_json::json!({
        "estimated_duration_secs": state.ack_estimated_secs,
        "max_duration_secs": state.ack_max_secs
    });

    (StatusCode::OK, axum::Json(ack))
}

// ---------------------------------------------------------------------------
// Test orchestrator
// ---------------------------------------------------------------------------

/// Shared state for the test orchestrator
#[derive(Debug, Clone)]
pub struct TestOrchestratorState {
    pub invocations: Arc<RwLock<Vec<OrchestratorInvocation>>>,
    pub cancelled: Arc<RwLock<bool>>,
    pub behavior: OrchestratorBehavior,
}

/// Configurable behavior for the test orchestrator
#[derive(Debug, Clone)]
pub enum OrchestratorBehavior {
    /// Complete immediately after being invoked
    CompleteImmediately,
    /// Wait until explicitly stopped (via cancellation)
    WaitForStop,
    /// Run a flat calibration plan: Vec<(filter, count, duration_secs)>
    FlatCalibration(Vec<(String, u32, f64)>),
}

impl Default for OrchestratorBehavior {
    fn default() -> Self {
        Self::CompleteImmediately
    }
}

/// In-process HTTP server that acts as an orchestrator plugin
#[derive(Debug)]
pub struct TestOrchestrator {
    pub invoke_url: String,
    pub port: u16,
    pub invocations: Arc<RwLock<Vec<OrchestratorInvocation>>>,
    pub cancelled: Arc<RwLock<bool>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TestOrchestrator {
    /// Start the test orchestrator on a random port
    pub async fn start(
        invocations: Arc<RwLock<Vec<OrchestratorInvocation>>>,
        cancelled: Arc<RwLock<bool>>,
        behavior: OrchestratorBehavior,
    ) -> Self {
        let state = TestOrchestratorState {
            invocations: invocations.clone(),
            cancelled: cancelled.clone(),
            behavior,
        };

        let app = Router::new()
            .route("/invoke", post(orchestrator_invoke_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind test orchestrator");
        let port = listener.local_addr().unwrap().port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("test orchestrator failed");
        });

        Self {
            invoke_url: format!("http://127.0.0.1:{}/invoke", port),
            port,
            invocations,
            cancelled,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for TestOrchestrator {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn orchestrator_invoke_handler(
    State(state): State<TestOrchestratorState>,
    axum::Json(body): axum::Json<Value>,
) -> (StatusCode, axum::Json<Value>) {
    let invocation = OrchestratorInvocation {
        workflow_id: body
            .get("workflow_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        session_id: body
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        mcp_server_url: body
            .get("mcp_server_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        recovery: body.get("recovery").cloned(),
    };

    state.invocations.write().await.push(invocation.clone());

    // Spawn the orchestrator behavior in the background
    let mcp_server_url = invocation.mcp_server_url.clone();
    let workflow_id = invocation.workflow_id.clone();
    let cancelled = state.cancelled.clone();
    let behavior = state.behavior.clone();

    tokio::spawn(async move {
        match behavior {
            OrchestratorBehavior::CompleteImmediately => {
                // Post completion back to rp immediately
                post_completion(&mcp_server_url, &workflow_id).await;
            }
            OrchestratorBehavior::WaitForStop => {
                // Wait until cancelled flag is set (checked by polling)
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if *cancelled.read().await {
                        break;
                    }
                }
            }
            OrchestratorBehavior::FlatCalibration(plan) => {
                // Connect as MCP client and drive the flat calibration workflow
                run_flat_calibration(&mcp_server_url, &workflow_id, &plan).await;
            }
        }
    });

    // Acknowledge with timing estimates
    let ack = serde_json::json!({
        "estimated_duration_secs": 300,
        "max_duration_secs": 0
    });

    (StatusCode::OK, axum::Json(ack))
}

/// Post orchestrator completion back to rp
async fn post_completion(mcp_server_url: &str, workflow_id: &str) {
    // Derive the rp base URL from the MCP URL (strip /mcp suffix)
    let base_url = mcp_server_url.trim_end_matches("/mcp");
    let url = format!("{}/api/plugins/{}/complete", base_url, workflow_id);

    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "status": "complete",
        "result": {
            "reason": "all_targets_complete",
            "exposures_captured": 0
        }
    });

    // Best-effort — if rp is down, this will just fail
    let _ = client.post(&url).json(&body).send().await;
}

/// Drive a flat calibration workflow via MCP tool calls
async fn run_flat_calibration(
    mcp_server_url: &str,
    workflow_id: &str,
    plan: &[(String, u32, f64)],
) {
    // TODO: Connect as MCP client to mcp_server_url and call tools:
    //   - set_filter for each filter
    //   - capture for each flat frame
    // For now, this is a placeholder that will be implemented when
    // the MCP client integration is available.

    let base_url = mcp_server_url.trim_end_matches("/mcp");

    // Placeholder: use REST API to simulate tool calls until MCP client is wired
    let client = reqwest::Client::new();
    for (filter, count, duration_secs) in plan {
        // Set filter
        let _set_filter = client
            .post(&format!("{}/mcp", base_url))
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "set_filter",
                    "arguments": {
                        "filter_wheel_id": "main-fw",
                        "filter_name": filter
                    }
                }
            }))
            .send()
            .await;

        // Capture N flats
        for _ in 0..*count {
            let _capture = client
                .post(&format!("{}/mcp", base_url))
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "capture",
                        "arguments": {
                            "camera_id": "main-cam",
                            "duration_secs": duration_secs
                        }
                    }
                }))
                .send()
                .await;
        }
    }

    // Post completion
    post_completion(mcp_server_url, workflow_id).await;
}
