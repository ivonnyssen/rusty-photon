use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use tracing::debug;

use crate::equipment::EquipmentRegistry;
use crate::mcp::McpHandler;
use crate::session::SessionManager;

#[derive(Clone)]
pub struct AppState {
    pub equipment: Arc<EquipmentRegistry>,
    pub mcp: Arc<McpHandler>,
    pub session: Arc<SessionManager>,
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/equipment", get(get_equipment))
        .route("/mcp", post(mcp_handler))
        .route("/api/session/start", post(session_start))
        .route("/api/session/stop", post(session_stop))
        .route("/api/session/status", get(session_status))
        .route(
            "/api/plugins/{workflow_id}/complete",
            post(workflow_complete),
        )
        .with_state(state)
}

async fn health() -> &'static str {
    "Hello World, I am healthy!"
}

async fn get_equipment(State(state): State<AppState>) -> Json<Value> {
    let status = state.equipment.status();
    Json(serde_json::to_value(status).unwrap_or_default())
}

async fn mcp_handler(State(state): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    debug!("received MCP request");
    let result = state.mcp.handle_request(body).await;
    Json(result)
}

async fn session_start(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match state.session.start().await {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err(e) => (StatusCode::CONFLICT, Json(serde_json::json!({"error": e}))),
    }
}

async fn session_stop(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match state.session.stop().await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "stopped"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        ),
    }
}

async fn session_status(State(state): State<AppState>) -> Json<Value> {
    let status = state.session.status().await;
    Json(serde_json::json!({"status": status}))
}

async fn workflow_complete(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> StatusCode {
    debug!(workflow_id = %workflow_id, "received workflow completion");
    state.session.workflow_complete(&workflow_id).await;
    StatusCode::OK
}
