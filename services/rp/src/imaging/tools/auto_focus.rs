//! `auto_focus`: V-curve focus sweep compound tool.
//!
//! The driving logic — sweep grid construction, the move/capture/measure
//! loop, parabolic least-squares fit, vertex-in-range validation — is
//! pure Rust and fully unit-testable via the [`FocuserOps`],
//! [`CaptureOps`], [`MeasureOps`] traits. The MCP wrapper in `mcp.rs`
//! provides concrete adapters that bind to the real Alpaca focuser /
//! camera and the image cache; tests substitute synthetic adapters
//! that drive the loop with deterministic per-position HFR data.
//!
//! Behavioral contract: `docs/services/rp.md` → Compound Tools →
//! `auto_focus` Contract.

use async_trait::async_trait;
use serde::Serialize;
use std::time::Duration;
use thiserror::Error;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct AutoFocusParams {
    pub duration: Duration,
    pub step_size: i32,
    pub half_width: i32,
    pub min_area: usize,
    pub max_area: usize,
    pub threshold_sigma: f64,
    pub min_fit_points: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CurvePoint {
    pub position: i32,
    pub hfr: Option<f64>,
    pub star_count: u32,
    pub document_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutoFocusResult {
    pub best_position: i32,
    pub best_hfr: f64,
    pub final_position: i32,
    pub samples_used: usize,
    pub curve_points: Vec<CurvePoint>,
    pub temperature_c: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct HfrSample {
    pub hfr: Option<f64>,
    pub star_count: u32,
}

#[derive(Debug, Error)]
pub enum AutoFocusError {
    #[error("step_size must be positive (got {0})")]
    InvalidStepSize(i32),
    #[error("half_width must be positive (got {0})")]
    InvalidHalfWidth(i32),
    #[error("min_fit_points must be at least 3 (got {0})")]
    InvalidMinFitPoints(usize),
    #[error(
        "sweep grid has {available} positions after clamping to focuser bounds; \
         min_fit_points={requested}"
    )]
    GridTooSmall { available: usize, requested: usize },
    #[error("not enough stars: only {got} of {needed} required samples have non-null HFR")]
    NotEnoughStars { got: usize, needed: usize },
    #[error("monotonic curve: {0}")]
    MonotonicCurve(String),
    #[error("equipment error during sweep: {0}")]
    Equipment(String),
}

#[async_trait]
pub trait FocuserOps {
    async fn position(&self) -> Result<i32, String>;
    async fn move_to(&self, position: i32) -> Result<i32, String>;
    /// Returns `None` for `NOT_IMPLEMENTED` or any transient read failure
    /// — temperature is informational on the result, never load-bearing
    /// on the sweep, so a single failure does not abort the run.
    async fn temperature(&self) -> Option<f64>;
}

#[async_trait]
pub trait CaptureOps {
    /// Capture an exposure of `duration` and return its `document_id`.
    async fn capture(&self, duration: Duration) -> Result<String, String>;
}

#[async_trait]
pub trait MeasureOps {
    async fn measure(
        &self,
        document_id: &str,
        min_area: usize,
        max_area: usize,
        threshold_sigma: f64,
    ) -> Result<HfrSample, String>;
}

pub fn validate_params(params: &AutoFocusParams) -> Result<(), AutoFocusError> {
    if params.step_size <= 0 {
        return Err(AutoFocusError::InvalidStepSize(params.step_size));
    }
    if params.half_width <= 0 {
        return Err(AutoFocusError::InvalidHalfWidth(params.half_width));
    }
    if params.min_fit_points < 3 {
        return Err(AutoFocusError::InvalidMinFitPoints(params.min_fit_points));
    }
    Ok(())
}

/// Build the sweep grid `[start, start+step, …, end]` clamped to
/// `[min_bound, max_bound]`. Out-of-range points are dropped (not
/// coerced) — coercion would produce duplicate samples at a bound and
/// distort the parabola fit.
pub fn build_grid(
    current: i32,
    step: i32,
    half_width: i32,
    bounds: (Option<i32>, Option<i32>),
) -> Vec<i32> {
    let start = current.saturating_sub(half_width);
    let end = current.saturating_add(half_width);
    let mut grid = Vec::new();
    let mut p = start;
    loop {
        let in_min = bounds.0.is_none_or(|min| p >= min);
        let in_max = bounds.1.is_none_or(|max| p <= max);
        if in_min && in_max {
            grid.push(p);
        }
        let next = p.saturating_add(step);
        if p == end || next <= p {
            break;
        }
        if next > end {
            break;
        }
        p = next;
    }
    grid
}

/// Result of fitting `hfr = a·x² + b·x + c` with vertex at
/// `(round(−b/2a), c − b²/(4a))`.
#[derive(Debug, Clone, Copy)]
pub struct ParabolaFit {
    pub a: f64,
    pub b: f64,
    pub c: f64,
}

impl ParabolaFit {
    pub fn vertex_position(&self) -> i32 {
        (-self.b / (2.0 * self.a)).round() as i32
    }

    pub fn vertex_value(&self) -> f64 {
        self.c - (self.b * self.b) / (4.0 * self.a)
    }
}

/// Weighted least-squares fit of a parabola `y = a·x² + b·x + c` to
/// `(position, hfr, weight)` samples. `weight` is typically the
/// per-frame `star_count`; samples with `weight == 0` are dropped.
///
/// Returns `MonotonicCurve` if `a ≤ 0` (the curve has no minimum) or if
/// the design matrix is too ill-conditioned to invert (essentially
/// flat input, where the vertex is undefined).
pub fn fit_parabola(samples: &[(i32, f64, u32)]) -> Result<ParabolaFit, AutoFocusError> {
    let filtered: Vec<(f64, f64, f64)> = samples
        .iter()
        .filter(|(_, _, w)| *w > 0)
        .map(|(x, y, w)| (*x as f64, *y, *w as f64))
        .collect();
    if filtered.len() < 3 {
        return Err(AutoFocusError::NotEnoughStars {
            got: filtered.len(),
            needed: 3,
        });
    }
    // Normal equations Aᵀ W A · [a, b, c]ᵀ = Aᵀ W y, with
    // A = [x², x, 1] per row, W = diag(weights). Solved via Cramer's
    // rule on the 3×3 system below.
    let mut m4 = 0.0;
    let mut m3 = 0.0;
    let mut m2 = 0.0;
    let mut m1 = 0.0;
    let mut m0 = 0.0;
    let mut t2 = 0.0;
    let mut t1 = 0.0;
    let mut t0 = 0.0;
    for (x, y, w) in &filtered {
        let x2 = x * x;
        let x3 = x2 * x;
        let x4 = x3 * x;
        m4 += w * x4;
        m3 += w * x3;
        m2 += w * x2;
        m1 += w * x;
        m0 += w;
        t2 += w * x2 * y;
        t1 += w * x * y;
        t0 += w * y;
    }
    let det = m4 * (m2 * m0 - m1 * m1) - m3 * (m3 * m0 - m1 * m2) + m2 * (m3 * m1 - m2 * m2);
    // Scale-invariant ill-conditioning check: the normal-equation
    // determinant is roughly O(m4·m2·m0) for the problem; require the
    // actual det to be at least ~1e-12 of that ceiling. Below this, the
    // input is effectively flat and the vertex is meaningless.
    let det_scale = (m4.abs() * m2.abs() * m0.abs()).max(1.0);
    if det.abs() < det_scale * 1e-12 {
        return Err(AutoFocusError::MonotonicCurve(format!(
            "design matrix is singular (det={:.3e}, scale={:.3e})",
            det, det_scale
        )));
    }
    let det_a = t2 * (m2 * m0 - m1 * m1) - m3 * (t1 * m0 - m1 * t0) + m2 * (t1 * m1 - m2 * t0);
    let det_b = m4 * (t1 * m0 - m1 * t0) - t2 * (m3 * m0 - m1 * m2) + m2 * (m3 * t0 - t1 * m2);
    let det_c = m4 * (m2 * t0 - t1 * m1) - m3 * (m3 * t0 - t1 * m2) + t2 * (m3 * m1 - m2 * m2);
    let a = det_a / det;
    let b = det_b / det;
    let c = det_c / det;
    if a <= 0.0 {
        return Err(AutoFocusError::MonotonicCurve(format!(
            "non-positive leading coefficient (a={:.3e})",
            a
        )));
    }
    Ok(ParabolaFit { a, b, c })
}

/// Drive the V-curve sweep against the supplied focuser/capturer/measurer
/// adapters. See `docs/services/rp.md` → `auto_focus` Contract for the
/// behavioral spec; this function is the reference implementation.
pub async fn run_auto_focus<F: FocuserOps + Sync, C: CaptureOps + Sync, M: MeasureOps + Sync>(
    focuser: &F,
    capturer: &C,
    measurer: &M,
    bounds: (Option<i32>, Option<i32>),
    params: AutoFocusParams,
) -> Result<AutoFocusResult, AutoFocusError> {
    validate_params(&params)?;

    let current = focuser
        .position()
        .await
        .map_err(AutoFocusError::Equipment)?;

    let grid = build_grid(current, params.step_size, params.half_width, bounds);
    if grid.len() < params.min_fit_points {
        return Err(AutoFocusError::GridTooSmall {
            available: grid.len(),
            requested: params.min_fit_points,
        });
    }

    let temperature_c = focuser.temperature().await;
    debug!(
        current_position = current,
        grid_len = grid.len(),
        temperature_c = ?temperature_c,
        "auto_focus sweep starting"
    );

    let mut curve_points = Vec::with_capacity(grid.len());
    for position in &grid {
        focuser
            .move_to(*position)
            .await
            .map_err(AutoFocusError::Equipment)?;
        let document_id = capturer
            .capture(params.duration)
            .await
            .map_err(AutoFocusError::Equipment)?;
        let sample = measurer
            .measure(
                &document_id,
                params.min_area,
                params.max_area,
                params.threshold_sigma,
            )
            .await
            .map_err(AutoFocusError::Equipment)?;
        curve_points.push(CurvePoint {
            position: *position,
            hfr: sample.hfr,
            star_count: sample.star_count,
            document_id,
        });
    }

    let valid_samples: Vec<(i32, f64, u32)> = curve_points
        .iter()
        .filter_map(|p| p.hfr.map(|h| (p.position, h, p.star_count)))
        .collect();
    if valid_samples.len() < params.min_fit_points {
        return Err(AutoFocusError::NotEnoughStars {
            got: valid_samples.len(),
            needed: params.min_fit_points,
        });
    }

    let fit = fit_parabola(&valid_samples)?;
    let best_position = fit.vertex_position();
    let grid_min = *grid.first().expect("grid non-empty by construction");
    let grid_max = *grid.last().expect("grid non-empty by construction");
    if best_position < grid_min || best_position > grid_max {
        return Err(AutoFocusError::MonotonicCurve(format!(
            "fitted vertex {} is outside sampled grid [{}, {}]",
            best_position, grid_min, grid_max
        )));
    }
    let best_hfr = fit.vertex_value();

    let final_position = focuser
        .move_to(best_position)
        .await
        .map_err(AutoFocusError::Equipment)?;

    Ok(AutoFocusResult {
        best_position,
        best_hfr,
        final_position,
        samples_used: valid_samples.len(),
        curve_points,
        temperature_c,
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ---- pure helpers ----

    #[test]
    fn validate_params_accepts_minimum_valid_input() {
        let p = AutoFocusParams {
            duration: Duration::from_millis(100),
            step_size: 1,
            half_width: 1,
            min_area: 1,
            max_area: 1,
            threshold_sigma: 5.0,
            min_fit_points: 3,
        };
        validate_params(&p).unwrap();
    }

    #[test]
    fn validate_params_rejects_zero_step_size() {
        let p = AutoFocusParams {
            duration: Duration::from_millis(100),
            step_size: 0,
            half_width: 100,
            min_area: 5,
            max_area: 1000,
            threshold_sigma: 5.0,
            min_fit_points: 5,
        };
        assert!(matches!(
            validate_params(&p),
            Err(AutoFocusError::InvalidStepSize(0))
        ));
    }

    #[test]
    fn validate_params_rejects_negative_half_width() {
        let p = AutoFocusParams {
            duration: Duration::from_millis(100),
            step_size: 50,
            half_width: -1,
            min_area: 5,
            max_area: 1000,
            threshold_sigma: 5.0,
            min_fit_points: 5,
        };
        assert!(matches!(
            validate_params(&p),
            Err(AutoFocusError::InvalidHalfWidth(-1))
        ));
    }

    #[test]
    fn validate_params_rejects_min_fit_points_below_3() {
        let p = AutoFocusParams {
            duration: Duration::from_millis(100),
            step_size: 50,
            half_width: 100,
            min_area: 5,
            max_area: 1000,
            threshold_sigma: 5.0,
            min_fit_points: 2,
        };
        assert!(matches!(
            validate_params(&p),
            Err(AutoFocusError::InvalidMinFitPoints(2))
        ));
    }

    // ---- grid construction ----

    #[test]
    fn build_grid_unbounded_is_symmetric() {
        let g = build_grid(1000, 100, 200, (None, None));
        assert_eq!(g, vec![800, 900, 1000, 1100, 1200]);
    }

    #[test]
    fn build_grid_clamps_below_min_position() {
        let g = build_grid(5000, 100, 500, (Some(4900), None));
        assert_eq!(g, vec![4900, 5000, 5100, 5200, 5300, 5400, 5500]);
    }

    #[test]
    fn build_grid_clamps_above_max_position() {
        let g = build_grid(5000, 100, 500, (None, Some(5100)));
        assert_eq!(g, vec![4500, 4600, 4700, 4800, 4900, 5000, 5100]);
    }

    #[test]
    fn build_grid_clamps_both_sides() {
        let g = build_grid(5000, 100, 500, (Some(4900), Some(5100)));
        assert_eq!(g, vec![4900, 5000, 5100]);
    }

    #[test]
    fn build_grid_step_larger_than_half_width_yields_singleton() {
        let g = build_grid(1000, 1000, 100, (None, None));
        assert_eq!(g, vec![900]);
    }

    // ---- parabola fit ----

    fn make_v_samples(vertex_x: i32, vertex_y: f64, curvature: f64) -> Vec<(i32, f64, u32)> {
        (vertex_x - 200..=vertex_x + 200)
            .step_by(50)
            .map(|x| {
                let dx = (x - vertex_x) as f64;
                let y = curvature * dx * dx + vertex_y;
                (x, y, 100)
            })
            .collect()
    }

    #[test]
    fn fit_parabola_recovers_known_minimum_to_within_one_step() {
        let samples = make_v_samples(1234, 1.5, 1e-4);
        let fit = fit_parabola(&samples).unwrap();
        let vx = fit.vertex_position();
        assert!(
            (vx - 1234).abs() <= 1,
            "expected vertex_position within ±1 of 1234, got {}",
            vx
        );
        let vy = fit.vertex_value();
        assert!(
            (vy - 1.5).abs() < 1e-6,
            "expected vertex_value ≈ 1.5, got {}",
            vy
        );
    }

    #[test]
    fn fit_parabola_rejects_flat_input() {
        let samples: Vec<_> = (0..10).map(|i| (i * 100, 5.0, 100)).collect();
        match fit_parabola(&samples) {
            Err(AutoFocusError::MonotonicCurve(_)) => {}
            other => panic!("expected MonotonicCurve, got {:?}", other),
        }
    }

    #[test]
    fn fit_parabola_rejects_concave_down_curve() {
        let samples: Vec<_> = (0..10)
            .map(|i| {
                let x = i * 50;
                let dx = (x - 100) as f64;
                (x, 5.0 - 1e-4 * dx * dx, 100)
            })
            .collect();
        match fit_parabola(&samples) {
            Err(AutoFocusError::MonotonicCurve(msg)) => {
                assert!(msg.contains("non-positive"), "got msg: {}", msg);
            }
            other => panic!("expected MonotonicCurve, got {:?}", other),
        }
    }

    #[test]
    fn fit_parabola_rejects_too_few_samples() {
        let samples = vec![(0, 1.0, 100), (10, 2.0, 100)];
        match fit_parabola(&samples) {
            Err(AutoFocusError::NotEnoughStars { got: 2, needed: 3 }) => {}
            other => panic!("expected NotEnoughStars, got {:?}", other),
        }
    }

    #[test]
    fn fit_parabola_drops_zero_weight_samples() {
        let mut samples = make_v_samples(1000, 2.0, 1e-4);
        samples.push((100_000, 999.0, 0));
        samples.push((-100_000, 999.0, 0));
        let fit = fit_parabola(&samples).unwrap();
        assert!((fit.vertex_position() - 1000).abs() <= 1);
    }

    // ---- end-to-end run_auto_focus over synthetic adapters ----

    struct StubFocuser {
        position: Mutex<i32>,
    }

    #[async_trait]
    impl FocuserOps for StubFocuser {
        async fn position(&self) -> Result<i32, String> {
            Ok(*self.position.lock().unwrap())
        }
        async fn move_to(&self, position: i32) -> Result<i32, String> {
            *self.position.lock().unwrap() = position;
            Ok(position)
        }
        async fn temperature(&self) -> Option<f64> {
            Some(4.5)
        }
    }

    /// The capturer reads the focuser's current position and stamps it
    /// into the synthetic `document_id`, so the measurer can recover
    /// per-position HFR values without any shared state beyond the id.
    struct StubCapturer<'a> {
        focuser: &'a StubFocuser,
        counter: Mutex<u64>,
    }

    #[async_trait]
    impl CaptureOps for StubCapturer<'_> {
        async fn capture(&self, _duration: Duration) -> Result<String, String> {
            let pos = *self.focuser.position.lock().unwrap();
            let mut c = self.counter.lock().unwrap();
            *c += 1;
            Ok(format!("doc-{:05}-pos{}", *c, pos))
        }
    }

    /// Synthetic V-curve: `hfr = curvature·(pos − vertex)² + vertex_y`.
    /// Recovers the position from the document id stamped by [`StubCapturer`].
    struct StubMeasurer {
        vertex: i32,
        vertex_y: f64,
        curvature: f64,
        star_count: u32,
    }

    #[async_trait]
    impl MeasureOps for StubMeasurer {
        async fn measure(
            &self,
            document_id: &str,
            _min_area: usize,
            _max_area: usize,
            _threshold_sigma: f64,
        ) -> Result<HfrSample, String> {
            let pos: i32 = document_id
                .rsplit_once("pos")
                .and_then(|(_, s)| s.parse().ok())
                .ok_or_else(|| format!("bad document_id: {document_id}"))?;
            let dx = (pos - self.vertex) as f64;
            let hfr = self.curvature * dx * dx + self.vertex_y;
            Ok(HfrSample {
                hfr: Some(hfr),
                star_count: self.star_count,
            })
        }
    }

    #[tokio::test]
    async fn run_auto_focus_recovers_known_vertex() {
        let foc = StubFocuser {
            position: Mutex::new(1234),
        };
        let cap = StubCapturer {
            focuser: &foc,
            counter: Mutex::new(0),
        };
        let meas = StubMeasurer {
            vertex: 1234,
            vertex_y: 2.0,
            curvature: 1e-4,
            star_count: 100,
        };
        let result = run_auto_focus(
            &foc,
            &cap,
            &meas,
            (None, None),
            AutoFocusParams {
                duration: Duration::from_millis(100),
                step_size: 100,
                half_width: 400,
                min_area: 5,
                max_area: 1000,
                threshold_sigma: 5.0,
                min_fit_points: 5,
            },
        )
        .await
        .unwrap();
        assert!(
            (result.best_position - 1234).abs() <= 1,
            "best_position {} not within ±1 of 1234",
            result.best_position
        );
        assert!((result.best_hfr - 2.0).abs() < 1e-6);
        assert_eq!(result.samples_used, 9);
        assert_eq!(result.curve_points.len(), 9);
        assert_eq!(result.final_position, result.best_position);
        assert_eq!(result.temperature_c, Some(4.5));
    }

    #[tokio::test]
    async fn run_auto_focus_errors_on_grid_too_small_after_clamp() {
        let foc = StubFocuser {
            position: Mutex::new(5000),
        };
        let cap = StubCapturer {
            focuser: &foc,
            counter: Mutex::new(0),
        };
        let meas = StubMeasurer {
            vertex: 5000,
            vertex_y: 2.0,
            curvature: 1e-4,
            star_count: 100,
        };
        let err = run_auto_focus(
            &foc,
            &cap,
            &meas,
            (Some(4900), Some(5100)),
            AutoFocusParams {
                duration: Duration::from_millis(100),
                step_size: 100,
                half_width: 500,
                min_area: 5,
                max_area: 1000,
                threshold_sigma: 5.0,
                min_fit_points: 5,
            },
        )
        .await;
        assert!(matches!(
            err,
            Err(AutoFocusError::GridTooSmall {
                available: 3,
                requested: 5
            })
        ));
    }

    /// Sparse stars: only the central pair has detections. With 9 grid
    /// points and only 2 useful samples, the run must error
    /// `NotEnoughStars`.
    #[tokio::test]
    async fn run_auto_focus_errors_on_not_enough_stars_after_skips() {
        struct Sparse;
        #[async_trait]
        impl MeasureOps for Sparse {
            async fn measure(
                &self,
                document_id: &str,
                _min_area: usize,
                _max_area: usize,
                _threshold_sigma: f64,
            ) -> Result<HfrSample, String> {
                let pos: i32 = document_id
                    .rsplit_once("pos")
                    .and_then(|(_, s)| s.parse().ok())
                    .unwrap();
                if (pos - 1234).abs() <= 50 {
                    Ok(HfrSample {
                        hfr: Some(2.5),
                        star_count: 50,
                    })
                } else {
                    Ok(HfrSample {
                        hfr: None,
                        star_count: 0,
                    })
                }
            }
        }
        let foc = StubFocuser {
            position: Mutex::new(1234),
        };
        let cap = StubCapturer {
            focuser: &foc,
            counter: Mutex::new(0),
        };
        let err = run_auto_focus(
            &foc,
            &cap,
            &Sparse,
            (None, None),
            AutoFocusParams {
                duration: Duration::from_millis(100),
                step_size: 100,
                half_width: 400,
                min_area: 5,
                max_area: 1000,
                threshold_sigma: 5.0,
                min_fit_points: 5,
            },
        )
        .await;
        assert!(matches!(
            err,
            Err(AutoFocusError::NotEnoughStars { needed: 5, .. })
        ));
    }
}
