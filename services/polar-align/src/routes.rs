//! HTTP routes: POST /invoke for orchestrator invocation, GET /status
//! for the live alignment state, POST /adjust/finish to end the
//! adjustment loop.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::config::PolarAlignConfig;
use crate::mcp_client::McpClient;
use crate::workflow::{self, Phase, WorkflowShared};

#[derive(Clone)]
pub struct AppState {
    pub config: PolarAlignConfig,
    pub shared: WorkflowShared,
}

pub fn build_router(config: PolarAlignConfig) -> Router {
    let state = AppState {
        config,
        shared: WorkflowShared::default(),
    };
    Router::new()
        .route("/health", get(health))
        .route("/invoke", post(invoke_handler))
        .route("/status", get(status_handler))
        .route("/adjust/finish", post(finish_handler))
        .with_state(state)
}

async fn health() -> &'static str {
    "polar-align healthy"
}

async fn status_handler(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let status = state.shared.status.read().await;
    match serde_json::to_value(&*status) {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(e) => {
            warn!(error = %e, "status serialization failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "status serialization failed" })),
            )
        }
    }
}

async fn finish_handler(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let phase = state.shared.status.read().await.phase;
    if phase != Phase::Adjusting {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("no adjustment in progress (phase {:?})", phase)
            })),
        );
    }
    state.shared.signal_finish().await;
    (StatusCode::ACCEPTED, Json(serde_json::json!({})))
}

async fn invoke_handler(
    State(state): State<AppState>,
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

    // Reject before reserving the workflow slot: a malformed request
    // must not flip the service into an error state.
    if workflow_id.is_empty() || mcp_server_url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "workflow_id and mcp_server_url are required"
            })),
        );
    }

    debug!(
        workflow_id = %workflow_id,
        mcp_server_url = %mcp_server_url,
        "received invocation"
    );

    // Reserve the single workflow slot before spawning: a second
    // /invoke while one alignment runs is meaningless (one mount, one
    // camera) and is rejected with 409.
    {
        let mut status = state.shared.status.write().await;
        if matches!(status.phase, Phase::Measuring | Phase::Adjusting) {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "a polar-alignment workflow is already running"
                })),
            );
        }
        *status = workflow::StatusState {
            phase: Phase::Measuring,
            workflow_id: Some(workflow_id.clone()),
            ..Default::default()
        };
    }
    // A `/adjust/finish` that raced the previous run's end may have
    // left a permit on the old signal; a fresh one discards it.
    state.shared.arm_finish().await;

    let wf_id = workflow_id.clone();
    let mcp_url = mcp_server_url.clone();
    let state_clone = state.clone();

    tokio::spawn(async move {
        let config = &state_clone.config;
        let mcp = match McpClient::new(&mcp_url, config.rp_auth(), config.rp_ca()).await {
            Ok(c) => c,
            Err(e) => {
                warn!(workflow_id = %wf_id, error = %e, "failed to connect MCP client");
                {
                    let mut status = state_clone.shared.status.write().await;
                    status.phase = Phase::Error;
                    status.error = Some(e.to_string());
                }
                post_failure(&mcp_url, &wf_id, &e.to_string(), config).await;
                return;
            }
        };

        match workflow::run(&mcp, config, &state_clone.shared).await {
            Ok(summary) => {
                info!(
                    workflow_id = %wf_id,
                    azimuth_error_arcmin = summary.final_measurement.azimuth_error_arcmin,
                    altitude_error_arcmin = summary.final_measurement.altitude_error_arcmin,
                    iterations = summary.adjustment_iterations,
                    "polar alignment completed"
                );
                post_completion(&mcp_url, &wf_id, &summary, config).await;
            }
            Err(e) => {
                post_failure(&mcp_url, &wf_id, &e.to_string(), config).await;
            }
        }
    });

    // Acknowledge with timing estimates: three slew/capture/solve
    // legs plus the adjustment window. Saturating arithmetic so a
    // pathological config cannot crash the ack.
    let per_point = state
        .config
        .measurement
        .exposure
        .saturating_add(state.config.measurement.settle)
        .saturating_add(std::time::Duration::from_secs(30));
    let measurement = per_point.saturating_mul(3);
    let estimated = measurement.saturating_add(state.config.adjustment.max_duration / 2);
    let max = measurement
        .saturating_add(state.config.adjustment.max_duration)
        .saturating_add(std::time::Duration::from_secs(60));

    let ack = serde_json::json!({
        "estimated_duration": humantime::format_duration(estimated).to_string(),
        "max_duration": humantime::format_duration(max).to_string(),
    });

    (StatusCode::OK, Json(ack))
}

async fn post_completion(
    mcp_server_url: &str,
    workflow_id: &str,
    summary: &workflow::WorkflowSummary,
    config: &PolarAlignConfig,
) {
    let body = serde_json::json!({
        "status": "complete",
        "result": {
            "reason": "polar_alignment_complete",
            "azimuth_error_arcmin": summary.final_measurement.azimuth_error_arcmin,
            "altitude_error_arcmin": summary.final_measurement.altitude_error_arcmin,
            "total_error_arcmin": summary.final_measurement.total_error_arcmin,
            "adjustment_iterations": summary.adjustment_iterations,
        }
    });
    post_to_rp(mcp_server_url, workflow_id, &body, config).await;
}

async fn post_failure(
    mcp_server_url: &str,
    workflow_id: &str,
    error: &str,
    config: &PolarAlignConfig,
) {
    let body = serde_json::json!({
        "status": "error",
        "result": {
            "reason": "polar_alignment_failed",
            "error": error,
        }
    });
    post_to_rp(mcp_server_url, workflow_id, &body, config).await;
}

/// The rp API base for an MCP endpoint URL, tolerating a trailing
/// slash (an operator-configured advertised URL may carry one).
fn rp_base_url(mcp_server_url: &str) -> &str {
    let trimmed = mcp_server_url.trim_end_matches('/');
    trimmed.strip_suffix("/mcp").unwrap_or(trimmed)
}

/// POST a completion body to `rp`, trusting and authenticating per
/// the ADR-017 policy — the same legs the MCP client uses.
async fn post_to_rp(
    mcp_server_url: &str,
    workflow_id: &str,
    body: &Value,
    config: &PolarAlignConfig,
) {
    let base_url = rp_base_url(mcp_server_url);
    let url = format!("{}/api/plugins/{}/complete", base_url, workflow_id);

    let client = match rusty_photon_tls::client::build_reqwest_client(config.rp_ca()) {
        Ok(client) => client,
        Err(e) => {
            warn!(%url, error = %e, "cannot build HTTP client for the completion post");
            return;
        }
    };
    let auth_header = rp_mcp_client::basic_authorization(&url, config.rp_auth(), config.rp_ca())
        .unwrap_or_else(|e| {
            warn!(%url, error = %e, "cannot build the completion Authorization header");
            None
        });
    let mut request = client.post(&url).json(body);
    if let Some(header) = auth_header {
        request = request.header(reqwest::header::AUTHORIZATION, header);
    }
    let _ = request.send().await;
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_rp_base_url_strips_the_mcp_suffix_with_and_without_a_trailing_slash() {
        assert_eq!(rp_base_url("https://rig:11170/mcp"), "https://rig:11170");
        assert_eq!(rp_base_url("https://rig:11170/mcp/"), "https://rig:11170");
        assert_eq!(rp_base_url("https://rig:11170"), "https://rig:11170");
    }
}
