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
        let mcp = match McpClient::new(&mcp_url, plan_clone.rp_auth(), plan_clone.rp_ca()).await {
            Ok(c) => c,
            Err(e) => {
                warn!(workflow_id = %wf_id, error = %e, "failed to connect MCP client");
                post_failure(&mcp_url, &wf_id, &e.to_string(), &plan_clone).await;
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
                post_completion(&mcp_url, &wf_id, &result, &plan_clone).await;
            }
            Err(e) => {
                warn!(workflow_id = %wf_id, error = %e, "flat calibration failed");
                post_failure(&mcp_url, &wf_id, &e.to_string(), &plan_clone).await;
            }
        }
    });

    // Acknowledge with timing estimate. Saturating arithmetic so a pathological
    // config (e.g. `initial_duration: "1000d"` × thousands of frames) cannot
    // crash the service while building the ack.
    let total_frames: u32 = plan.filters.iter().map(|f| f.count).sum();
    let estimated = plan
        .initial_duration
        .saturating_mul(total_frames)
        .saturating_add(std::time::Duration::from_secs(60));

    let ack = serde_json::json!({
        "estimated_duration": humantime::format_duration(estimated).to_string(),
        "max_duration": humantime::format_duration(estimated.saturating_mul(2)).to_string(),
    });

    (StatusCode::OK, Json(ack))
}

async fn post_completion(
    mcp_server_url: &str,
    workflow_id: &str,
    result: &workflow::WorkflowResult,
    plan: &FlatPlan,
) {
    let base_url = mcp_server_url.trim_end_matches("/mcp");
    let url = format!("{}/api/plugins/{}/complete", base_url, workflow_id);

    let filters: Vec<Value> = result
        .filters_completed
        .iter()
        .map(|f| {
            serde_json::json!({
                "filter": f.filter_name,
                "duration": humantime::format_duration(f.duration).to_string(),
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

    post_to_rp(&url, &body, plan).await;
}

async fn post_failure(mcp_server_url: &str, workflow_id: &str, error: &str, plan: &FlatPlan) {
    let base_url = mcp_server_url.trim_end_matches("/mcp");
    let url = format!("{}/api/plugins/{}/complete", base_url, workflow_id);

    let body = serde_json::json!({
        "status": "error",
        "result": {
            "reason": "flat_calibration_failed",
            "error": error,
        }
    });

    post_to_rp(&url, &body, plan).await;
}

/// POST a completion body to `rp`, trusting and authenticating per the
/// ADR-017 policy — the same legs the MCP client uses.
async fn post_to_rp(url: &str, body: &Value, plan: &FlatPlan) {
    let client = match rusty_photon_tls::client::build_reqwest_client(plan.rp_ca()) {
        Ok(client) => client,
        Err(e) => {
            warn!(%url, error = %e, "cannot build HTTP client for the completion post");
            return;
        }
    };
    let auth_header = rp_mcp_client::basic_authorization(url, plan.rp_auth(), plan.rp_ca())
        .unwrap_or_else(|e| {
            warn!(%url, error = %e, "cannot build the completion Authorization header");
            None
        });
    let mut request = client.post(url).json(body);
    if let Some(header) = auth_header {
        request = request.header(reqwest::header::AUTHORIZATION, header);
    }
    let _ = request.send().await;
}
