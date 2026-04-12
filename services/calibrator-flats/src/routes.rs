//! HTTP routes: POST /invoke for orchestrator invocation.

use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::config::FlatPlan;
use crate::mcp_client::McpClient;
use crate::workflow;

pub fn build_router(plan: FlatPlan) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/invoke", post(invoke_handler))
        .with_state(plan)
}

async fn health() -> &'static str {
    "calibrator-flats healthy"
}

async fn invoke_handler(
    axum::extract::State(plan): axum::extract::State<FlatPlan>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let workflow_id = body
        .get("workflow_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mcp_server_url = body
        .get("mcp_server_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    debug!(
        workflow_id = %workflow_id,
        mcp_server_url = %mcp_server_url,
        "received invocation"
    );

    // Spawn the workflow in the background so we can acknowledge immediately
    let wf_id = workflow_id.clone();
    let mcp_url = mcp_server_url.clone();
    let plan_clone = plan.clone();

    tokio::spawn(async move {
        let mcp = match McpClient::new(&mcp_url).await {
            Ok(c) => c,
            Err(e) => {
                warn!(workflow_id = %wf_id, error = %e, "failed to connect MCP client");
                post_failure(&mcp_url, &wf_id, &e.to_string()).await;
                return;
            }
        };

        match workflow::run(&mcp, &plan_clone).await {
            Ok(result) => {
                info!(
                    workflow_id = %wf_id,
                    total_frames = result.total_frames,
                    "flat calibration completed"
                );
                post_completion(&mcp_url, &wf_id, &result).await;
            }
            Err(e) => {
                warn!(workflow_id = %wf_id, error = %e, "flat calibration failed");
                post_failure(&mcp_url, &wf_id, &e.to_string()).await;
            }
        }
    });

    // Acknowledge with timing estimate
    let total_frames: u32 = plan.filters.iter().map(|f| f.count).sum();
    let estimated_secs = (total_frames as u64 * plan.initial_duration_ms as u64) / 1000 + 60; // add margin for iterations

    let ack = serde_json::json!({
        "estimated_duration_secs": estimated_secs,
        "max_duration_secs": estimated_secs * 2,
    });

    (StatusCode::OK, Json(ack))
}

async fn post_completion(
    mcp_server_url: &str,
    workflow_id: &str,
    result: &workflow::WorkflowResult,
) {
    let base_url = mcp_server_url.trim_end_matches("/mcp");
    let url = format!("{}/api/plugins/{}/complete", base_url, workflow_id);

    let filters: Vec<Value> = result
        .filters_completed
        .iter()
        .map(|f| {
            serde_json::json!({
                "filter": f.filter_name,
                "duration_ms": f.duration_ms,
                "median_adu": f.median_adu,
                "frames": f.frames_captured,
                "converged": f.converged,
            })
        })
        .collect();

    let body = serde_json::json!({
        "status": "complete",
        "result": {
            "reason": "flat_calibration_complete",
            "filters_completed": filters,
            "total_frames": result.total_frames,
        }
    });

    let client = reqwest::Client::new();
    let _ = client.post(&url).json(&body).send().await;
}

async fn post_failure(mcp_server_url: &str, workflow_id: &str, error: &str) {
    let base_url = mcp_server_url.trim_end_matches("/mcp");
    let url = format!("{}/api/plugins/{}/complete", base_url, workflow_id);

    let body = serde_json::json!({
        "status": "error",
        "result": {
            "reason": "flat_calibration_failed",
            "error": error,
        }
    });

    let client = reqwest::Client::new();
    let _ = client.post(&url).json(&body).send().await;
}
