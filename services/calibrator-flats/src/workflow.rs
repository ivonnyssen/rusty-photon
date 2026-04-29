//! Flat calibration workflow: iterative exposure optimization + batch capture.

use std::time::Duration;

use tracing::{debug, info, warn};

use crate::config::FlatPlan;
use crate::error::{CalibratorFlatsError, Result};
use crate::mcp_client::McpClient;

/// Result of the flat calibration workflow.
#[derive(Debug)]
pub struct WorkflowResult {
    pub filters_completed: Vec<FilterResult>,
    pub total_frames: u32,
}

/// Result for a single filter.
#[derive(Debug)]
pub struct FilterResult {
    pub filter_name: String,
    pub duration: Duration,
    pub median_adu: u32,
    pub frames_captured: u32,
    pub iterations: u32,
    pub converged: bool,
}

/// Run the full flat calibration workflow.
///
/// 1. Query camera capabilities
/// 2. Close cover and turn on calibrator
/// 3. For each filter: find optimal exposure, capture N frames
/// 4. Turn off calibrator and open cover (always, even on error)
pub async fn run(mcp: &McpClient, plan: &FlatPlan) -> Result<WorkflowResult> {
    // 1. Get camera info
    let camera_info = mcp.get_camera_info(&plan.camera_id).await?;
    let target_adu = (camera_info.max_adu as f64 * plan.target_adu_fraction) as u32;

    info!(
        max_adu = camera_info.max_adu,
        target_adu = target_adu,
        filters = plan.filters.len(),
        "starting calibrator flats calibration"
    );

    // 2. Prepare flat panel
    mcp.close_cover(&plan.calibrator_id).await?;
    mcp.calibrator_on(&plan.calibrator_id, plan.brightness)
        .await?;

    // 3. Capture flats (with cleanup guard)
    let result = run_capture_loop(mcp, plan, target_adu, &camera_info).await;

    // 4. Always clean up
    if let Err(e) = mcp.calibrator_off(&plan.calibrator_id).await {
        warn!(error = %e, "failed to turn calibrator off during cleanup");
    }
    if let Err(e) = mcp.open_cover(&plan.calibrator_id).await {
        warn!(error = %e, "failed to open cover during cleanup");
    }

    result
}

async fn run_capture_loop(
    mcp: &McpClient,
    plan: &FlatPlan,
    target_adu: u32,
    camera_info: &crate::mcp_client::CameraInfo,
) -> Result<WorkflowResult> {
    let mut filters_completed = Vec::new();
    let mut total_frames = 0u32;

    for filter in &plan.filters {
        debug!(filter = %filter.name, count = filter.count, "switching filter");
        mcp.set_filter(&plan.filter_wheel_id, &filter.name).await?;

        // Find optimal exposure time
        let (duration, median_adu, iterations, converged) =
            find_optimal_duration(mcp, plan, target_adu, camera_info).await?;

        if converged {
            info!(
                filter = %filter.name,
                duration = %humantime::format_duration(duration),
                median_adu = median_adu,
                iterations = iterations,
                "exposure converged"
            );
        } else {
            warn!(
                filter = %filter.name,
                duration = %humantime::format_duration(duration),
                median_adu = median_adu,
                iterations = iterations,
                "exposure did not converge, using best duration"
            );
        }

        // Capture the requested number of flat frames
        for i in 1..=filter.count {
            debug!(filter = %filter.name, frame = i, total = filter.count, "capturing flat");
            mcp.capture(&plan.camera_id, duration).await?;
        }

        total_frames += filter.count;
        filters_completed.push(FilterResult {
            filter_name: filter.name.clone(),
            duration,
            median_adu,
            frames_captured: filter.count,
            iterations,
            converged,
        });
    }

    Ok(WorkflowResult {
        filters_completed,
        total_frames,
    })
}

/// Proportionally adjust exposure to hit `target_adu`, clamped to the
/// camera's exposure range.
///
/// `new = current * (target_adu / last_median)`, with a doubling guard
/// when `last_median == 0` to escape the division-by-zero case.
fn next_duration(
    current: Duration,
    target_adu: u32,
    last_median: u32,
    exposure_min: Duration,
    exposure_max: Duration,
) -> Duration {
    let adjusted = if last_median == 0 {
        current.saturating_mul(2)
    } else {
        let ratio = target_adu as f64 / last_median as f64;
        current.mul_f64(ratio)
    };
    adjusted.max(exposure_min).min(exposure_max)
}

/// Fractional deviation of a measured ADU from the target.
fn deviation(target_adu: u32, last_median: u32) -> f64 {
    (last_median as f64 - target_adu as f64).abs() / target_adu as f64
}

/// Iteratively adjust exposure time to hit the target ADU.
///
/// Returns `(duration, last_median_adu, iterations, converged)`.
async fn find_optimal_duration(
    mcp: &McpClient,
    plan: &FlatPlan,
    target_adu: u32,
    camera_info: &crate::mcp_client::CameraInfo,
) -> Result<(Duration, u32, u32, bool)> {
    if target_adu == 0 {
        return Err(CalibratorFlatsError::Workflow(
            "target_adu is 0 (max_adu * fraction = 0)".into(),
        ));
    }

    let mut duration = plan.initial_duration;
    let mut last_median = 0u32;

    for iteration in 1..=plan.max_iterations {
        let capture_result = mcp.capture(&plan.camera_id, duration).await?;
        let stats = mcp
            .compute_image_stats(
                &capture_result.image_path,
                Some(&capture_result.document_id),
            )
            .await?;

        last_median = stats.median_adu;
        let dev = deviation(target_adu, last_median);

        debug!(
            iteration = iteration,
            duration = %humantime::format_duration(duration),
            median_adu = last_median,
            target_adu = target_adu,
            deviation = %format!("{:.1}%", dev * 100.0),
            "exposure iteration"
        );

        if dev <= plan.tolerance {
            return Ok((duration, last_median, iteration, true));
        }

        duration = next_duration(
            duration,
            target_adu,
            last_median,
            camera_info.exposure_min,
            camera_info.exposure_max,
        );
    }

    // Did not converge, return best effort
    Ok((duration, last_median, plan.max_iterations, false))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{deviation, next_duration};
    use std::time::Duration;

    const MIN: Duration = Duration::from_micros(10);
    const MAX: Duration = Duration::from_secs(3600);

    #[test]
    fn next_duration_doubles_for_half_signal() {
        let next = next_duration(Duration::from_secs(1), 32_000, 16_000, MIN, MAX);
        assert_eq!(next, Duration::from_secs(2));
    }

    #[test]
    fn next_duration_halves_for_double_signal() {
        let next = next_duration(Duration::from_secs(1), 32_000, 64_000, MIN, MAX);
        assert_eq!(next, Duration::from_millis(500));
    }

    #[test]
    fn next_duration_doubles_when_zero_signal() {
        let next = next_duration(Duration::from_millis(500), 32_000, 0, MIN, MAX);
        assert_eq!(next, Duration::from_secs(1));
    }

    #[test]
    fn next_duration_clamps_to_exposure_min() {
        // Heavily over-exposed: ratio drives adjustment far below MIN.
        let next = next_duration(Duration::from_millis(1), 1_000, 1_000_000, MIN, MAX);
        assert_eq!(next, MIN);
    }

    #[test]
    fn next_duration_clamps_to_exposure_max() {
        // Heavily under-exposed: ratio drives adjustment past MAX.
        let next = next_duration(Duration::from_secs(1), 60_000, 1, MIN, MAX);
        assert_eq!(next, MAX);
    }

    #[test]
    fn next_duration_zero_signal_clamps_to_exposure_max() {
        // Doubling guard would still exceed MAX.
        let next = next_duration(Duration::from_secs(2400), 32_000, 0, MIN, MAX);
        assert_eq!(next, MAX);
    }

    #[test]
    fn next_duration_preserves_microsecond_precision() {
        // 50 µs bias-class exposure — sub-ms input must produce sub-ms output.
        let next = next_duration(Duration::from_micros(50), 32_000, 32_000, MIN, MAX);
        assert_eq!(next, Duration::from_micros(50));
    }

    #[test]
    fn next_duration_saturating_mul_does_not_overflow() {
        // `Duration::MAX * 2` saturates rather than panicking.
        let next = next_duration(Duration::MAX, 1, 0, MIN, MAX);
        assert_eq!(next, MAX);
    }

    #[test]
    fn deviation_zero_when_on_target() {
        assert_eq!(deviation(32_000, 32_000), 0.0);
    }

    #[test]
    fn deviation_symmetric_above_and_below() {
        assert_eq!(deviation(32_000, 16_000), 0.5);
        assert_eq!(deviation(32_000, 48_000), 0.5);
    }
}
