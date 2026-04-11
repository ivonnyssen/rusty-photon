#![allow(dead_code)]
//! Test infrastructure: OmniSim process management, rp process management,
//! test webhook receiver, and test orchestrator.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use serde_json::Value;
use tokio::sync::{OnceCell, RwLock};

use crate::world::{OrchestratorInvocation, ReceivedEvent};

pub use bdd_infra::ServiceHandle;

// ---------------------------------------------------------------------------
// OmniSim native process handle (shared singleton)
// ---------------------------------------------------------------------------

/// OmniSim's default HTTP port when launched without `--urls`.
const OMNISIM_PORT: u16 = 32323;

/// Shared OmniSim info returned to each scenario
#[derive(Debug, Clone)]
pub struct OmniSimHandle {
    pub base_url: String,
    pub port: u16,
}

/// Singleton that owns the OmniSim child process for the entire test run.
/// When `None`, a pre-existing OmniSim was reused and we don't own the process.
struct OmniSimProcess {
    _child: Option<std::process::Child>,
    base_url: String,
    port: u16,
}

/// Global singleton — one OmniSim process shared by all scenarios
static OMNISIM: OnceCell<OmniSimProcess> = OnceCell::const_new();

impl OmniSimHandle {
    /// Get or start the shared OmniSim process. Returns a lightweight handle.
    ///
    /// If an OmniSim instance is already listening on the default port (32323),
    /// it is reused. Otherwise a new process is spawned with
    /// `PR_SET_PDEATHSIG` so the kernel kills it when the test process exits.
    ///
    /// Binary discovery order (when spawning):
    /// 1. `OMNISIM_PATH` env var — full path to the binary
    /// 2. `OMNISIM_DIR` env var — directory containing the binary
    /// 3. `ascom.alpaca.simulators` on `PATH`
    pub async fn start() -> Self {
        let process = OMNISIM
            .get_or_init(|| async { OmniSimProcess::get_or_spawn().await })
            .await;
        Self {
            base_url: process.base_url.clone(),
            port: process.port,
        }
    }
}

impl OmniSimProcess {
    async fn get_or_spawn() -> Self {
        let base_url = format!("http://127.0.0.1:{}", OMNISIM_PORT);

        // Reuse an already-running OmniSim if it responds to a health check.
        if Self::is_healthy(&base_url).await {
            return Self {
                _child: None,
                base_url,
                port: OMNISIM_PORT,
            };
        }

        let binary = Self::find_binary();

        // Clear sanitizer-related env vars so the .NET runtime isn't broken
        // by LD_PRELOAD injection from ASAN/LSAN.
        let mut cmd = std::process::Command::new(&binary);
        cmd.stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_remove("LD_PRELOAD")
            .env_remove("ASAN_OPTIONS")
            .env_remove("LSAN_OPTIONS");

        // On Linux, set PR_SET_PDEATHSIG so the kernel will SIGKILL this
        // child when the test process exits (normal, panic, or SIGKILL).
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                    Ok(())
                });
            }
        }

        let child = cmd
            .spawn()
            .unwrap_or_else(|e| panic!("failed to start OmniSim binary '{}': {}", binary, e));

        let process = Self {
            _child: Some(child),
            base_url,
            port: OMNISIM_PORT,
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

    /// Single health-check probe — returns true if OmniSim is responding.
    async fn is_healthy(base_url: &str) -> bool {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("failed to build reqwest client");
        let url = format!("{}/api/v1/camera/0/connected", base_url);
        matches!(client.get(&url).send().await, Ok(resp) if resp.status().is_success())
    }

    /// Wait for OmniSim to respond to HTTP requests
    async fn wait_healthy(&self) {
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if Self::is_healthy(&self.base_url).await {
                return;
            }
        }
        panic!("OmniSim did not become healthy within 30 seconds");
    }
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
#[derive(Debug, Clone, Default)]
pub enum OrchestratorBehavior {
    /// Complete immediately after being invoked
    #[default]
    CompleteImmediately,
    /// Wait until explicitly stopped (via cancellation)
    WaitForStop,
    /// Run a flat calibration plan: Vec<(filter, count)>
    FlatCalibration(Vec<(String, u32)>),
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

/// Drive a flat calibration workflow via MCP tool calls.
///
/// Uses `duration_ms` for exposure times. The test uses a fixed 100ms
/// duration since OmniSim produces instant images regardless of duration.
async fn run_flat_calibration(mcp_server_url: &str, workflow_id: &str, plan: &[(String, u32)]) {
    let base_url = mcp_server_url.trim_end_matches("/mcp");
    let client = reqwest::Client::new();

    for (filter, count) in plan {
        // Set filter
        let _set_filter = client
            .post(format!("{}/mcp", base_url))
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
                .post(format!("{}/mcp", base_url))
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "capture",
                        "arguments": {
                            "camera_id": "main-cam",
                            "duration_ms": 100
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
