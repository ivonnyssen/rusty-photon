//! In-process HTTP stub of the guider rp-managed service.
//!
//! Spawned by rp's BDD scenarios so the guiding MCP tools' HTTP
//! client (`rp-guider`) has something to talk to without standing up
//! the real `phd2-guider serve` process (whose own contract is
//! covered by its self-contained BDD suite against `mock_phd2`).
//! Records each incoming request as `(path, body)` so subsequent
//! `Then` steps can assert on settle/dither plumbing, and returns
//! either canned guiding responses or a structured error envelope
//! based on the configured behavior.
//!
//! The wire shape mirrors `services/phd2-guider/src/service/api.rs`
//! and `.../error.rs` exactly — see `docs/services/phd2-guider.md`
//! § "HTTP Service Mode". The stub does not depend on the
//! `phd2-guider` crate (Tenet 4: remote interfaces only) — the
//! duplication is the point.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use tokio::sync::RwLock;

/// Canned guiding numbers used for both the settle responses
/// (`guiding/start`, `dither`) and the `guiding/stats` body. The
/// defaults are the Pythagorean triple the `mock_phd2` fixture also
/// produces (0.3 / 0.4 / 0.5), so numbers stay recognizable across
/// the two test layers.
#[derive(Debug, Clone)]
pub struct CannedGuiding {
    pub rms_ra_px: f64,
    pub rms_dec_px: f64,
    pub total_rms_px: f64,
    pub sample_count: u32,
    /// Star SNR reported by `guiding/stats`.
    pub snr: f64,
    /// Star mass reported by `guiding/stats`.
    pub star_mass: f64,
    /// Whether `guiding/stats` reports an active guiding loop
    /// (`guiding: true`, `app_state: "Guiding"`). `false` reports a
    /// stopped guider (`app_state: "Stopped"`) — the state
    /// refocus_train's pause/resume handshake checks before pausing.
    pub guiding: bool,
    /// Delay applied before answering the settle-blocking endpoints
    /// (`guiding/start`, `dither`). Zero (the default) answers
    /// immediately; rp's motion-gate scenarios use a non-zero delay
    /// to hold a dither in flight while a concurrent capture is
    /// issued.
    pub settle_delay: std::time::Duration,
}

impl Default for CannedGuiding {
    fn default() -> Self {
        Self {
            rms_ra_px: 0.3,
            rms_dec_px: 0.4,
            total_rms_px: 0.5,
            sample_count: 12,
            snr: 25.0,
            star_mass: 5432.0,
            guiding: true,
            settle_delay: std::time::Duration::ZERO,
        }
    }
}

/// Canned response variants the stub can return.
#[derive(Debug, Clone)]
pub enum GuiderStubBehavior {
    /// Every endpoint succeeds: settle-blocking calls return the
    /// canned RMS snapshot, state-only calls return their state, and
    /// stats reports `app_state: "Guiding"`.
    Canned(CannedGuiding),
    /// Every endpoint returns a structured error envelope. `code`
    /// matches the service's `ErrorCode` (`invalid_request`,
    /// `not_guiding`, `guide_failed`, `settle_timeout`,
    /// `stop_timeout`, `phd2_unreachable`, `internal`); the
    /// appropriate HTTP status is selected per the service's mapping.
    Error { code: String, message: String },
}

/// In-process axum stub of the guider service. Hold the handle alive
/// for the scenario duration; on drop the listener task is cancelled.
#[derive(Debug)]
pub struct GuiderStub {
    pub url: String,
    pub port: u16,
    requests: Arc<RwLock<Vec<(String, Value)>>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

#[derive(Clone)]
struct StubState {
    behavior: GuiderStubBehavior,
    requests: Arc<RwLock<Vec<(String, Value)>>>,
}

impl GuiderStub {
    /// Spawn an axum router on `127.0.0.1:0` serving the guider
    /// service's endpoints with the configured behavior.
    pub async fn start(behavior: GuiderStubBehavior) -> Self {
        let requests: Arc<RwLock<Vec<(String, Value)>>> = Arc::new(RwLock::new(Vec::new()));
        let state = StubState {
            behavior,
            requests: requests.clone(),
        };

        let app = Router::new()
            .route("/api/v1/guiding/start", post(settle_handler))
            .route("/api/v1/guiding/stop", post(stop_handler))
            .route("/api/v1/guiding/pause", post(pause_handler))
            .route("/api/v1/guiding/resume", post(resume_handler))
            .route("/api/v1/dither", post(settle_handler))
            .route("/api/v1/guiding/stats", get(stats_handler))
            .route("/health", get(health_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind guider stub");
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{}", port);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("guider stub failed");
        });

        Self {
            url,
            port,
            requests,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// All `(path, body)` pairs the stub has received, in order. The
    /// body is `Value::Null` for endpoints called without a JSON body
    /// (`stop`, `resume`).
    pub async fn requests(&self) -> Vec<(String, Value)> {
        self.requests.read().await.clone()
    }

    /// The recorded bodies of requests whose path ends with
    /// `path_suffix` (e.g. `"/guiding/start"`, `"/dither"`).
    pub async fn requests_to(&self, path_suffix: &str) -> Vec<Value> {
        self.requests
            .read()
            .await
            .iter()
            .filter(|(path, _)| path.ends_with(path_suffix))
            .map(|(_, body)| body.clone())
            .collect()
    }

    /// Stop the stub server.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for GuiderStub {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Record the request and hand back the behavior-appropriate error,
/// or `None` when the behavior is `Canned` (the caller then builds
/// its endpoint-specific success body).
async fn record_and_check(
    state: &StubState,
    uri: &axum::http::Uri,
    body: axum::body::Bytes,
) -> Option<axum::response::Response> {
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    state
        .requests
        .write()
        .await
        .push((uri.path().to_string(), parsed));

    match &state.behavior {
        GuiderStubBehavior::Canned(_) => None,
        GuiderStubBehavior::Error { code, message } => {
            let status = match code.as_str() {
                "invalid_request" => StatusCode::BAD_REQUEST,
                "not_guiding" => StatusCode::CONFLICT,
                "guide_failed" => StatusCode::UNPROCESSABLE_ENTITY,
                "settle_timeout" | "stop_timeout" => StatusCode::GATEWAY_TIMEOUT,
                "phd2_unreachable" => StatusCode::BAD_GATEWAY,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            let body = Json(serde_json::json!({
                "error": code,
                "message": message,
            }));
            Some((status, body).into_response())
        }
    }
}

fn canned(state: &StubState) -> &CannedGuiding {
    match &state.behavior {
        GuiderStubBehavior::Canned(c) => c,
        // record_and_check already returned the error response for
        // the Error behavior; this branch is unreachable by
        // construction.
        GuiderStubBehavior::Error { .. } => unreachable!("canned() called on Error behavior"),
    }
}

async fn settle_handler(
    State(state): State<StubState>,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if let Some(err) = record_and_check(&state, &uri, body).await {
        return err;
    }
    let c = canned(&state);
    if !c.settle_delay.is_zero() {
        tokio::time::sleep(c.settle_delay).await;
    }
    Json(serde_json::json!({
        "state": "guiding",
        "rms_ra_px": c.rms_ra_px,
        "rms_dec_px": c.rms_dec_px,
        "total_rms_px": c.total_rms_px,
        "sample_count": c.sample_count,
    }))
    .into_response()
}

async fn stop_handler(
    State(state): State<StubState>,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if let Some(err) = record_and_check(&state, &uri, body).await {
        return err;
    }
    Json(serde_json::json!({ "state": "stopped" })).into_response()
}

async fn pause_handler(
    State(state): State<StubState>,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if let Some(err) = record_and_check(&state, &uri, body).await {
        return err;
    }
    Json(serde_json::json!({ "state": "paused" })).into_response()
}

async fn resume_handler(
    State(state): State<StubState>,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if let Some(err) = record_and_check(&state, &uri, body).await {
        return err;
    }
    Json(serde_json::json!({ "state": "resumed" })).into_response()
}

async fn stats_handler(
    State(state): State<StubState>,
    uri: axum::http::Uri,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if let Some(err) = record_and_check(&state, &uri, body).await {
        return err;
    }
    let c = canned(&state);
    Json(serde_json::json!({
        "app_state": if c.guiding { "Guiding" } else { "Stopped" },
        "guiding": c.guiding,
        "rms_ra_px": c.rms_ra_px,
        "rms_dec_px": c.rms_dec_px,
        "total_rms_px": c.total_rms_px,
        "snr": c.snr,
        "star_mass": c.star_mass,
        "sample_count": c.sample_count,
    }))
    .into_response()
}

async fn health_handler(State(_state): State<StubState>) -> axum::response::Response {
    Json(serde_json::json!({ "status": "ok" })).into_response()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn canned_stub_answers_every_endpoint_and_records_requests() {
        let stub = GuiderStub::start(GuiderStubBehavior::Canned(CannedGuiding::default())).await;
        let client = reqwest::Client::new();

        let start: Value = client
            .post(format!("{}/api/v1/guiding/start", stub.url))
            .json(&serde_json::json!({ "recalibrate": true }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(start["state"], "guiding");
        assert_eq!(start["total_rms_px"], 0.5);

        let stop: Value = client
            .post(format!("{}/api/v1/guiding/stop", stub.url))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(stop["state"], "stopped");

        let stats: Value = client
            .get(format!("{}/api/v1/guiding/stats", stub.url))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(stats["app_state"], "Guiding");
        assert_eq!(stats["snr"], 25.0);

        let requests = stub.requests().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].0, "/api/v1/guiding/start");
        assert_eq!(requests[0].1["recalibrate"], true);
        // A body-less stop records Null.
        assert_eq!(requests[1].1, Value::Null);

        let starts = stub.requests_to("/guiding/start").await;
        assert_eq!(starts.len(), 1);
    }

    #[tokio::test]
    async fn error_behavior_maps_codes_to_the_service_statuses() {
        for (code, expected_status) in [
            ("invalid_request", 400),
            ("not_guiding", 409),
            ("guide_failed", 422),
            ("settle_timeout", 504),
            ("stop_timeout", 504),
            ("phd2_unreachable", 502),
            ("internal", 500),
        ] {
            let stub = GuiderStub::start(GuiderStubBehavior::Error {
                code: code.to_string(),
                message: "boom".to_string(),
            })
            .await;
            let resp = reqwest::Client::new()
                .post(format!("{}/api/v1/dither", stub.url))
                .json(&serde_json::json!({ "amount_px": 3.0 }))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), expected_status, "code {code}");
            let body: Value = resp.json().await.unwrap();
            assert_eq!(body["error"], code);
            assert_eq!(body["message"], "boom");
        }
    }
}
