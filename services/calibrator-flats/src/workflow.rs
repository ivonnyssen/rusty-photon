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
    pub duration_ms: u32,
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
        let (duration_ms, median_adu, iterations, converged) =
            find_optimal_duration(mcp, plan, target_adu, camera_info).await?;

        if converged {
            info!(
                filter = %filter.name,
                duration_ms = duration_ms,
                median_adu = median_adu,
                iterations = iterations,
                "exposure converged"
            );
        } else {
            warn!(
                filter = %filter.name,
                duration_ms = duration_ms,
                median_adu = median_adu,
                iterations = iterations,
                "exposure did not converge, using best duration"
            );
        }

        // Capture the requested number of flat frames
        let capture_duration = Duration::from_millis(duration_ms as u64);
        for i in 1..=filter.count {
            debug!(filter = %filter.name, frame = i, total = filter.count, "capturing flat");
            mcp.capture(&plan.camera_id, capture_duration).await?;
        }

        total_frames += filter.count;
        filters_completed.push(FilterResult {
            filter_name: filter.name.clone(),
            duration_ms,
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

/// Iteratively adjust exposure time to hit the target ADU.
///
/// Uses proportional adjustment: `new_duration = old_duration * (target / measured)`.
/// Returns `(duration_ms, last_median_adu, iterations, converged)`.
async fn find_optimal_duration(
    mcp: &McpClient,
    plan: &FlatPlan,
    target_adu: u32,
    camera_info: &crate::mcp_client::CameraInfo,
) -> Result<(u32, u32, u32, bool)> {
    let mut duration_ms = plan.initial_duration.as_millis() as u32;
    let mut last_median = 0u32;

    for iteration in 1..=plan.max_iterations {
        let capture_result = mcp
            .capture(&plan.camera_id, Duration::from_millis(duration_ms as u64))
            .await?;
        let stats = mcp
            .compute_image_stats(
                &capture_result.image_path,
                Some(&capture_result.document_id),
            )
            .await?;

        last_median = stats.median_adu;

        let deviation = if target_adu > 0 {
            (last_median as f64 - target_adu as f64).abs() / target_adu as f64
        } else {
            return Err(CalibratorFlatsError::Workflow(
                "target_adu is 0 (max_adu * fraction = 0)".into(),
            ));
        };

        debug!(
            iteration = iteration,
            duration_ms = duration_ms,
            median_adu = last_median,
            target_adu = target_adu,
            deviation = %format!("{:.1}%", deviation * 100.0),
            "exposure iteration"
        );

        if deviation <= plan.tolerance {
            return Ok((duration_ms, last_median, iteration, true));
        }

        // Adjust proportionally
        duration_ms = if last_median == 0 {
            // Guard division by zero: double the duration
            duration_ms.saturating_mul(2)
        } else {
            let ratio = target_adu as f64 / last_median as f64;
            (duration_ms as f64 * ratio) as u32
        };

        // Clamp to camera limits
        duration_ms = duration_ms
            .max(camera_info.exposure_min_ms as u32)
            .min(camera_info.exposure_max_ms as u32);
    }

    // Did not converge, return best effort
    Ok((duration_ms, last_median, plan.max_iterations, false))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn proportional_adjustment_doubles_for_half_signal() {
        let current_ms = 1000u32;
        let target = 32000u32;
        let measured = 16000u32;
        let ratio = target as f64 / measured as f64;
        let new_ms = (current_ms as f64 * ratio) as u32;
        assert_eq!(new_ms, 2000);
    }

    #[test]
    fn proportional_adjustment_halves_for_double_signal() {
        let current_ms = 1000u32;
        let target = 32000u32;
        let measured = 64000u32;
        let ratio = target as f64 / measured as f64;
        let new_ms = (current_ms as f64 * ratio) as u32;
        assert_eq!(new_ms, 500);
    }

    #[test]
    fn zero_measurement_doubles_duration() {
        let duration_ms = 500u32;
        let result = duration_ms.saturating_mul(2);
        assert_eq!(result, 1000);
    }
}
