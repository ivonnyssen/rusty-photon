use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tracing::debug;

use super::super::handler::McpHandler;
use super::super::{resolve_device, tool_error, tool_success};

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
    pub camera_id: String,
}

#[tool_router(router = tool_router_camera, vis = "pub")]
impl McpHandler {
    #[tool(description = "Capture an image, download image_array, save FITS file")]
    pub(crate) async fn capture(
        &self,
        Parameters(params): Parameters<CaptureParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self.do_capture(&params.camera_id, params.duration).await {
            Ok((image_path, document_id)) => Ok(tool_success!({
                "image_path": image_path,
                "document_id": document_id,
            })),
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }

    #[tool(description = "Read camera capabilities: max_adu, exposure limits, sensor dimensions")]
    pub(crate) async fn get_camera_info(
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
}
