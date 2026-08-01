//! MCP client for calling rp's built-in tools, built on the standard
//! `rp-mcp-client` crate (ADR-017): CA-pinned TLS and the observatory
//! credential over verified HTTPS only.
//!
//! Unit traps this wrapper normalizes for its callers: rp's `slew`
//! takes RA in decimal hours while `plate_solve` returns and hints in
//! decimal degrees. Everything crossing this module's public surface
//! is degrees; the hours conversion happens here.

use std::path::Path;
use std::time::Duration;

use rp_mcp_client::{ClientAuthConfig, RpMcpClient};
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use crate::error::{PolarAlignError, Result};
use crate::math::WcsMatrix;

/// MCP client for one `rp` session.
pub struct McpClient {
    inner: RpMcpClient,
}

/// Result from the `capture` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct CaptureResult {
    pub image_path: String,
    pub document_id: String,
}

/// Result from the `get_camera_info` tool — the sensor bounds the
/// adjustment overlay's `in_frame` flag is computed against.
#[derive(Debug, Clone, Deserialize)]
pub struct CameraInfo {
    pub sensor_x: u32,
    pub sensor_y: u32,
}

/// The `wcs_matrix` block of a solve result (plate-solver contract:
/// all-or-nothing, absent when the sidecar lacked a complete
/// CRPIX + CD set).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct SolvedWcsMatrix {
    pub crpix1: f64,
    pub crpix2: f64,
    pub cd1_1: f64,
    pub cd1_2: f64,
    pub cd2_1: f64,
    pub cd2_2: f64,
}

impl From<SolvedWcsMatrix> for WcsMatrix {
    fn from(m: SolvedWcsMatrix) -> Self {
        Self {
            crpix1: m.crpix1,
            crpix2: m.crpix2,
            cd1_1: m.cd1_1,
            cd1_2: m.cd1_2,
            cd2_1: m.cd2_1,
            cd2_2: m.cd2_2,
        }
    }
}

/// Result from the `plate_solve` tool. RA/dec in decimal degrees.
#[derive(Debug, Clone, Deserialize)]
pub struct SolveResult {
    pub ra_center: f64,
    pub dec_center: f64,
    pub pixel_scale_arcsec: f64,
    pub rotation_deg: f64,
    #[serde(default)]
    pub wcs_matrix: Option<SolvedWcsMatrix>,
}

/// Result from the `get_tracking` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct TrackingState {
    pub tracking: bool,
    pub can_set_tracking: bool,
}

/// Result from the `get_park_state` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct ParkState {
    pub at_park: bool,
    pub can_unpark: bool,
}

/// One star from the `detect_stars` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct DetectedStar {
    pub x: f64,
    pub y: f64,
    pub flux: f64,
    pub saturated_pixel_count: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetectedStars {
    pub stars: Vec<DetectedStar>,
}

impl McpClient {
    /// Connect to an MCP server at the given URL, presenting
    /// `service_auth` per the ADR-017 credential policy.
    pub async fn new(
        mcp_url: &str,
        service_auth: Option<&ClientAuthConfig>,
        ca_cert: Option<&Path>,
    ) -> Result<Self> {
        debug!(url = %mcp_url, "connecting MCP client");
        let inner = RpMcpClient::connect(mcp_url, service_auth, ca_cert)
            .await
            .map_err(|e| PolarAlignError::ToolCall(format!("MCP connect: {}", e)))?;
        Ok(Self { inner })
    }

    pub async fn capture(&self, camera_id: &str, duration: Duration) -> Result<CaptureResult> {
        self.call_tool(
            "capture",
            serde_json::json!({
                "camera_id": camera_id,
                "duration": humantime::format_duration(duration).to_string(),
            }),
        )
        .await
    }

    pub async fn get_camera_info(&self, camera_id: &str) -> Result<CameraInfo> {
        self.call_tool(
            "get_camera_info",
            serde_json::json!({"camera_id": camera_id}),
        )
        .await
    }

    /// Solve a captured image, hinted with the expected pointing in
    /// decimal degrees.
    pub async fn plate_solve(
        &self,
        image_path: &str,
        document_id: &str,
        hint_ra_deg: f64,
        hint_dec_deg: f64,
        search_radius_deg: f64,
        timeout: Duration,
    ) -> Result<SolveResult> {
        self.call_tool(
            "plate_solve",
            serde_json::json!({
                "document_id": document_id,
                "image_path": image_path,
                "pointing_hint": { "ra_deg": hint_ra_deg, "dec_deg": hint_dec_deg },
                "search_radius_deg": search_radius_deg,
                "timeout": humantime::format_duration(timeout).to_string(),
            }),
        )
        .await
    }

    /// Slew to RA/dec in decimal degrees (converted here to the
    /// tool's decimal-hours RA contract), settling afterwards.
    pub async fn slew(&self, ra_deg: f64, dec_deg: f64, settle: Duration) -> Result<()> {
        let _: Value = self
            .call_tool(
                "slew",
                serde_json::json!({
                    "ra": ra_deg.rem_euclid(360.0) / 15.0,
                    "dec": dec_deg,
                    "settle_after": humantime::format_duration(settle).to_string(),
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn abort_slew(&self) -> Result<()> {
        let _: Value = self.call_tool("abort_slew", serde_json::json!({})).await?;
        Ok(())
    }

    pub async fn get_tracking(&self) -> Result<TrackingState> {
        self.call_tool("get_tracking", serde_json::json!({})).await
    }

    pub async fn set_tracking(&self, enabled: bool) -> Result<()> {
        let _: Value = self
            .call_tool("set_tracking", serde_json::json!({"enabled": enabled}))
            .await?;
        Ok(())
    }

    pub async fn get_park_state(&self) -> Result<ParkState> {
        self.call_tool("get_park_state", serde_json::json!({}))
            .await
    }

    pub async fn unpark(&self) -> Result<()> {
        let _: Value = self.call_tool("unpark", serde_json::json!({})).await?;
        Ok(())
    }

    /// Detect stars on a captured frame. Area bounds are fixed at
    /// values that admit ordinary stellar PSFs and reject hot pixels
    /// and extended blobs.
    pub async fn detect_stars(&self, image_path: &str, document_id: &str) -> Result<DetectedStars> {
        self.call_tool(
            "detect_stars",
            serde_json::json!({
                "document_id": document_id,
                "image_path": image_path,
                "threshold_sigma": 5.0,
                "min_area": 4,
                "max_area": 1000,
            }),
        )
        .await
    }

    /// Generic helper: call tool, check for errors, deserialize result.
    async fn call_tool<T: serde::de::DeserializeOwned>(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<T> {
        debug!(tool = %tool_name, "calling MCP tool");

        let args = arguments.as_object().cloned().unwrap_or_default();
        let value = self
            .inner
            .call_tool(tool_name, args)
            .await
            .map_err(|e| PolarAlignError::ToolCall(format!("{}: {}", tool_name, e)))?;

        serde_json::from_value(value).map_err(|e| {
            PolarAlignError::ToolCall(format!("{}: failed to parse result: {}", tool_name, e))
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn test_solve_result_parses_with_wcs_matrix() {
        let json = r#"{
            "ra_center": 52.1, "dec_center": 85.2,
            "pixel_scale_arcsec": 1.05, "rotation_deg": 12.0,
            "solver": "astap",
            "wcs_matrix": {
                "crpix1": 512.0, "crpix2": 384.0,
                "cd1_1": -2.9e-4, "cd1_2": 0.0, "cd2_1": 0.0, "cd2_2": 2.9e-4
            }
        }"#;
        let result: SolveResult = serde_json::from_str(json).unwrap();
        let matrix = result.wcs_matrix.unwrap();
        assert_eq!(matrix.crpix1, 512.0);
        assert_eq!(matrix.cd1_1, -2.9e-4);
    }

    #[test]
    fn test_solve_result_parses_without_wcs_matrix() {
        let json = r#"{
            "ra_center": 52.1, "dec_center": 85.2,
            "pixel_scale_arcsec": 1.05, "rotation_deg": 12.0
        }"#;
        let result: SolveResult = serde_json::from_str(json).unwrap();
        assert!(result.wcs_matrix.is_none());
    }

    #[test]
    fn test_solve_result_parses_null_wcs_matrix() {
        let json = r#"{
            "ra_center": 52.1, "dec_center": 85.2,
            "pixel_scale_arcsec": 1.05, "rotation_deg": 12.0,
            "wcs_matrix": null
        }"#;
        let result: SolveResult = serde_json::from_str(json).unwrap();
        assert!(result.wcs_matrix.is_none());
    }

    #[test]
    fn test_detected_stars_parse_the_rp_payload_shape() {
        let json = r#"{
            "stars": [
                {"x": 10.5, "y": 20.5, "flux": 5000.0, "peak": 900,
                 "saturated_pixel_count": 0}
            ],
            "star_count": 1,
            "saturated_star_count": 0,
            "background_mean": 100.0,
            "background_stddev": 5.0
        }"#;
        let stars: DetectedStars = serde_json::from_str(json).unwrap();
        assert_eq!(stars.stars.len(), 1);
        assert_eq!(stars.stars[0].flux, 5000.0);
    }
}
