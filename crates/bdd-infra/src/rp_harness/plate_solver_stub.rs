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
    /// Read the request's `fits_path`, parse `CRVAL1` / `CRVAL2` from
    /// the FITS primary header, and return them as the solved
    /// `ra_center` / `dec_center`. The CRVAL round-trip models what
    /// an honest plate solver on a WCS-bearing cutout would do.
    /// Other fields (`pixel_scale_arcsec`, `rotation_deg`, `solver`)
    /// come from the embedded `template`. If CRVAL1/2 are missing
    /// the handler returns a `solve_failed` error envelope so the
    /// test fails loudly rather than silently degrading to a fixed
    /// answer.
    ///
    /// **Note on the rp closed-loop centering BDD.** That scenario
    /// does *not* use this variant: rp's persistence layer writes
    /// its own FITS from the camera's `ImageArray` and strips any
    /// camera-side WCS, so the `fits_path` rp hands to the stub has
    /// no `CRVAL1/2` to echo. The closed-loop scenarios use
    /// [`Sequence`] instead, feeding pre-computed solve outcomes
    /// in the order the loop will request them. This variant
    /// remains useful for unit tests / future flows that hand the
    /// stub a WCS-bearing FITS directly (e.g. paired with
    /// [`crate::sky_survey_camera_harness::SkyViewStub`]).
    ///
    /// [`Sequence`]: StubBehavior::Sequence
    EchoFitsCenter { template: CannedWcs },
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
    // Extract `fits_path` *before* moving `body` into the request log
    // — only the `EchoFitsCenter` branch needs it, but borrowing
    // here avoids cloning the whole `Value` for every request shape.
    let fits_path_owned = body
        .get("fits_path")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
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
        StubBehavior::EchoFitsCenter { ref template } => {
            let fits_path = fits_path_owned.as_deref().unwrap_or("");
            match read_crval_from_fits(fits_path) {
                Ok((ra, dec)) => canned_response(&CannedWcs {
                    ra_center: ra,
                    dec_center: dec,
                    pixel_scale_arcsec: template.pixel_scale_arcsec,
                    rotation_deg: template.rotation_deg,
                    solver: template.solver.clone(),
                }),
                Err(reason) => {
                    let body = Json(serde_json::json!({
                        "error": "solve_failed",
                        "message": format!(
                            "EchoFitsCenter could not read CRVAL1/CRVAL2 from {}: {}",
                            fits_path, reason
                        ),
                    }));
                    (StatusCode::UNPROCESSABLE_ENTITY, body).into_response()
                }
            }
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

fn read_crval_from_fits(path: &str) -> Result<(f64, f64), String> {
    use rp_fits::reader::read_primary_keyword;
    use rp_fits::writer::KeywordValue;
    if path.is_empty() {
        return Err("missing fits_path in request body".to_string());
    }
    // Read the file once into memory; the keyword reader consumes its
    // input stream, so two separate Cursor reads over the same buffer
    // is the cleanest way to fetch both CRVAL1 and CRVAL2.
    let bytes = std::fs::read(path).map_err(|e| format!("open: {e}"))?;
    let ra = read_primary_keyword(std::io::Cursor::new(&bytes), "CRVAL1")
        .map_err(|e| format!("read CRVAL1: {e}"))?
        .ok_or_else(|| "CRVAL1 absent".to_string())?;
    let dec = read_primary_keyword(std::io::Cursor::new(&bytes), "CRVAL2")
        .map_err(|e| format!("read CRVAL2: {e}"))?
        .ok_or_else(|| "CRVAL2 absent".to_string())?;
    let ra_deg = match ra {
        KeywordValue::Float(v) => v,
        KeywordValue::Int(v) => v as f64,
        other => return Err(format!("CRVAL1 not numeric: {other:?}")),
    };
    let dec_deg = match dec {
        KeywordValue::Float(v) => v,
        KeywordValue::Int(v) => v as f64,
        other => return Err(format!("CRVAL2 not numeric: {other:?}")),
    };
    Ok((ra_deg, dec_deg))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    fn template() -> CannedWcs {
        CannedWcs {
            ra_center: 0.0,
            dec_center: 0.0,
            pixel_scale_arcsec: 1.05,
            rotation_deg: 0.0,
            solver: "stub".to_string(),
        }
    }

    /// Build a 1×1 BITPIX=16 FITS with the requested CRVAL header
    /// records. `with_crval = false` produces a header that's missing
    /// CRVAL1/CRVAL2, used to drive the absent-keyword error path.
    /// Local to the test module so the stub's no-production-crate-dep
    /// invariant stays intact.
    fn minimal_fits(with_crval: Option<(f64, f64)>) -> Vec<u8> {
        const BLOCK: usize = 2880;
        fn pad(s: &str) -> String {
            let mut p = format!("{s:<80}");
            p.truncate(80);
            p
        }
        let mut header = String::new();
        header.push_str(&pad("SIMPLE  =                    T"));
        header.push_str(&pad("BITPIX  =                   16"));
        header.push_str(&pad("NAXIS   =                    2"));
        header.push_str(&pad("NAXIS1  =                    1"));
        header.push_str(&pad("NAXIS2  =                    1"));
        if let Some((ra, dec)) = with_crval {
            header.push_str(&pad(&format!("CRVAL1  = {ra:>20.10E}")));
            header.push_str(&pad(&format!("CRVAL2  = {dec:>20.10E}")));
        }
        header.push_str(&pad("END"));
        while !header.len().is_multiple_of(BLOCK) {
            header.push(' ');
        }
        let mut bytes = header.into_bytes();
        bytes.extend_from_slice(&0i16.to_be_bytes());
        while !bytes.len().is_multiple_of(BLOCK) {
            bytes.push(0);
        }
        bytes
    }

    #[tokio::test]
    async fn echo_fits_center_returns_crval_from_request_path() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits");
        std::fs::write(&fits_path, minimal_fits(Some((12.5, -34.5)))).unwrap();

        let stub = PlateSolverStub::start(StubBehavior::EchoFitsCenter {
            template: template(),
        })
        .await;

        let resp: Value = reqwest::Client::new()
            .post(format!("{}/api/v1/solve", stub.url))
            .json(&serde_json::json!({ "fits_path": fits_path.to_string_lossy() }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert!(
            (resp["ra_center"].as_f64().unwrap_or(f64::NAN) - 12.5).abs() < 1e-6,
            "response = {resp}"
        );
        assert!(
            (resp["dec_center"].as_f64().unwrap_or(f64::NAN) - -34.5).abs() < 1e-6,
            "response = {resp}"
        );
        assert_eq!(resp["pixel_scale_arcsec"], 1.05);
    }

    #[tokio::test]
    async fn echo_fits_center_returns_solve_failed_when_crval_absent() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits");
        std::fs::write(&fits_path, minimal_fits(None)).unwrap();

        let stub = PlateSolverStub::start(StubBehavior::EchoFitsCenter {
            template: template(),
        })
        .await;

        let resp = reqwest::Client::new()
            .post(format!("{}/api/v1/solve", stub.url))
            .json(&serde_json::json!({ "fits_path": fits_path.to_string_lossy() }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 422);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "solve_failed");
        assert!(body["message"].as_str().unwrap().contains("CRVAL"));
    }
}
