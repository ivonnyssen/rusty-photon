#![allow(dead_code)]
//! Test infrastructure: OmniSim Docker management, rp process management,
//! test webhook receiver, and test orchestrator.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::world::{OrchestratorInvocation, ReceivedEvent};

// ---------------------------------------------------------------------------
// OmniSim Docker handle
// ---------------------------------------------------------------------------

/// Handle to a running OmniSim Docker container
#[derive(Debug)]
pub struct OmniSimHandle {
    pub container_id: String,
    pub base_url: String,
}

impl OmniSimHandle {
    /// Start OmniSim via `docker run`. Returns once the container is healthy.
    pub async fn start() -> Self {
        // Check if OmniSim is already running (reuse across scenarios)
        let output = tokio::process::Command::new("docker")
            .args(["ps", "-q", "-f", "name=rp-test-omnisim"])
            .output()
            .await
            .expect("failed to run docker ps");

        let existing_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if !existing_id.is_empty() {
            let handle = Self {
                container_id: existing_id,
                base_url: "http://localhost:32323".to_string(),
            };
            handle.wait_healthy().await;
            return handle;
        }

        let output = tokio::process::Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                "rp-test-omnisim",
                "-p",
                "32323:32323",
                "ghcr.io/ascominitiative/ascom-alpaca-simulators:latest",
            ])
            .output()
            .await
            .expect("failed to start OmniSim container");

        assert!(
            output.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let handle = Self {
            container_id,
            base_url: "http://localhost:32323".to_string(),
        };

        handle.wait_healthy().await;
        handle
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

// We intentionally do NOT stop the container in Drop — it is reused
// across scenarios for speed. A cleanup script or CI job handles removal.

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
}

impl RpHandle {
    /// Start rp via `cargo run` with the given config file
    pub async fn start(config_path: &str, port: u16) -> Self {
        let child = tokio::process::Command::new("cargo")
            .args([
                "run",
                "--package",
                "rp",
                "--quiet",
                "--",
                "--config",
                config_path,
            ])
            .kill_on_drop(true)
            .spawn()
            .expect("failed to start rp process");

        Self {
            child: Some(child),
            base_url: format!("http://127.0.0.1:{}", port),
            port,
            config_path: config_path.to_string(),
        }
    }

    /// Stop the rp process
    pub async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

impl Drop for RpHandle {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            // Best-effort kill on drop — async kill not possible in Drop
            let _ = child.start_kill();
        }
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
