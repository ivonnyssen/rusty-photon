//! In-process HTTP stub of the `plate-solver` rp-managed service.
//!
//! Spawned by rp's BDD scenarios so the `plate_solve` MCP tool's HTTP
//! client (`rp-plate-solver`) has something to talk to without
//! standing up the real `plate-solver` process. Records each
//! incoming request body so subsequent `Then` steps can assert on
//! hint plumbing, and returns either a canned WCS payload or a
//! structured error envelope based on the configured behavior.
//!
//! The wire shape mirrors `services/plate-solver/src/api.rs` and
//! `services/plate-solver/src/error.rs` exactly — see
//! `docs/services/plate-solver.md` §"HTTP API". The stub does not
//! depend on the `plate-solver` crate (Tenet 4: remote interfaces
//! only) — the duplication is the point.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use tokio::sync::RwLock;

/// One canned 200 success body. Used as the payload for both the
/// single-shot `CannedWcs` variant (where every solve call returns
/// the same body) and the `Sequence` variant (where successive
/// solve calls walk the queue).
#[derive(Debug, Clone)]
pub struct CannedWcs {
    pub ra_center: f64,
    pub dec_center: f64,
    pub pixel_scale_arcsec: f64,
    pub rotation_deg: f64,
    pub solver: String,
}

/// Canned response variants the stub can return for `POST /api/v1/solve`.
#[derive(Debug, Clone)]
pub enum StubBehavior {
    /// Return a fixed 200 success body for every call.
    Canned(CannedWcs),
    /// Walk a queue of WCS responses, one per call. After the last
    /// queued entry is consumed, subsequent calls keep returning the
    /// final entry — this lets `center_on_target` tests express
    /// "first iter outside tolerance, then converged" without having
    /// to count the exact number of solves the loop will make.
    /// `responses` must be non-empty.
    Sequence(Vec<CannedWcs>),
    /// Return a structured error envelope. `code` matches the
    /// wrapper's `ErrorCode` (`invalid_request`, `fits_not_found`,
    /// `solve_failed`, `solve_timeout`, `internal`); the appropriate
    /// HTTP status is selected per the wrapper's mapping.
    Error { code: String, message: String },
}

impl StubBehavior {
    /// Default canned WCS used by happy-path scenarios: RA 10.6848°
    /// (≈ M31), Dec 41.269°, 1.05"/px scale, 12.3° rotation.
    pub fn default_canned_wcs() -> Self {
        Self::Canned(CannedWcs {
            ra_center: 10.6848,
            dec_center: 41.269,
            pixel_scale_arcsec: 1.05,
            rotation_deg: 12.3,
            solver: "stub-astap-1.0".to_string(),
        })
    }
}

/// In-process axum stub of `plate-solver`. Hold the handle alive for
/// the scenario duration; on drop the listener task is cancelled.
#[derive(Debug)]
pub struct PlateSolverStub {
    pub url: String,
    pub port: u16,
    requests: Arc<RwLock<Vec<Value>>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

#[derive(Clone)]
struct StubState {
    behavior: StubBehavior,
    requests: Arc<RwLock<Vec<Value>>>,
    /// Cursor into `Sequence` behavior's response queue — incremented
    /// per call, clamped to the last entry once exhausted. Unused for
    /// the other variants. `Arc<...>` so cloning the State (axum does
    /// it once per request) shares the cursor across handlers.
    sequence_cursor: Arc<std::sync::atomic::AtomicUsize>,
}

impl PlateSolverStub {
    /// Spawn an axum router on `127.0.0.1:0` returning the configured
    /// canned response for every `POST /api/v1/solve`.
    pub async fn start(behavior: StubBehavior) -> Self {
        if let StubBehavior::Sequence(responses) = &behavior {
            assert!(
                !responses.is_empty(),
                "PlateSolverStub::start: Sequence must have at least one response"
            );
        }
        let requests: Arc<RwLock<Vec<Value>>> = Arc::new(RwLock::new(Vec::new()));
        let state = StubState {
            behavior,
            requests: requests.clone(),
            sequence_cursor: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        let app = Router::new()
            .route("/api/v1/solve", post(solve_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind plate solver stub");
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{}", port);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("plate solver stub failed");
        });

        Self {
            url,
            port,
            requests,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// All request bodies the stub has received, in order.
    pub async fn requests(&self) -> Vec<Value> {
        self.requests.read().await.clone()
    }

    /// Stop the stub server.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for PlateSolverStub {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn solve_handler(
    State(state): State<StubState>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    state.requests.write().await.push(body);

    match state.behavior {
        StubBehavior::Canned(ref wcs) => canned_response(wcs),
        StubBehavior::Sequence(ref responses) => {
            // Walk the queue, clamping at the final entry.
            let idx = state
                .sequence_cursor
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .min(responses.len() - 1);
            canned_response(&responses[idx])
        }
        StubBehavior::Error {
            ref code,
            ref message,
        } => {
            let status = match code.as_str() {
                "invalid_request" => StatusCode::BAD_REQUEST,
                "fits_not_found" => StatusCode::NOT_FOUND,
                "solve_failed" => StatusCode::UNPROCESSABLE_ENTITY,
                "solve_timeout" => StatusCode::GATEWAY_TIMEOUT,
                "internal" => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            let body = Json(serde_json::json!({
                "error": code,
                "message": message,
            }));
            (status, body).into_response()
        }
    }
}

fn canned_response(wcs: &CannedWcs) -> axum::response::Response {
    Json(serde_json::json!({
        "ra_center": wcs.ra_center,
        "dec_center": wcs.dec_center,
        "pixel_scale_arcsec": wcs.pixel_scale_arcsec,
        "rotation_deg": wcs.rotation_deg,
        "solver": wcs.solver,
    }))
    .into_response()
}
