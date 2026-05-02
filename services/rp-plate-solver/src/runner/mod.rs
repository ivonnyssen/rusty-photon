//! Subprocess-runner abstraction.
//!
//! `AstapRunner` is the trait the HTTP handler calls. The real
//! implementation is in `astap.rs`; tests use `mockall`'s generated mock.

use async_trait::async_trait;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

pub mod astap;
pub mod wcs;

/// Inputs to one solve attempt. Hint fields use decimal degrees on the wire
/// to match the response field `ra_center` (also degrees, sourced from
/// `CRVAL1`); the runner converts to ASTAP's expected internal units before
/// spawning. See `docs/services/rp-plate-solver.md` §"Hint Mapping".
#[derive(Debug, Clone)]
pub struct SolveRequest {
    pub fits_path: PathBuf,
    pub ra_hint: Option<f64>,
    pub dec_hint: Option<f64>,
    pub fov_hint_deg: Option<f64>,
    pub search_radius_deg: Option<f64>,
    pub timeout: Duration,
}

/// Successful solve result, returned to the HTTP layer for serialization.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SolveOutcome {
    pub ra_center: f64,
    pub dec_center: f64,
    pub pixel_scale_arcsec: f64,
    pub rotation_deg: f64,
    pub solver: String,
}

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("ASTAP exited with code {status}: {stderr_tail}")]
    ExitStatus { status: i32, stderr_tail: String },

    #[error("ASTAP did not write a .wcs sidecar (exit was clean)")]
    NoWcs,

    #[error("malformed .wcs: {0}")]
    MalformedWcs(String),

    #[error("solve timed out (terminated)")]
    TimedOutTerminated,

    #[error("solve timed out (killed after grace)")]
    TimedOutKilled,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AstapRunner: Send + Sync {
    async fn solve(&self, request: SolveRequest) -> Result<SolveOutcome, RunnerError>;
}
