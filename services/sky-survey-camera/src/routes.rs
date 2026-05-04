use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::camera::DeviceState;
use crate::pointing::PointingSource;

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

#[derive(Debug, Serialize)]
struct ConflictBody {
    error: &'static str,
    reason: &'static str,
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
    // F6: returns the most recently snapshotted pointing in either
    // mode. In static mode this is the value updated by POST; in
    // follow mode it's the last successful mount read (written
    // before the SkyView fetch, so it reflects mount state even if
    // a later exposure step fails).
    let snap = state.last_snapshot.snapshot().await;
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
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    if !state.connected.load(Ordering::Acquire) {
        // P6: 409 when disconnected.
        return (
            StatusCode::CONFLICT,
            Json(ConflictBody {
                error: "not_connected",
                reason: "device must be connected to accept position writes",
            }),
        )
            .into_response();
    }

    // F6: 409 in follow mode — any value written here would be silently
    // overwritten by the next mount read.
    let static_pointing = match &state.pointing_source {
        PointingSource::Static(p) => p,
        PointingSource::Telescope(_) => {
            return (
                StatusCode::CONFLICT,
                Json(ConflictBody {
                    error: "follow_mode",
                    reason: "pointing.telescope is configured; pointing is read from the mount",
                }),
            )
                .into_response();
        }
    };

    match static_pointing
        .update(req.ra_deg, req.dec_deg, req.rotation_deg)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}
