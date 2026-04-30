//! In-process HTTP server that acts as an rp orchestrator plugin.
//!
//! Configurable via [`OrchestratorBehavior`] to model different workflow
//! shapes (complete immediately, wait for cancellation, run a flat
//! calibration plan end-to-end via MCP).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use serde_json::Value;
use tokio::sync::RwLock;

use super::mcp_client::McpTestClient;

/// A single orchestrator invocation captured by [`TestOrchestrator`].
#[derive(Debug, Clone)]
pub struct OrchestratorInvocation {
    pub workflow_id: String,
    pub session_id: String,
    pub mcp_server_url: String,
    pub recovery: Option<Value>,
}

/// Configurable behavior for the test orchestrator.
#[derive(Debug, Clone, Default)]
pub enum OrchestratorBehavior {
    /// Complete immediately after being invoked.
    #[default]
    CompleteImmediately,
    /// Wait until explicitly stopped (via cancellation).
    WaitForStop,
    /// Run a flat calibration plan: `Vec<(filter, count)>`.
    FlatCalibration(Vec<(String, u32)>),
}

#[derive(Debug, Clone)]
struct TestOrchestratorState {
    invocations: Arc<RwLock<Vec<OrchestratorInvocation>>>,
    cancelled: Arc<RwLock<bool>>,
    behavior: OrchestratorBehavior,
}

/// In-process HTTP server that acts as an orchestrator plugin.
#[derive(Debug)]
pub struct TestOrchestrator {
    pub invoke_url: String,
    pub port: u16,
    pub invocations: Arc<RwLock<Vec<OrchestratorInvocation>>>,
    pub cancelled: Arc<RwLock<bool>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TestOrchestrator {
    /// Start the test orchestrator on a random port.
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

    let mcp_server_url = invocation.mcp_server_url.clone();
    let workflow_id = invocation.workflow_id.clone();
    let cancelled = state.cancelled.clone();
    let behavior = state.behavior.clone();

    tokio::spawn(async move {
        match behavior {
            OrchestratorBehavior::CompleteImmediately => {
                post_completion(&mcp_server_url, &workflow_id).await;
            }
            OrchestratorBehavior::WaitForStop => loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if *cancelled.read().await {
                    break;
                }
            },
            OrchestratorBehavior::FlatCalibration(plan) => {
                run_flat_calibration(&mcp_server_url, &workflow_id, &plan).await;
            }
        }
    });

    let ack = serde_json::json!({
        "estimated_duration": "5m",
        "max_duration": "0s"
    });

    (StatusCode::OK, axum::Json(ack))
}

/// Post orchestrator completion back to rp.
async fn post_completion(mcp_server_url: &str, workflow_id: &str) {
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

    // Best-effort — if rp is down, this will just fail silently.
    let _ = client.post(&url).json(&body).send().await;
}

/// Drive a flat calibration workflow via MCP tool calls.
///
/// Uses a fixed 100ms exposure since OmniSim produces instant images
/// regardless of duration.
async fn run_flat_calibration(mcp_server_url: &str, workflow_id: &str, plan: &[(String, u32)]) {
    let client = McpTestClient::connect(mcp_server_url)
        .await
        .expect("failed to connect MCP client for flat calibration");

    for (filter, count) in plan {
        let _ = client
            .call_tool(
                "set_filter",
                serde_json::json!({
                    "filter_wheel_id": "main-fw",
                    "filter_name": filter
                }),
            )
            .await;

        for _ in 0..*count {
            let _ = client
                .call_tool(
                    "capture",
                    serde_json::json!({
                        "camera_id": "main-cam",
                        "duration": "100ms"
                    }),
                )
                .await;
        }
    }

    post_completion(mcp_server_url, workflow_id).await;
}
