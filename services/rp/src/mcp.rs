use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::cover_calibrator::{CalibratorStatus, CoverStatus};
use ascom_alpaca::api::CoverCalibrator;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tracing::debug;
use uuid::Uuid;

use crate::equipment::EquipmentRegistry;
use crate::events::EventBus;
use crate::imaging;
use crate::session::SessionConfig;

// ---------------------------------------------------------------------------
// Parameter structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CaptureParams {
    /// Camera device ID
    pub camera_id: String,
    /// Exposure time in milliseconds
    pub duration_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CameraIdParams {
    /// Camera device ID
    pub camera_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ComputeImageStatsParams {
    /// Filesystem path to FITS image file
    pub image_path: String,
    /// Optional: document ID for tracking
    #[serde(default)]
    pub document_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetFilterParams {
    /// Filter wheel device ID
    pub filter_wheel_id: String,
    /// Filter name (must match filter wheel configuration)
    pub filter_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FilterWheelIdParams {
    /// Filter wheel device ID
    pub filter_wheel_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CalibratorIdParams {
    /// CoverCalibrator device ID
    pub calibrator_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CalibratorOnParams {
    /// CoverCalibrator device ID
    pub calibrator_id: String,
    /// Brightness level (0..max_brightness). Defaults to max if omitted
    #[serde(default)]
    pub brightness: Option<u32>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tool_success(result: serde_json::Value) -> CallToolResult {
    CallToolResult::success(vec![Content::text(result.to_string())])
}

fn tool_error(msg: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![Content::text(msg.to_string())])
}

// ---------------------------------------------------------------------------
// McpHandler
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct McpHandler {
    pub equipment: Arc<EquipmentRegistry>,
    pub event_bus: Arc<EventBus>,
    pub session_config: SessionConfig,
}

impl McpHandler {
    pub fn new(
        equipment: Arc<EquipmentRegistry>,
        event_bus: Arc<EventBus>,
        session_config: SessionConfig,
    ) -> Self {
        Self {
            equipment,
            event_bus,
            session_config,
        }
    }

    /// Resolve camera_id, look up device.
    fn resolve_camera(
        &self,
        camera_id: &str,
    ) -> Result<
        (
            &crate::equipment::CameraEntry,
            Arc<dyn ascom_alpaca::api::Camera>,
        ),
        CallToolResult,
    > {
        let cam_entry = self
            .equipment
            .find_camera(camera_id)
            .ok_or_else(|| tool_error(format!("camera not found: {}", camera_id)))?;

        let cam = cam_entry
            .device
            .clone()
            .ok_or_else(|| tool_error(format!("camera not connected: {}", camera_id)))?;

        Ok((cam_entry, cam))
    }

    /// Resolve filter_wheel_id, look up entry and device.
    fn resolve_filter_wheel(
        &self,
        fw_id: &str,
    ) -> Result<
        (
            &crate::equipment::FilterWheelEntry,
            Arc<dyn ascom_alpaca::api::FilterWheel>,
        ),
        CallToolResult,
    > {
        let fw_entry = self
            .equipment
            .find_filter_wheel(fw_id)
            .ok_or_else(|| tool_error(format!("filter wheel not found: {}", fw_id)))?;

        let fw = fw_entry
            .device
            .clone()
            .ok_or_else(|| tool_error(format!("filter wheel not connected: {}", fw_id)))?;

        Ok((fw_entry, fw))
    }

    /// Resolve calibrator_id, look up device and poll interval.
    fn resolve_calibrator(
        &self,
        calibrator_id: &str,
    ) -> Result<(Arc<dyn CoverCalibrator>, Duration), CallToolResult> {
        let cc_entry = self
            .equipment
            .find_cover_calibrator(calibrator_id)
            .ok_or_else(|| tool_error(format!("calibrator not found: {}", calibrator_id)))?;

        let cc = cc_entry
            .device
            .clone()
            .ok_or_else(|| tool_error(format!("calibrator not connected: {}", calibrator_id)))?;

        let poll_interval = Duration::from_secs(cc_entry.config.poll_interval_secs);
        Ok((cc, poll_interval))
    }
}

#[tool_router(server_handler)]
impl McpHandler {
    // -------------------------------------------------------------------
    // Camera tools
    // -------------------------------------------------------------------

    #[tool(description = "Capture an image, download image_array, save FITS file")]
    async fn capture(
        &self,
        Parameters(params): Parameters<CaptureParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let duration = Duration::from_millis(params.duration_ms);

        let (_cam_entry, cam) = match self.resolve_camera(&params.camera_id) {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };

        let document_id = Uuid::new_v4().to_string();
        let image_path = format!(
            "{}/capture_{}.fits",
            self.session_config.data_directory, document_id
        );

        self.event_bus.emit(
            "exposure_started",
            serde_json::json!({
                "camera_id": params.camera_id,
                "duration_ms": duration.as_millis() as u64,
            }),
        );

        if let Err(e) = cam.start_exposure(duration, true).await {
            return Ok(tool_error(format!("failed to start exposure: {}", e)));
        }

        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match cam.image_ready().await {
                Ok(true) => break,
                Ok(false) => continue,
                Err(e) => {
                    return Ok(tool_error(format!("error checking image ready: {}", e)));
                }
            }
        }

        let image_array = match cam.image_array().await {
            Ok(arr) => arr,
            Err(e) => {
                return Ok(tool_error(format!("failed to download image array: {}", e)));
            }
        };

        let (dim_x, dim_y, _planes) = image_array.dim();
        let width = dim_x as u32;
        let height = dim_y as u32;
        let pixels: Vec<i32> = image_array.iter().copied().collect();

        if let Err(e) = imaging::write_fits(&image_path, &pixels, width, height).await {
            return Ok(tool_error(format!("failed to write FITS file: {}", e)));
        }

        self.event_bus.emit(
            "exposure_complete",
            serde_json::json!({
                "document_id": document_id,
                "file_path": image_path,
            }),
        );

        Ok(tool_success(serde_json::json!({
            "image_path": image_path,
            "document_id": document_id,
        })))
    }

    #[tool(description = "Read camera capabilities: max_adu, exposure limits, sensor dimensions")]
    async fn get_camera_info(
        &self,
        Parameters(params): Parameters<CameraIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_cam_entry, cam) = match self.resolve_camera(&params.camera_id) {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };

        let max_adu = match cam.max_adu().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error(format!("failed to read max_adu: {}", e))),
        };

        let (sensor_x, sensor_y) = match cam.camera_size().await {
            Ok(size) => (size[0], size[1]),
            Err(e) => return Ok(tool_error(format!("failed to read sensor size: {}", e))),
        };

        let (bin_x, bin_y) = match cam.bin().await {
            Ok(bin) => (bin[0] as u32, bin[1] as u32),
            Err(e) => {
                debug!(error = %e, "failed to read binning, using defaults");
                (1u32, 1u32)
            }
        };

        let (exposure_min_ms, exposure_max_ms) = match cam.exposure_range().await {
            Ok(range) => (
                range.start().as_millis() as u64,
                range.end().as_millis() as u64,
            ),
            Err(e) => {
                debug!(error = %e, "failed to read exposure range, using defaults");
                (1u64, 3600000u64)
            }
        };

        Ok(tool_success(serde_json::json!({
            "camera_id": params.camera_id,
            "max_adu": max_adu,
            "sensor_x": sensor_x,
            "sensor_y": sensor_y,
            "bin_x": bin_x,
            "bin_y": bin_y,
            "exposure_min_ms": exposure_min_ms,
            "exposure_max_ms": exposure_max_ms,
        })))
    }

    // -------------------------------------------------------------------
    // Image stats tool
    // -------------------------------------------------------------------

    #[tool(
        description = "Read FITS file and compute pixel statistics (median, mean, min, max ADU)"
    )]
    async fn compute_image_stats(
        &self,
        Parameters(params): Parameters<ComputeImageStatsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let image_path = params.image_path;

        let path_clone = image_path.clone();
        let stats = match tokio::task::spawn_blocking(move || {
            let pixels = imaging::read_fits_pixels(&path_clone)?;
            imaging::compute_stats(&pixels)
                .ok_or_else(|| crate::error::RpError::Imaging("image has no pixels".into()))
        })
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Ok(tool_error(format!("failed to compute stats: {}", e))),
            Err(e) => return Ok(tool_error(format!("task error: {}", e))),
        };

        debug!(
            image_path = %image_path,
            median = stats.median_adu,
            mean = %stats.mean_adu,
            "computed image stats"
        );

        Ok(tool_success(serde_json::json!({
            "median_adu": stats.median_adu,
            "mean_adu": stats.mean_adu,
            "min_adu": stats.min_adu,
            "max_adu": stats.max_adu,
            "pixel_count": stats.pixel_count,
        })))
    }

    // -------------------------------------------------------------------
    // Filter wheel tools
    // -------------------------------------------------------------------

    #[tool(description = "Set the active filter on a filter wheel")]
    async fn set_filter(
        &self,
        Parameters(params): Parameters<SetFilterParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (fw_entry, fw) = match self.resolve_filter_wheel(&params.filter_wheel_id) {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };

        let position = match fw_entry
            .config
            .filters
            .iter()
            .position(|f| f == &params.filter_name)
        {
            Some(p) => p,
            None => {
                return Ok(tool_error(format!(
                    "filter not found: {}",
                    params.filter_name
                )))
            }
        };

        if let Err(e) = fw.set_position(position).await {
            return Ok(tool_error(format!("failed to set filter position: {}", e)));
        }

        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match fw.position().await {
                Ok(Some(p)) if p == position => break,
                Ok(Some(_)) | Ok(None) => continue,
                Err(e) => {
                    return Ok(tool_error(format!("error waiting for filter wheel: {}", e)));
                }
            }
        }

        self.event_bus.emit(
            "filter_switch",
            serde_json::json!({
                "filter_wheel_id": params.filter_wheel_id,
                "filter_name": params.filter_name,
            }),
        );

        Ok(tool_success(serde_json::json!({
            "filter_wheel_id": params.filter_wheel_id,
            "filter_name": params.filter_name,
            "position": position,
        })))
    }

    #[tool(description = "Get the current filter on a filter wheel")]
    async fn get_filter(
        &self,
        Parameters(params): Parameters<FilterWheelIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (fw_entry, fw) = match self.resolve_filter_wheel(&params.filter_wheel_id) {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };

        let position = match fw.position().await {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(tool_error("filter wheel is moving")),
            Err(e) => {
                return Ok(tool_error(format!("failed to get filter position: {}", e)));
            }
        };

        let filter_name = fw_entry
            .config
            .filters
            .get(position)
            .cloned()
            .unwrap_or_else(|| format!("Filter {}", position));

        Ok(tool_success(serde_json::json!({
            "filter_wheel_id": params.filter_wheel_id,
            "filter_name": filter_name,
            "position": position,
        })))
    }

    // -------------------------------------------------------------------
    // CoverCalibrator tools
    // -------------------------------------------------------------------

    #[tool(description = "Close the dust cover (blocks until closed)")]
    async fn close_cover(
        &self,
        Parameters(params): Parameters<CalibratorIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc, poll_interval) = match self.resolve_calibrator(&params.calibrator_id) {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };

        debug!(calibrator_id = %params.calibrator_id, "closing cover");
        if let Err(e) = cc.close_cover().await {
            return Ok(tool_error(format!("failed to close cover: {}", e)));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.cover_state().await {
                Ok(CoverStatus::Closed) => {
                    debug!(calibrator_id = %params.calibrator_id, "cover closed");
                    return Ok(tool_success(serde_json::json!({"status": "closed"})));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error(format!("error polling cover state: {}", e)));
                }
            }
        }

        Ok(tool_error("timeout waiting for cover to close"))
    }

    #[tool(description = "Open the dust cover (blocks until open)")]
    async fn open_cover(
        &self,
        Parameters(params): Parameters<CalibratorIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc, poll_interval) = match self.resolve_calibrator(&params.calibrator_id) {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };

        debug!(calibrator_id = %params.calibrator_id, "opening cover");
        if let Err(e) = cc.open_cover().await {
            return Ok(tool_error(format!("failed to open cover: {}", e)));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.cover_state().await {
                Ok(CoverStatus::Open) => {
                    debug!(calibrator_id = %params.calibrator_id, "cover opened");
                    return Ok(tool_success(serde_json::json!({"status": "open"})));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error(format!("error polling cover state: {}", e)));
                }
            }
        }

        Ok(tool_error("timeout waiting for cover to open"))
    }

    #[tool(description = "Turn on flat panel at brightness (default: max). Blocks until ready")]
    async fn calibrator_on(
        &self,
        Parameters(params): Parameters<CalibratorOnParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc, poll_interval) = match self.resolve_calibrator(&params.calibrator_id) {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };

        let brightness = if let Some(b) = params.brightness {
            b
        } else {
            match cc.max_brightness().await {
                Ok(max) => max,
                Err(e) => return Ok(tool_error(format!("failed to read max_brightness: {}", e))),
            }
        };

        debug!(calibrator_id = %params.calibrator_id, brightness = brightness, "turning calibrator on");
        if let Err(e) = cc.calibrator_on(brightness).await {
            return Ok(tool_error(format!("failed to turn calibrator on: {}", e)));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.calibrator_state().await {
                Ok(CalibratorStatus::Ready) => {
                    debug!(calibrator_id = %params.calibrator_id, "calibrator ready");
                    return Ok(tool_success(
                        serde_json::json!({"status": "ready", "brightness": brightness}),
                    ));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error(format!("error polling calibrator state: {}", e)));
                }
            }
        }

        Ok(tool_error("timeout waiting for calibrator to become ready"))
    }

    #[tool(description = "Turn off flat panel. Blocks until off")]
    async fn calibrator_off(
        &self,
        Parameters(params): Parameters<CalibratorIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc, poll_interval) = match self.resolve_calibrator(&params.calibrator_id) {
            Ok(v) => v,
            Err(e) => return Ok(e),
        };

        debug!(calibrator_id = %params.calibrator_id, "turning calibrator off");
        if let Err(e) = cc.calibrator_off().await {
            return Ok(tool_error(format!("failed to turn calibrator off: {}", e)));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.calibrator_state().await {
                Ok(CalibratorStatus::Off) => {
                    debug!(calibrator_id = %params.calibrator_id, "calibrator off");
                    return Ok(tool_success(serde_json::json!({"status": "off"})));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error(format!("error polling calibrator state: {}", e)));
                }
            }
        }

        Ok(tool_error("timeout waiting for calibrator to turn off"))
    }
}
