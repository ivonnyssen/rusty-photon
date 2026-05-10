use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::camera::DeviceState;
use crate::pointing::{PointingSource, PointingState};

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

    // F7: in follow mode, POST arms a one-shot override that the next
    // light `StartExposure` consumes — letting test harnesses inject
    // "the camera saw something different from where the mount thinks
    // it is" without requiring the mount itself to lie about its
    // position. The static-mode write path is unchanged.
    match &state.pointing_source {
        PointingSource::Static(p) => {
            match p.update(req.ra_deg, req.dec_deg, req.rotation_deg).await {
                Ok(_) => StatusCode::NO_CONTENT.into_response(),
                Err(_) => StatusCode::BAD_REQUEST.into_response(),
            }
        }
        PointingSource::Telescope(_) => {
            // Validate ra/dec/rotation by routing through the
            // SharedPointing validator. We don't actually mutate
            // `last_snapshot` here — the next exposure will overwrite
            // it from either the override or a fresh mount read — but
            // we want identical 400-on-bad-input semantics with
            // static mode (P4/P5).
            let validated = match validate_position(
                req.ra_deg,
                req.dec_deg,
                req.rotation_deg,
                &state.last_snapshot.snapshot().await,
            ) {
                Ok(v) => v,
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            };
            let mut guard = state
                .next_pointing_override
                .lock()
                .expect("next_pointing_override poisoned");
            *guard = Some(validated);
            StatusCode::NO_CONTENT.into_response()
        }
    }
}

/// Apply the same input rules as [`SharedPointing::update`] without
/// touching shared state — used by the follow-mode `POST` path that
/// arms a one-shot override instead of mutating `last_snapshot`.
fn validate_position(
    ra_deg: f64,
    dec_deg: f64,
    rotation_deg: Option<f64>,
    fallback_rotation: &PointingState,
) -> Result<PointingState, ()> {
    if !ra_deg.is_finite() || !(0.0..360.0).contains(&ra_deg) {
        return Err(());
    }
    if !dec_deg.is_finite() || !(-90.0..=90.0).contains(&dec_deg) {
        return Err(());
    }
    if let Some(r) = rotation_deg {
        if !r.is_finite() {
            return Err(());
        }
    }
    let rot = rotation_deg.unwrap_or(fallback_rotation.rotation_deg);
    Ok(PointingState::new(ra_deg, dec_deg, rot))
}
