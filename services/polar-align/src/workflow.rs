//! The polar-alignment workflow: three-point measurement, then the
//! live adjustment loop. Publishes progress into the shared status
//! the HTTP layer serves on `GET /status`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::{Notify, RwLock};
use tracing::{debug, info, warn};

use crate::config::PolarAlignConfig;
use crate::ephemeris::EphemerisCtx;
use crate::error::{PolarAlignError, Result};
use crate::math::{
    alignment_errors, attitude_from_wcs, axis_from_three_points, relative_rotation,
    rotation_between, unit_from_radec, wcs_pixel_to_sky, wcs_sky_to_pixel, AlignmentErrors, Mat3,
    SolvedFrame, Vec3,
};
use crate::mcp_client::{DetectedStar, McpClient, SolveResult};

/// Workflow lifecycle, serialized verbatim into `/status.phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Measuring,
    Adjusting,
    Complete,
    Error,
}

/// The measurement block of `/status`, refreshed by every adjustment
/// solve as the operator converges.
#[derive(Debug, Clone, Serialize)]
pub struct MeasurementStatus {
    pub axis_azimuth_deg: f64,
    pub axis_altitude_deg: f64,
    pub azimuth_error_arcmin: f64,
    pub altitude_error_arcmin: f64,
    pub total_error_arcmin: f64,
    pub azimuth_direction: String,
    pub altitude_direction: String,
    pub measured_at: DateTime<Utc>,
}

/// A detected star paired with the pixel it will occupy once the
/// axis sits on the pole. Both pairs are 0-based pixel indices into
/// the adjustment capture — the convention `detect_stars` reports.
#[derive(Debug, Clone, Serialize)]
pub struct StarTarget {
    pub x: f64,
    pub y: f64,
    pub target_x: f64,
    pub target_y: f64,
}

/// The adjustment block of `/status`.
#[derive(Debug, Clone, Serialize)]
pub struct AdjustmentStatus {
    pub updated_at: DateTime<Utc>,
    pub image_path: String,
    pub in_frame: bool,
    pub stars: Vec<StarTarget>,
    pub last_solve: String,
    pub consecutive_solve_failures: u32,
    pub iterations: u32,
}

/// Everything `GET /status` serves.
#[derive(Debug, Clone, Serialize)]
pub struct StatusState {
    pub phase: Phase,
    pub workflow_id: Option<String>,
    pub measurement: Option<MeasurementStatus>,
    pub adjustment: Option<AdjustmentStatus>,
    pub error: Option<String>,
}

impl Default for StatusState {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            workflow_id: None,
            measurement: None,
            adjustment: None,
            error: None,
        }
    }
}

/// Handles shared between the HTTP layer and the running workflow.
///
/// The finish signal is per-invocation: `Notify` stores a permit, so
/// a `/adjust/finish` that races the end of one run must not end the
/// next run's adjustment the instant it starts. Re-arming installs a
/// fresh `Notify`, and any stale permit dies with the old one.
#[derive(Clone, Default)]
pub struct WorkflowShared {
    pub status: Arc<RwLock<StatusState>>,
    finish: Arc<RwLock<Arc<Notify>>>,
}

impl WorkflowShared {
    /// Installs a fresh finish signal for a new invocation.
    pub async fn arm_finish(&self) {
        *self.finish.write().await = Arc::new(Notify::new());
    }

    /// The finish signal of the current invocation.
    pub async fn finish_signal(&self) -> Arc<Notify> {
        self.finish.read().await.clone()
    }

    /// Signals the current invocation's adjustment loop to end.
    pub async fn signal_finish(&self) {
        self.finish.read().await.notify_one();
    }
}

/// What the completion report carries.
#[derive(Debug)]
pub struct WorkflowSummary {
    pub final_measurement: MeasurementStatus,
    pub adjustment_iterations: u32,
}

fn measurement_status(
    axis_azimuth_deg: f64,
    axis_altitude_deg: f64,
    errors: &AlignmentErrors,
    measured_at: DateTime<Utc>,
) -> MeasurementStatus {
    MeasurementStatus {
        axis_azimuth_deg,
        axis_altitude_deg,
        azimuth_error_arcmin: errors.azimuth_error_deg * 60.0,
        altitude_error_arcmin: errors.altitude_error_deg * 60.0,
        total_error_arcmin: errors.total_error_deg * 60.0,
        azimuth_direction: errors.azimuth_direction().to_string(),
        altitude_direction: errors.altitude_direction().to_string(),
        measured_at,
    }
}

fn solved_frame(solve: &SolveResult) -> Option<SolvedFrame> {
    solve.wcs_matrix.map(|m| SolvedFrame {
        center_ra_deg: solve.ra_center,
        center_dec_deg: solve.dec_center,
        matrix: m.into(),
    })
}

/// Run the full polar-alignment workflow. The caller (routes) owns
/// posting the completion report from the returned summary or error.
pub async fn run(
    mcp: &McpClient,
    config: &PolarAlignConfig,
    shared: &WorkflowShared,
) -> Result<WorkflowSummary> {
    let eph = EphemerisCtx::from_config(config)?;

    let result = run_inner(mcp, config, shared, &eph).await;

    if let Err(ref e) = result {
        warn!(error = %e, "polar alignment workflow failed");
        // Stop-class cleanup only (tenet 3): halt any in-flight
        // motion, leave the mount tracking where it stands. abort_slew
        // is a no-op error when nothing is slewing; that is fine.
        if let Err(abort_err) = mcp.abort_slew().await {
            debug!(error = %abort_err, "cleanup abort_slew reported (expected when no slew is in flight)");
        }
        let mut status = shared.status.write().await;
        status.phase = Phase::Error;
        status.error = Some(e.to_string());
    }

    result
}

async fn run_inner(
    mcp: &McpClient,
    config: &PolarAlignConfig,
    shared: &WorkflowShared,
    eph: &EphemerisCtx,
) -> Result<WorkflowSummary> {
    // Preflight: sensor bounds for the overlay, park and tracking
    // state before any motion.
    let camera = mcp.get_camera_info(&config.camera_id).await?;

    let park = mcp.get_park_state().await?;
    if park.at_park {
        if !park.can_unpark {
            return Err(PolarAlignError::Workflow(
                "mount is parked and does not support unparking; unpark it manually first"
                    .to_string(),
            ));
        }
        debug!("mount is parked; unparking");
        mcp.unpark().await?;
    }

    let tracking = mcp.get_tracking().await?;
    if !tracking.tracking {
        if !tracking.can_set_tracking {
            return Err(PolarAlignError::Workflow(
                "tracking is off and the mount does not support enabling it; slews would fail"
                    .to_string(),
            ));
        }
        debug!("enabling sidereal tracking");
        mcp.set_tracking(true).await?;
    }

    // Three-point measurement sweep: equal-declination points on one
    // side of the meridian, hour angles first + i·sweep.
    let dec_deg = config.measurement_dec_deg();
    let ha_sign = config.measurement.direction.ha_sign();
    let mut centers: Vec<Vec3> = Vec::with_capacity(3);
    let mut last_solve: Option<SolveResult> = None;
    let mut last_capture_center = (0.0, 0.0);

    for i in 0..3u32 {
        let ha_deg = ha_sign
            * (config.measurement.first_point_ha_deg.degrees()
                + f64::from(i) * config.measurement.sweep_deg.degrees());
        let lst_hours = eph.lst_hours(Utc::now());
        let ra_deg = (lst_hours * 15.0 - ha_deg).rem_euclid(360.0);

        debug!(
            point = i + 1,
            ra_deg, dec_deg, ha_deg, "slewing to measurement point"
        );
        mcp.slew(ra_deg, dec_deg, config.measurement.settle).await?;

        let capture = mcp
            .capture(&config.camera_id, config.measurement.exposure)
            .await?;
        let solve = mcp
            .plate_solve(
                &capture.image_path,
                &capture.document_id,
                ra_deg,
                dec_deg,
                config.solve.search_radius_deg,
                config.solve.timeout,
            )
            .await?;
        debug!(
            point = i + 1,
            ra_center = solve.ra_center,
            dec_center = solve.dec_center,
            "measurement point solved"
        );
        centers.push(unit_from_radec(solve.ra_center, solve.dec_center));
        last_capture_center = (solve.ra_center, solve.dec_center);
        last_solve = Some(solve);
    }

    let mut axis = axis_from_three_points(
        centers[0],
        centers[1],
        centers[2],
        eph.pole_hemisphere_unit(),
    )?;

    let now = Utc::now();
    let observed = eph.observed_of(axis, now)?;
    let pole = eph.pole_target_alt_az();
    let errors = alignment_errors(
        observed.azimuth_degrees,
        observed.altitude_degrees,
        pole.azimuth_degrees,
        pole.altitude_degrees,
    );
    info!(
        azimuth_error_arcmin = errors.azimuth_error_deg * 60.0,
        altitude_error_arcmin = errors.altitude_error_deg * 60.0,
        "polar axis measured"
    );

    {
        let mut status = shared.status.write().await;
        status.measurement = Some(measurement_status(
            observed.azimuth_degrees,
            observed.altitude_degrees,
            &errors,
            now,
        ));
        status.phase = Phase::Adjusting;
    }

    // Adjustment loop. The attitude seed comes from the last
    // measurement solve when it carried a wcs_matrix; otherwise the
    // first matrix-bearing adjustment solve seeds it (the axis simply
    // doesn't move until then).
    let mut attitude_prev: Option<Mat3> = match last_solve.as_ref().and_then(solved_frame) {
        Some(frame) => Some(attitude_from_wcs(&frame)?),
        None => None,
    };
    let mut hint = last_capture_center;
    let mut consecutive_failures = 0u32;
    let mut iterations = 0u32;
    let deadline = tokio::time::Instant::now() + config.adjustment.max_duration;
    let mut final_measurement = measurement_status(
        observed.azimuth_degrees,
        observed.altitude_degrees,
        &errors,
        now,
    );

    let finish = shared.finish_signal().await;
    loop {
        tokio::select! {
            _ = finish.notified() => {
                info!("adjustment finished by operator");
                break;
            }
            _ = tokio::time::sleep_until(deadline) => {
                info!("adjustment reached its configured maximum duration");
                break;
            }
            _ = tokio::time::sleep(config.adjustment.interval) => {}
        }

        iterations += 1;
        match adjustment_iteration(
            mcp,
            config,
            eph,
            &mut axis,
            &mut attitude_prev,
            &mut hint,
            (camera.sensor_x, camera.sensor_y),
        )
        .await
        {
            Ok((measurement, mut adjustment)) => {
                consecutive_failures = 0;
                adjustment.consecutive_solve_failures = 0;
                adjustment.iterations = iterations;
                final_measurement = measurement.clone();
                let mut status = shared.status.write().await;
                status.measurement = Some(measurement);
                status.adjustment = Some(adjustment);
            }
            Err(e) => {
                consecutive_failures += 1;
                debug!(
                    error = %e,
                    consecutive_failures,
                    "adjustment iteration failed (expected while the operator moves the mount)"
                );
                if consecutive_failures >= config.adjustment.max_solve_failures {
                    return Err(PolarAlignError::Workflow(format!(
                        "{} consecutive adjustment solves failed (last: {}); aborting — \
                         check sky conditions",
                        consecutive_failures, e
                    )));
                }
                let mut status = shared.status.write().await;
                let adjustment = status.adjustment.get_or_insert_with(|| AdjustmentStatus {
                    updated_at: Utc::now(),
                    image_path: String::new(),
                    in_frame: false,
                    stars: Vec::new(),
                    last_solve: "failed".to_string(),
                    consecutive_solve_failures: 0,
                    iterations: 0,
                });
                adjustment.updated_at = Utc::now();
                adjustment.last_solve = "failed".to_string();
                adjustment.consecutive_solve_failures = consecutive_failures;
                adjustment.iterations = iterations;
            }
        }
    }

    {
        let mut status = shared.status.write().await;
        status.phase = Phase::Complete;
    }

    Ok(WorkflowSummary {
        final_measurement,
        adjustment_iterations: iterations,
    })
}

/// One adjustment iteration: capture, solve, update the axis through
/// the attitude change, recompute errors, and build the star/target
/// overlay.
#[allow(clippy::too_many_arguments)]
async fn adjustment_iteration(
    mcp: &McpClient,
    config: &PolarAlignConfig,
    eph: &EphemerisCtx,
    axis: &mut Vec3,
    attitude_prev: &mut Option<Mat3>,
    hint: &mut (f64, f64),
    sensor: (u32, u32),
) -> Result<(MeasurementStatus, AdjustmentStatus)> {
    let capture = mcp
        .capture(&config.camera_id, config.adjustment.exposure)
        .await?;
    let solve = mcp
        .plate_solve(
            &capture.image_path,
            &capture.document_id,
            hint.0,
            hint.1,
            config.solve.search_radius_deg,
            config.solve.timeout,
        )
        .await?;
    *hint = (solve.ra_center, solve.dec_center);

    let frame = solved_frame(&solve).ok_or_else(|| {
        PolarAlignError::Workflow(
            "solve carried no wcs_matrix; the adjustment loop needs the full WCS".to_string(),
        )
    })?;
    let attitude_now = attitude_from_wcs(&frame)?;
    if let Some(prev) = attitude_prev.take() {
        let rotation = relative_rotation(prev, attitude_now);
        *axis = rotation.mul_vec(*axis).normalized()?;
    }
    *attitude_prev = Some(attitude_now);

    let now = Utc::now();
    let observed = eph.observed_of(*axis, now)?;
    let pole = eph.pole_target_alt_az();
    let errors = alignment_errors(
        observed.azimuth_degrees,
        observed.altitude_degrees,
        pole.azimuth_degrees,
        pole.altitude_degrees,
    );

    // Star/target overlay: where each bright star lands once the
    // axis is on the pole. Correction rotation in ICRS; the target
    // pixel of a star at sky direction s is W⁻¹(R_corrᵀ · s).
    let target_icrs = eph.axis_target_icrs(now)?;
    let correction_t = rotation_between(*axis, target_icrs)?.transpose();

    let detected = mcp
        .detect_stars(&capture.image_path, &capture.document_id)
        .await?;
    let mut brightest: Vec<_> = detected
        .stars
        .into_iter()
        .filter(|s| s.saturated_pixel_count == 0)
        .collect();
    brightest.sort_by(|a, b| b.flux.total_cmp(&a.flux));
    brightest.truncate(config.adjustment.star_count);

    let (stars, in_frame) = star_overlay(&frame, correction_t, brightest, sensor)?;

    Ok((
        measurement_status(
            observed.azimuth_degrees,
            observed.altitude_degrees,
            &errors,
            now,
        ),
        AdjustmentStatus {
            updated_at: now,
            image_path: capture.image_path,
            in_frame,
            stars,
            last_solve: "ok".to_string(),
            consecutive_solve_failures: 0,
            iterations: 0,
        },
    ))
}

/// Pairs each detected star with the pixel it will occupy once the
/// correction rotation is applied, and reports whether every target
/// stays on the sensor. Detected centroids are 0-based array indices
/// while the WCS math speaks FITS 1-based pixels, so coordinates
/// shift by one on the way into the WCS and back on the way out;
/// `/status` publishes both halves of each pair 0-based.
fn star_overlay(
    frame: &SolvedFrame,
    correction_t: Mat3,
    brightest: Vec<DetectedStar>,
    sensor: (u32, u32),
) -> Result<(Vec<StarTarget>, bool)> {
    let mut stars = Vec::with_capacity(brightest.len());
    let mut in_frame = !brightest.is_empty();
    for star in brightest {
        let sky = wcs_pixel_to_sky(frame, star.x + 1.0, star.y + 1.0)?;
        match wcs_sky_to_pixel(frame, correction_t.mul_vec(sky))? {
            Some((fits_x, fits_y)) => {
                if !(1.0..=f64::from(sensor.0)).contains(&fits_x)
                    || !(1.0..=f64::from(sensor.1)).contains(&fits_y)
                {
                    in_frame = false;
                }
                stars.push(StarTarget {
                    x: star.x,
                    y: star.y,
                    target_x: fits_x - 1.0,
                    target_y: fits_y - 1.0,
                });
            }
            None => in_frame = false,
        }
    }
    Ok((stars, in_frame))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn test_status_state_defaults_to_idle() {
        let status = StatusState::default();
        assert_eq!(status.phase, Phase::Idle);
        assert!(status.workflow_id.is_none());
        assert!(status.measurement.is_none());
        assert!(status.adjustment.is_none());
    }

    #[test]
    fn test_phase_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Phase::Adjusting).unwrap(),
            "\"adjusting\""
        );
        assert_eq!(serde_json::to_string(&Phase::Idle).unwrap(), "\"idle\"");
    }

    #[test]
    fn test_measurement_status_converts_degrees_to_arcminutes() {
        let errors = AlignmentErrors {
            azimuth_error_deg: 0.35,
            altitude_error_deg: -0.2,
            total_error_deg: 0.4,
        };
        let status = measurement_status(0.5, 48.2, &errors, Utc::now());
        assert!((status.azimuth_error_arcmin - 21.0).abs() < 1e-9);
        assert!((status.altitude_error_arcmin + 12.0).abs() < 1e-9);
        assert!((status.total_error_arcmin - 24.0).abs() < 1e-9);
        assert_eq!(status.azimuth_direction, "move azimuth west");
        assert_eq!(status.altitude_direction, "raise altitude");
    }

    #[test]
    fn test_solved_frame_requires_the_matrix() {
        let json = r#"{
            "ra_center": 52.1, "dec_center": 85.2,
            "pixel_scale_arcsec": 1.05, "rotation_deg": 12.0
        }"#;
        let solve: SolveResult = serde_json::from_str(json).unwrap();
        assert!(solved_frame(&solve).is_none());
    }

    #[test]
    fn test_star_overlay_speaks_zero_based_outside_and_one_based_into_the_wcs() {
        use crate::math::WcsMatrix;

        // Square-pixel frame; a 90° rotation about the boresight is an
        // exact pixel rotation about CRPIX, which turns any 0/1-based
        // mixup into a full-pixel error instead of a second-order one.
        let frame = SolvedFrame {
            center_ra_deg: 100.0,
            center_dec_deg: 40.0,
            matrix: WcsMatrix {
                crpix1: 512.0,
                crpix2: 384.0,
                cd1_1: -2.9167e-4,
                cd1_2: 0.0,
                cd2_1: 0.0,
                cd2_2: 2.9167e-4,
            },
        };
        let boresight = unit_from_radec(100.0, 40.0);
        let correction_t = Mat3::from_axis_angle(boresight, std::f64::consts::FRAC_PI_2);
        // 50 pixels right of the rotation center, in 0-based indices.
        let star = DetectedStar {
            x: 561.0,
            y: 383.0,
            flux: 1000.0,
            saturated_pixel_count: 0,
        };

        let (stars, in_frame) =
            star_overlay(&frame, correction_t, vec![star], (1024, 768)).unwrap();

        assert!(in_frame);
        assert_eq!(stars[0].x, 561.0);
        assert_eq!(stars[0].y, 383.0);
        // The rotation center in 0-based indices is CRPIX − 1.
        let (dx, dy) = (stars[0].target_x - 511.0, stars[0].target_y - 383.0);
        let radius = (dx * dx + dy * dy).sqrt();
        assert!(
            (radius - 50.0).abs() < 1e-6,
            "target must stay 50 px from the rotation center, got {radius}"
        );
        assert!(
            (50.0 * dx).abs() < 1e-3,
            "a 90° rotation must land the target perpendicular to the star offset, got dot {}",
            50.0 * dx
        );
    }

    #[tokio::test]
    async fn test_stale_finish_permit_does_not_survive_re_arming() {
        let shared = WorkflowShared::default();
        // A finish that raced the previous run's end leaves a permit.
        shared.signal_finish().await;
        shared.arm_finish().await;
        let armed = shared.finish_signal().await;
        let woke = tokio::time::timeout(std::time::Duration::from_millis(50), armed.notified());
        assert!(
            woke.await.is_err(),
            "a stale permit leaked into the new invocation"
        );
    }

    #[tokio::test]
    async fn test_finish_signal_sent_before_the_loop_waits_still_ends_adjustment() {
        let shared = WorkflowShared::default();
        shared.arm_finish().await;
        shared.signal_finish().await;
        let signal = shared.finish_signal().await;
        tokio::time::timeout(std::time::Duration::from_secs(1), signal.notified())
            .await
            .unwrap();
    }
}
