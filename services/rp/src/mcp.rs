use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::cover_calibrator::{CalibratorStatus, CoverStatus};
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
// Macros
// ---------------------------------------------------------------------------

/// Build a successful `CallToolResult` from a `serde_json::json!(...)` value.
macro_rules! tool_success {
    ($($json:tt)+) => {
        CallToolResult::success(vec![Content::text(
            serde_json::json!($($json)+).to_string(),
        )])
    };
}

/// Build an error `CallToolResult` from a format string or literal.
macro_rules! tool_error {
    ($lit:literal) => {
        CallToolResult::error(vec![Content::text($lit)])
    };
    ($($arg:tt)+) => {
        CallToolResult::error(vec![Content::text(format!($($arg)+))])
    };
}

/// Look up a device by ID and return the entry + connected device, or
/// early-return a `tool_error` `CallToolResult` from the enclosing function.
///
/// Usage: `let (entry, device) = resolve_device!(self, find_camera, id, "camera");`
macro_rules! resolve_device {
    ($self:expr, $finder:ident, $id:expr, $kind:literal) => {{
        let entry = match $self.equipment.$finder($id) {
            Some(e) => e,
            None => return Ok(tool_error!(concat!($kind, " not found: {}"), $id)),
        };
        let device = match &entry.device {
            Some(d) => d.clone(),
            None => return Ok(tool_error!(concat!($kind, " not connected: {}"), $id)),
        };
        (entry, device)
    }};
}

// ---------------------------------------------------------------------------
// Parameter structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CaptureParams {
    /// Camera device ID
    pub camera_id: String,
    /// Exposure time as a humantime string (e.g. `"500ms"`, `"30s"`, `"1m30s"`).
    #[serde(with = "humantime_serde")]
    #[schemars(with = "String")]
    pub duration: Duration,
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
        let (_cam_entry, cam) = resolve_device!(self, find_camera, &params.camera_id, "camera");

        let document_id = Uuid::new_v4().to_string();
        let image_path = format!(
            "{}/capture_{}.fits",
            self.session_config.data_directory, document_id
        );

        self.event_bus.emit(
            "exposure_started",
            serde_json::json!({
                "camera_id": params.camera_id,
                "duration": humantime::format_duration(params.duration).to_string(),
            }),
        );

        if let Err(e) = cam.start_exposure(params.duration, true).await {
            return Ok(tool_error!("failed to start exposure: {}", e));
        }

        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match cam.image_ready().await {
                Ok(true) => break,
                Ok(false) => continue,
                Err(e) => {
                    return Ok(tool_error!("error checking image ready: {}", e));
                }
            }
        }

        let image_array = match cam.image_array().await {
            Ok(arr) => arr,
            Err(e) => {
                return Ok(tool_error!("failed to download image array: {}", e));
            }
        };

        let (dim_x, dim_y, _planes) = image_array.dim();
        let width = dim_x as u32;
        let height = dim_y as u32;
        let pixels: Vec<i32> = image_array.iter().copied().collect();

        if let Err(e) = imaging::write_fits(&image_path, &pixels, width, height).await {
            return Ok(tool_error!("failed to write FITS file: {}", e));
        }

        self.event_bus.emit(
            "exposure_complete",
            serde_json::json!({
                "document_id": document_id,
                "file_path": image_path,
            }),
        );

        Ok(tool_success!({
            "image_path": image_path,
            "document_id": document_id,
        }))
    }

    #[tool(description = "Read camera capabilities: max_adu, exposure limits, sensor dimensions")]
    async fn get_camera_info(
        &self,
        Parameters(params): Parameters<CameraIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_cam_entry, cam) = resolve_device!(self, find_camera, &params.camera_id, "camera");

        let max_adu = match cam.max_adu().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read max_adu: {}", e)),
        };

        let (sensor_x, sensor_y) = match cam.camera_size().await {
            Ok(size) => (size[0], size[1]),
            Err(e) => return Ok(tool_error!("failed to read sensor size: {}", e)),
        };

        let (bin_x, bin_y) = match cam.bin().await {
            Ok(bin) => (bin[0] as u32, bin[1] as u32),
            Err(e) => {
                debug!(error = %e, "failed to read binning, using defaults");
                (1u32, 1u32)
            }
        };

        let (exposure_min, exposure_max) = match cam.exposure_range().await {
            Ok(range) => (*range.start(), *range.end()),
            Err(e) => {
                debug!(error = %e, "failed to read exposure range, using defaults");
                (Duration::from_millis(1), Duration::from_secs(3600))
            }
        };

        Ok(tool_success!({
            "camera_id": params.camera_id,
            "max_adu": max_adu,
            "sensor_x": sensor_x,
            "sensor_y": sensor_y,
            "bin_x": bin_x,
            "bin_y": bin_y,
            "exposure_min": humantime::format_duration(exposure_min).to_string(),
            "exposure_max": humantime::format_duration(exposure_max).to_string(),
        }))
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
            Ok(Err(e)) => return Ok(tool_error!("failed to compute stats: {}", e)),
            Err(e) => return Ok(tool_error!("task error: {}", e)),
        };

        debug!(
            image_path = %image_path,
            median = stats.median_adu,
            mean = %stats.mean_adu,
            "computed image stats"
        );

        Ok(tool_success!({
            "median_adu": stats.median_adu,
            "mean_adu": stats.mean_adu,
            "min_adu": stats.min_adu,
            "max_adu": stats.max_adu,
            "pixel_count": stats.pixel_count,
        }))
    }

    // -------------------------------------------------------------------
    // Filter wheel tools
    // -------------------------------------------------------------------

    #[tool(description = "Set the active filter on a filter wheel")]
    async fn set_filter(
        &self,
        Parameters(params): Parameters<SetFilterParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (fw_entry, fw) = resolve_device!(
            self,
            find_filter_wheel,
            &params.filter_wheel_id,
            "filter wheel"
        );

        let position = match fw_entry
            .config
            .filters
            .iter()
            .position(|f| f == &params.filter_name)
        {
            Some(p) => p,
            None => return Ok(tool_error!("filter not found: {}", params.filter_name)),
        };

        if let Err(e) = fw.set_position(position).await {
            return Ok(tool_error!("failed to set filter position: {}", e));
        }

        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match fw.position().await {
                Ok(Some(p)) if p == position => break,
                Ok(Some(_)) | Ok(None) => continue,
                Err(e) => {
                    return Ok(tool_error!("error waiting for filter wheel: {}", e));
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

        Ok(tool_success!({
            "filter_wheel_id": params.filter_wheel_id,
            "filter_name": params.filter_name,
            "position": position,
        }))
    }

    #[tool(description = "Get the current filter on a filter wheel")]
    async fn get_filter(
        &self,
        Parameters(params): Parameters<FilterWheelIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (fw_entry, fw) = resolve_device!(
            self,
            find_filter_wheel,
            &params.filter_wheel_id,
            "filter wheel"
        );

        let position = match fw.position().await {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(tool_error!("filter wheel is moving")),
            Err(e) => {
                return Ok(tool_error!("failed to get filter position: {}", e));
            }
        };

        let filter_name = fw_entry
            .config
            .filters
            .get(position)
            .cloned()
            .unwrap_or_else(|| format!("Filter {}", position));

        Ok(tool_success!({
            "filter_wheel_id": params.filter_wheel_id,
            "filter_name": filter_name,
            "position": position,
        }))
    }

    // -------------------------------------------------------------------
    // CoverCalibrator tools
    // -------------------------------------------------------------------

    #[tool(description = "Close the dust cover (blocks until closed)")]
    async fn close_cover(
        &self,
        Parameters(params): Parameters<CalibratorIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc_entry, cc) = resolve_device!(
            self,
            find_cover_calibrator,
            &params.calibrator_id,
            "calibrator"
        );
        let poll_interval = cc_entry.config.poll_interval;

        debug!(calibrator_id = %params.calibrator_id, "closing cover");
        if let Err(e) = cc.close_cover().await {
            return Ok(tool_error!("failed to close cover: {}", e));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.cover_state().await {
                Ok(CoverStatus::Closed) => {
                    debug!(calibrator_id = %params.calibrator_id, "cover closed");
                    return Ok(tool_success!({"status": "closed"}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error!("error polling cover state: {}", e));
                }
            }
        }

        Ok(tool_error!("timeout waiting for cover to close"))
    }

    #[tool(description = "Open the dust cover (blocks until open)")]
    async fn open_cover(
        &self,
        Parameters(params): Parameters<CalibratorIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc_entry, cc) = resolve_device!(
            self,
            find_cover_calibrator,
            &params.calibrator_id,
            "calibrator"
        );
        let poll_interval = cc_entry.config.poll_interval;

        debug!(calibrator_id = %params.calibrator_id, "opening cover");
        if let Err(e) = cc.open_cover().await {
            return Ok(tool_error!("failed to open cover: {}", e));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.cover_state().await {
                Ok(CoverStatus::Open) => {
                    debug!(calibrator_id = %params.calibrator_id, "cover opened");
                    return Ok(tool_success!({"status": "open"}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error!("error polling cover state: {}", e));
                }
            }
        }

        Ok(tool_error!("timeout waiting for cover to open"))
    }

    #[tool(description = "Turn on flat panel at brightness (default: max). Blocks until ready")]
    async fn calibrator_on(
        &self,
        Parameters(params): Parameters<CalibratorOnParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc_entry, cc) = resolve_device!(
            self,
            find_cover_calibrator,
            &params.calibrator_id,
            "calibrator"
        );
        let poll_interval = cc_entry.config.poll_interval;

        let brightness = if let Some(b) = params.brightness {
            b
        } else {
            match cc.max_brightness().await {
                Ok(max) => max,
                Err(e) => return Ok(tool_error!("failed to read max_brightness: {}", e)),
            }
        };

        debug!(calibrator_id = %params.calibrator_id, brightness = brightness, "turning calibrator on");
        if let Err(e) = cc.calibrator_on(brightness).await {
            return Ok(tool_error!("failed to turn calibrator on: {}", e));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.calibrator_state().await {
                Ok(CalibratorStatus::Ready) => {
                    debug!(calibrator_id = %params.calibrator_id, "calibrator ready");
                    return Ok(tool_success!({"status": "ready", "brightness": brightness}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error!("error polling calibrator state: {}", e));
                }
            }
        }

        Ok(tool_error!(
            "timeout waiting for calibrator to become ready"
        ))
    }

    #[tool(description = "Turn off flat panel. Blocks until off")]
    async fn calibrator_off(
        &self,
        Parameters(params): Parameters<CalibratorIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc_entry, cc) = resolve_device!(
            self,
            find_cover_calibrator,
            &params.calibrator_id,
            "calibrator"
        );
        let poll_interval = cc_entry.config.poll_interval;

        debug!(calibrator_id = %params.calibrator_id, "turning calibrator off");
        if let Err(e) = cc.calibrator_off().await {
            return Ok(tool_error!("failed to turn calibrator off: {}", e));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.calibrator_state().await {
                Ok(CalibratorStatus::Off) => {
                    debug!(calibrator_id = %params.calibrator_id, "calibrator off");
                    return Ok(tool_success!({"status": "off"}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error!("error polling calibrator state: {}", e));
                }
            }
        }

        Ok(tool_error!("timeout waiting for calibrator to turn off"))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use ascom_alpaca::ASCOMError;

    // -----------------------------------------------------------------------
    // Mock Device macro
    // -----------------------------------------------------------------------

    /// Generates Debug + Device impl with stubs for all required methods.
    macro_rules! impl_mock_device {
        ($name:ident) => {
            impl std::fmt::Debug for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, stringify!($name))
                }
            }

            #[async_trait::async_trait]
            impl ascom_alpaca::api::Device for $name {
                fn static_name(&self) -> &str {
                    "mock"
                }
                fn unique_id(&self) -> &str {
                    "mock-id"
                }
                async fn connected(&self) -> ascom_alpaca::ASCOMResult<bool> {
                    Ok(true)
                }
                async fn set_connected(&self, _: bool) -> ascom_alpaca::ASCOMResult<()> {
                    Ok(())
                }
                async fn description(&self) -> ascom_alpaca::ASCOMResult<String> {
                    Ok("mock".into())
                }
                async fn driver_info(&self) -> ascom_alpaca::ASCOMResult<String> {
                    Ok("mock".into())
                }
                async fn driver_version(&self) -> ascom_alpaca::ASCOMResult<String> {
                    Ok("0.0".into())
                }
            }
        };
    }

    // -----------------------------------------------------------------------
    // MockCamera — single configurable mock for all Camera error-injection
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockCamera {
        fail_start_exposure: bool,
        fail_image_ready: bool,
        fail_image_array: bool,
        fail_max_adu: bool,
        fail_camera_size: bool,
        fail_exposure_range: bool,
    }

    impl_mock_device!(MockCamera);

    #[async_trait::async_trait]
    impl ascom_alpaca::api::Camera for MockCamera {
        async fn start_exposure(
            &self,
            _duration: Duration,
            _light: bool,
        ) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_start_exposure {
                return Err(ASCOMError::invalid_operation("shutter jammed"));
            }
            Ok(())
        }

        async fn image_ready(&self) -> ascom_alpaca::ASCOMResult<bool> {
            if self.fail_image_ready {
                return Err(ASCOMError::invalid_operation("readout failed"));
            }
            Ok(true)
        }

        async fn image_array(
            &self,
        ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::camera::ImageArray> {
            if self.fail_image_array {
                return Err(ASCOMError::invalid_operation("download timeout"));
            }
            Ok(ndarray::Array3::<i32>::zeros((2, 2, 1)).into())
        }

        async fn max_adu(&self) -> ascom_alpaca::ASCOMResult<u32> {
            if self.fail_max_adu {
                return Err(ASCOMError::invalid_operation("not available"));
            }
            Ok(65535)
        }

        async fn camera_x_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
            if self.fail_camera_size {
                return Err(ASCOMError::invalid_operation("sensor error"));
            }
            Ok(1024)
        }

        async fn camera_y_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
            if self.fail_camera_size {
                return Err(ASCOMError::invalid_operation("sensor error"));
            }
            Ok(1024)
        }

        async fn exposure_max(&self) -> ascom_alpaca::ASCOMResult<Duration> {
            if self.fail_exposure_range {
                return Err(ASCOMError::invalid_operation("range unavailable"));
            }
            Ok(Duration::from_secs(3600))
        }

        async fn exposure_min(&self) -> ascom_alpaca::ASCOMResult<Duration> {
            if self.fail_exposure_range {
                return Err(ASCOMError::invalid_operation("range unavailable"));
            }
            Ok(Duration::from_millis(1))
        }

        async fn exposure_resolution(&self) -> ascom_alpaca::ASCOMResult<Duration> {
            Ok(Duration::from_millis(1))
        }

        async fn has_shutter(&self) -> ascom_alpaca::ASCOMResult<bool> {
            Ok(true)
        }

        async fn pixel_size_x(&self) -> ascom_alpaca::ASCOMResult<f64> {
            Ok(3.76)
        }

        async fn pixel_size_y(&self) -> ascom_alpaca::ASCOMResult<f64> {
            Ok(3.76)
        }

        async fn start_x(&self) -> ascom_alpaca::ASCOMResult<u32> {
            Ok(0)
        }

        async fn set_start_x(&self, _start_x: u32) -> ascom_alpaca::ASCOMResult<()> {
            Ok(())
        }

        async fn start_y(&self) -> ascom_alpaca::ASCOMResult<u32> {
            Ok(0)
        }

        async fn set_start_y(&self, _start_y: u32) -> ascom_alpaca::ASCOMResult<()> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // MockFilterWheel — single configurable mock for FilterWheel errors
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockFilterWheel {
        fail_set_position: bool,
        fail_position_poll: bool,
        report_moving: bool,
    }

    impl_mock_device!(MockFilterWheel);

    #[async_trait::async_trait]
    impl ascom_alpaca::api::FilterWheel for MockFilterWheel {
        async fn set_position(&self, _position: usize) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_set_position {
                return Err(ASCOMError::invalid_operation("wheel stuck"));
            }
            Ok(())
        }

        async fn position(&self) -> ascom_alpaca::ASCOMResult<Option<usize>> {
            if self.fail_position_poll {
                return Err(ASCOMError::invalid_operation("encoder error"));
            }
            if self.report_moving {
                return Ok(None);
            }
            Ok(Some(0))
        }

        async fn names(&self) -> ascom_alpaca::ASCOMResult<Vec<String>> {
            Ok(vec!["Lum".into(), "Red".into()])
        }

        async fn focus_offsets(&self) -> ascom_alpaca::ASCOMResult<Vec<i32>> {
            Ok(vec![0, 0])
        }
    }

    // -----------------------------------------------------------------------
    // MockCoverCalibrator — single configurable mock for CoverCalibrator
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockCoverCalibrator {
        fail_close_cover: bool,
        fail_open_cover: bool,
        fail_calibrator_on: bool,
        fail_calibrator_off: bool,
        fail_max_brightness: bool,
        fail_cover_state_poll: bool,
        stuck_cover_moving: bool,
        fail_calibrator_state_poll: bool,
        stuck_calibrator_not_ready: bool,
    }

    impl_mock_device!(MockCoverCalibrator);

    #[async_trait::async_trait]
    impl ascom_alpaca::api::CoverCalibrator for MockCoverCalibrator {
        async fn close_cover(&self) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_close_cover {
                return Err(ASCOMError::invalid_operation("motor fault"));
            }
            Ok(())
        }

        async fn open_cover(&self) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_open_cover {
                return Err(ASCOMError::invalid_operation("motor fault"));
            }
            Ok(())
        }

        async fn calibrator_on(&self, _brightness: u32) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_calibrator_on {
                return Err(ASCOMError::invalid_operation("lamp failure"));
            }
            Ok(())
        }

        async fn calibrator_off(&self) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_calibrator_off {
                return Err(ASCOMError::invalid_operation("stuck on"));
            }
            Ok(())
        }

        async fn cover_state(&self) -> ascom_alpaca::ASCOMResult<CoverStatus> {
            if self.fail_cover_state_poll {
                return Err(ASCOMError::invalid_operation("device unreachable"));
            }
            if self.stuck_cover_moving {
                return Ok(CoverStatus::Moving);
            }
            Ok(CoverStatus::Closed)
        }

        async fn calibrator_state(&self) -> ascom_alpaca::ASCOMResult<CalibratorStatus> {
            if self.fail_calibrator_state_poll {
                return Err(ASCOMError::invalid_operation("device unreachable"));
            }
            if self.stuck_calibrator_not_ready {
                return Ok(CalibratorStatus::NotReady);
            }
            Ok(CalibratorStatus::Off)
        }

        async fn max_brightness(&self) -> ascom_alpaca::ASCOMResult<u32> {
            if self.fail_max_brightness {
                return Err(ASCOMError::invalid_operation("not supported"));
            }
            Ok(255)
        }

        async fn brightness(&self) -> ascom_alpaca::ASCOMResult<u32> {
            Ok(0)
        }
    }

    // -----------------------------------------------------------------------
    // Helper functions
    // -----------------------------------------------------------------------

    fn test_handler(registry: crate::equipment::EquipmentRegistry) -> McpHandler {
        McpHandler::new(
            Arc::new(registry),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: std::env::temp_dir()
                    .join("rp-unit-test")
                    .to_string_lossy()
                    .to_string(),
            },
        )
    }

    fn assert_tool_error(result: Result<CallToolResult, rmcp::ErrorData>, expected_substr: &str) {
        let call_result = result.expect("tool returned protocol error");
        assert!(
            call_result.is_error.unwrap_or(false),
            "expected is_error=true"
        );
        let text = call_result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.as_str())
            .unwrap_or("");
        assert!(
            text.contains(expected_substr),
            "expected error containing '{}', got: '{}'",
            expected_substr,
            text
        );
    }

    // -----------------------------------------------------------------------
    // Registry builders
    // -----------------------------------------------------------------------

    fn camera_registry(
        cam: Arc<dyn ascom_alpaca::api::Camera>,
    ) -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![crate::equipment::CameraEntry {
                id: "cam".to_string(),
                connected: true,
                config: crate::config::CameraConfig {
                    id: "cam".to_string(),
                    name: "mock".to_string(),
                    alpaca_url: "http://localhost:1".to_string(),
                    device_type: String::new(),
                    device_number: 0,
                    cooler_target_c: None,
                    gain: None,
                    offset: None,
                    auth: None,
                },
                device: Some(cam),
            }],
            filter_wheels: vec![],
            cover_calibrators: vec![],
        }
    }

    fn filter_wheel_registry(
        fw: Arc<dyn ascom_alpaca::api::FilterWheel>,
    ) -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![crate::equipment::FilterWheelEntry {
                id: "fw".to_string(),
                connected: true,
                config: crate::config::FilterWheelConfig {
                    id: "fw".to_string(),
                    camera_id: String::new(),
                    alpaca_url: "http://localhost:1".to_string(),
                    device_number: 0,
                    filters: vec!["Lum".to_string(), "Red".to_string()],
                    auth: None,
                },
                device: Some(fw),
            }],
            cover_calibrators: vec![],
        }
    }

    fn calibrator_registry(
        cc: Arc<dyn ascom_alpaca::api::CoverCalibrator>,
    ) -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![crate::equipment::CoverCalibratorEntry {
                id: "cc".to_string(),
                connected: true,
                config: crate::config::CoverCalibratorConfig {
                    id: "cc".to_string(),
                    alpaca_url: "http://localhost:1".to_string(),
                    device_number: 0,
                    poll_interval: Duration::from_secs(1),
                    auth: None,
                },
                device: Some(cc),
            }],
        }
    }

    // -----------------------------------------------------------------------
    // Capture tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_capture_start_exposure_fails() {
        let cam = MockCamera {
            fail_start_exposure: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await;
        assert_tool_error(result, "failed to start exposure");
    }

    #[tokio::test]
    async fn test_capture_image_ready_error() {
        let cam = MockCamera {
            fail_image_ready: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await;
        assert_tool_error(result, "error checking image ready");
    }

    #[tokio::test]
    async fn test_capture_image_array_fails() {
        let cam = MockCamera {
            fail_image_array: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await;
        assert_tool_error(result, "failed to download image array");
    }

    // -----------------------------------------------------------------------
    // get_camera_info tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_camera_info_max_adu_fails() {
        let cam = MockCamera {
            fail_max_adu: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .get_camera_info(Parameters(CameraIdParams {
                camera_id: "cam".into(),
            }))
            .await;
        assert_tool_error(result, "failed to read max_adu");
    }

    #[tokio::test]
    async fn test_get_camera_info_sensor_size_fails() {
        let cam = MockCamera {
            fail_camera_size: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .get_camera_info(Parameters(CameraIdParams {
                camera_id: "cam".into(),
            }))
            .await;
        assert_tool_error(result, "failed to read sensor size");
    }

    // -----------------------------------------------------------------------
    // set_filter tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_set_filter_set_position_fails() {
        let fw = MockFilterWheel {
            fail_set_position: true,
            ..Default::default()
        };
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .set_filter(Parameters(SetFilterParams {
                filter_wheel_id: "fw".into(),
                filter_name: "Lum".into(),
            }))
            .await;
        assert_tool_error(result, "failed to set filter position");
    }

    // -----------------------------------------------------------------------
    // CoverCalibrator tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_close_cover_command_fails() {
        let cc = MockCoverCalibrator {
            fail_close_cover: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .close_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "failed to close cover");
    }

    #[tokio::test]
    async fn test_close_cover_polling_error() {
        let cc = MockCoverCalibrator {
            fail_cover_state_poll: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .close_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "error polling cover state");
    }

    #[tokio::test]
    async fn test_open_cover_command_fails() {
        let cc = MockCoverCalibrator {
            fail_open_cover: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .open_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "failed to open cover");
    }

    #[tokio::test]
    async fn test_calibrator_on_max_brightness_fails() {
        let cc = MockCoverCalibrator {
            fail_max_brightness: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_on(Parameters(CalibratorOnParams {
                calibrator_id: "cc".into(),
                brightness: None,
            }))
            .await;
        assert_tool_error(result, "failed to read max_brightness");
    }

    #[tokio::test]
    async fn test_calibrator_on_command_fails() {
        let cc = MockCoverCalibrator {
            fail_calibrator_on: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_on(Parameters(CalibratorOnParams {
                calibrator_id: "cc".into(),
                brightness: None,
            }))
            .await;
        assert_tool_error(result, "failed to turn calibrator on");
    }

    #[tokio::test]
    async fn test_calibrator_off_command_fails() {
        let cc = MockCoverCalibrator {
            fail_calibrator_off: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_off(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "failed to turn calibrator off");
    }

    // -----------------------------------------------------------------------
    // capture — write_fits failure
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_capture_write_fits_fails() {
        let cam = MockCamera::default(); // succeeds through image_array
        let registry = camera_registry(Arc::new(cam));
        // Use an existing file as the "directory" so write_fits fails cross-platform.
        // The capture tool appends /capture_{uuid}.fits — creating a file inside
        // another file fails on all OSes.
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let handler = McpHandler::new(
            Arc::new(registry),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: blocker.path().to_string_lossy().to_string(),
            },
        );
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await;
        assert_tool_error(result, "failed to write FITS file");
    }

    // -----------------------------------------------------------------------
    // get_camera_info — exposure_range fallback
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_camera_info_exposure_range_fallback() {
        let cam = MockCamera {
            fail_exposure_range: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .get_camera_info(Parameters(CameraIdParams {
                camera_id: "cam".into(),
            }))
            .await;
        // This is a soft failure — it falls back to defaults, so the call succeeds
        let call_result = result.unwrap();
        assert!(!call_result.is_error.unwrap_or(false));
        let text = call_result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.as_str())
            .unwrap_or("");
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(json["exposure_min"], "1ms");
        assert_eq!(json["exposure_max"], "1h");
    }

    // -----------------------------------------------------------------------
    // set_filter — polling error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_set_filter_polling_error() {
        let fw = MockFilterWheel {
            fail_position_poll: true,
            ..Default::default()
        };
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .set_filter(Parameters(SetFilterParams {
                filter_wheel_id: "fw".into(),
                filter_name: "Lum".into(),
            }))
            .await;
        assert_tool_error(result, "error waiting for filter wheel");
    }

    // -----------------------------------------------------------------------
    // get_filter — errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_filter_position_error() {
        let fw = MockFilterWheel {
            fail_position_poll: true,
            ..Default::default()
        };
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .get_filter(Parameters(FilterWheelIdParams {
                filter_wheel_id: "fw".into(),
            }))
            .await;
        assert_tool_error(result, "failed to get filter position");
    }

    #[tokio::test]
    async fn test_get_filter_wheel_moving() {
        let fw = MockFilterWheel {
            report_moving: true,
            ..Default::default()
        };
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .get_filter(Parameters(FilterWheelIdParams {
                filter_wheel_id: "fw".into(),
            }))
            .await;
        assert_tool_error(result, "filter wheel is moving");
    }

    // -----------------------------------------------------------------------
    // Timeout tests (use tokio::time::pause to fast-forward)
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_close_cover_timeout() {
        let cc = MockCoverCalibrator {
            stuck_cover_moving: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .close_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "timeout waiting for cover to close");
    }

    #[tokio::test(start_paused = true)]
    async fn test_open_cover_timeout() {
        let cc = MockCoverCalibrator {
            stuck_cover_moving: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .open_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "timeout waiting for cover to open");
    }

    #[tokio::test]
    async fn test_open_cover_polling_error() {
        let cc = MockCoverCalibrator {
            fail_cover_state_poll: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .open_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "error polling cover state");
    }

    #[tokio::test(start_paused = true)]
    async fn test_calibrator_on_timeout() {
        let cc = MockCoverCalibrator {
            stuck_calibrator_not_ready: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_on(Parameters(CalibratorOnParams {
                calibrator_id: "cc".into(),
                brightness: Some(100),
            }))
            .await;
        assert_tool_error(result, "timeout waiting for calibrator to become ready");
    }

    #[tokio::test]
    async fn test_calibrator_on_polling_error() {
        let cc = MockCoverCalibrator {
            fail_calibrator_state_poll: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_on(Parameters(CalibratorOnParams {
                calibrator_id: "cc".into(),
                brightness: Some(100),
            }))
            .await;
        assert_tool_error(result, "error polling calibrator state");
    }

    #[tokio::test(start_paused = true)]
    async fn test_calibrator_off_timeout() {
        let cc = MockCoverCalibrator {
            stuck_calibrator_not_ready: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_off(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "timeout waiting for calibrator to turn off");
    }

    #[tokio::test]
    async fn test_calibrator_off_polling_error() {
        let cc = MockCoverCalibrator {
            fail_calibrator_state_poll: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_off(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "error polling calibrator state");
    }

    // -----------------------------------------------------------------------
    // compute_image_stats error paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_compute_image_stats_bad_fits() {
        // Write a non-FITS file so read_fits_pixels fails inside spawn_blocking
        let dir = tempfile::tempdir().unwrap();
        let bad_file = dir.path().join("bad.fits");
        std::fs::write(&bad_file, b"not a fits file").unwrap();

        let handler = test_handler(crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
        });
        let result = handler
            .compute_image_stats(Parameters(ComputeImageStatsParams {
                image_path: bad_file.to_string_lossy().to_string(),
                document_id: None,
            }))
            .await;
        assert_tool_error(result, "failed to compute stats");
    }

    // -----------------------------------------------------------------------
    // set_filter — filter not found
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_set_filter_filter_not_found() {
        let fw = MockFilterWheel::default();
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .set_filter(Parameters(SetFilterParams {
                filter_wheel_id: "fw".into(),
                filter_name: "Ultraviolet".into(), // not in mock's filter list
            }))
            .await;
        assert_tool_error(result, "filter not found");
    }

    // -----------------------------------------------------------------------
    // get_filter — success path (covers lines 387-391)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_filter_success() {
        let fw = MockFilterWheel::default(); // position() returns Some(0)
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .get_filter(Parameters(FilterWheelIdParams {
                filter_wheel_id: "fw".into(),
            }))
            .await;
        let call_result = result.unwrap();
        assert!(!call_result.is_error.unwrap_or(false));
        let text = call_result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.as_str())
            .unwrap_or("");
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(json["filter_name"], "Lum");
        assert_eq!(json["position"], 0);
    }

    // -----------------------------------------------------------------------
    // CoverCalibrator success paths (covers resolve_device! macro lines)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_close_cover_success() {
        let cc = MockCoverCalibrator::default(); // cover_state returns Closed
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .close_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        let call_result = result.unwrap();
        assert!(!call_result.is_error.unwrap_or(false));
    }
}
