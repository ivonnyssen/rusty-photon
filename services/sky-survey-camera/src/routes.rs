use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::camera::DeviceState;

#[derive(Debug, Serialize)]
struct PositionResponse {
    ra_deg: f64,
    dec_deg: f64,
    rotation_deg: f64,
}

#[derive(Debug, Deserialize)]
struct PositionRequest {
    ra_deg: f64,
    dec_deg: f64,
    #[serde(default)]
    rotation_deg: Option<f64>,
}

pub fn build_router(state: Arc<DeviceState>) -> Router {
    Router::new()
        .route(
            "/sky-survey/position",
            get(get_position).post(post_position),
        )
        .with_state(state)
}

async fn get_position(State(state): State<Arc<DeviceState>>) -> impl IntoResponse {
    let snap = state.pointing.snapshot();
    Json(PositionResponse {
        ra_deg: snap.ra_deg,
        dec_deg: snap.dec_deg,
        rotation_deg: snap.rotation_deg,
    })
}

async fn post_position(
    State(state): State<Arc<DeviceState>>,
    body: Result<Json<PositionRequest>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    let Json(req) = match body {
        Ok(json) => json,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    if !state.connected.load(Ordering::Acquire) {
        return StatusCode::CONFLICT;
    }

    match state
        .pointing
        .update(req.ra_deg, req.dec_deg, req.rotation_deg)
    {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::BAD_REQUEST,
    }
}
