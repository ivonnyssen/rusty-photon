use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use tracing::debug;

use super::super::handler::McpHandler;
use super::super::internals::ResolvedParams;
use super::super::progress::{ProgressEmitter, ProgressSink};
use super::super::{resolve_device, tool_error, tool_success};
use crate::imaging;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AutoFocusToolParams {
    /// Camera that captures each sweep frame.
    #[serde(default)]
    pub camera_id: Option<String>,
    /// Focuser to sweep.
    #[serde(default)]
    pub focuser_id: Option<String>,
    /// Per-frame exposure (humantime string).
    #[serde(default, with = "humantime_serde::option")]
    #[schemars(with = "Option<String>")]
    pub duration: Option<Duration>,
    /// Focuser steps between sweep grid points (positive integer).
    #[serde(default)]
    pub step_size: Option<i32>,
    /// Half-width of the sweep around the current focuser position
    /// (positive integer).
    #[serde(default)]
    pub half_width: Option<i32>,
    /// Minimum component pixel area for `measure_basic` (no default —
    /// rig-dependent).
    #[serde(default)]
    pub min_area: Option<usize>,
    /// Maximum component pixel area for `measure_basic` (no default —
    /// rig-dependent; donut PSFs at extreme defocus can span many
    /// hundreds of pixels).
    #[serde(default)]
    pub max_area: Option<usize>,
    /// Per-frame `measure_basic` threshold (sigma units). Default 5.0.
    #[serde(default = "default_threshold_sigma")]
    pub threshold_sigma: f64,
    /// Minimum number of non-null HFR samples for the parabolic fit.
    /// Default 5.
    #[serde(default)]
    pub min_fit_points: Option<usize>,
}

fn default_threshold_sigma() -> f64 {
    5.0
}

#[tool_router(router = tool_router_auto_focus, vis = "pub")]
impl McpHandler {
    #[tool(
        description = "V-curve auto-focus: sweep ± half_width around the focuser's current position, capture and run measure_basic at each step, fit a parabola in HFR, and move the focuser to the fitted minimum"
    )]
    pub(crate) async fn auto_focus(
        &self,
        Parameters(params): Parameters<AutoFocusToolParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let progress_sink = ProgressSink::from_request_context(&ctx);
        self.auto_focus_inner(params, progress_sink).await
    }

    /// Body of the `auto_focus` MCP tool, split out so unit tests can
    /// pass `None` for the progress sink without constructing a real
    /// rmcp `Peer` (its constructor is `pub(crate)` in rmcp 1.7).
    pub(crate) async fn auto_focus_inner(
        &self,
        params: AutoFocusToolParams,
        progress_sink: Option<ProgressSink>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // Field-presence validation runs in input order so the error
        // message always points at the first missing field — same
        // pattern as `measure_basic`.
        let camera_id = match params.camera_id.as_deref() {
            Some(s) => s.to_string(),
            None => return Ok(tool_error!("missing required parameter: camera_id")),
        };
        let focuser_id = match params.focuser_id.as_deref() {
            Some(s) => s.to_string(),
            None => return Ok(tool_error!("missing required parameter: focuser_id")),
        };
        let duration = match params.duration {
            Some(d) => d,
            None => return Ok(tool_error!("missing required parameter: duration")),
        };
        let step_size = match params.step_size {
            Some(s) => s,
            None => return Ok(tool_error!("missing required parameter: step_size")),
        };
        let half_width = match params.half_width {
            Some(s) => s,
            None => return Ok(tool_error!("missing required parameter: half_width")),
        };
        let min_area = match params.min_area {
            Some(s) => s,
            None => return Ok(tool_error!("missing required parameter: min_area")),
        };
        let max_area = match params.max_area {
            Some(s) => s,
            None => return Ok(tool_error!("missing required parameter: max_area")),
        };

        // Resolve devices early — emits the standard
        // "<kind> not found" / "<kind> not connected" errors expected
        // by the BDD device-resolution scenarios. Camera order before
        // focuser matches input order in the contract.
        let (_cam_entry, cam) = resolve_device!(self, find_camera, &camera_id, "camera");
        let _ = cam; // resolved purely for the connection check; do_capture re-resolves.
        let (foc_entry, foc) = resolve_device!(self, find_focuser, &focuser_id, "focuser");

        // Read the current focuser position + temperature exactly once
        // each (per the Contract algorithm step 1) and thread the values
        // through to both `focus_started` *and* `run_auto_focus` so the
        // event payload and the result's `temperature_c`/sweep-grid
        // origin can never disagree. Temperature is informational only:
        // any read failure (NOT_IMPLEMENTED or transient) becomes
        // `temperature_c: null`; we don't abort an auto-focus run over
        // a missing thermistor.
        let starting_position = match foc.position().await {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("failed to read focuser position: {}", e)),
        };
        let starting_temperature_c: Option<f64> = foc.temperature().await.ok();
        self.event_bus.emit(
            "focus_started",
            serde_json::json!({
                "camera_id": camera_id,
                "focuser_id": focuser_id,
                "position": starting_position,
                "temperature": starting_temperature_c,
            }),
        );

        let bounds = (foc_entry.config.min_position, foc_entry.config.max_position);
        let af_params = imaging::tools::auto_focus::AutoFocusParams {
            duration,
            step_size,
            half_width,
            min_area,
            max_area,
            threshold_sigma: params.threshold_sigma,
            min_fit_points: params.min_fit_points.unwrap_or(5),
        };

        // Store the per-request sink on the adapter so every
        // inner `do_capture` / `do_move_focuser_blocking` call emits
        // progress through the same `progressToken`. See
        // `mcp::progress` for the rmcp 300 s session keep-alive race
        // this guards against.
        let adapter = AutoFocusAdapter {
            handler: self,
            camera_id: camera_id.clone(),
            focuser_id: focuser_id.clone(),
            progress: progress_sink,
        };

        match imaging::tools::auto_focus::run_auto_focus(
            &adapter,
            &adapter,
            &adapter,
            bounds,
            starting_position,
            starting_temperature_c,
            af_params,
        )
        .await
        {
            Ok(result) => {
                self.event_bus.emit(
                    "focus_complete",
                    serde_json::json!({
                        "camera_id": camera_id,
                        "focuser_id": focuser_id,
                        "position": result.best_position,
                        "hfr": result.best_hfr,
                        "samples_used": result.samples_used,
                    }),
                );
                let curve_points =
                    serde_json::to_value(&result.curve_points).unwrap_or(serde_json::Value::Null);
                Ok(tool_success!({
                    "best_position": result.best_position,
                    "best_hfr": result.best_hfr,
                    "final_position": result.final_position,
                    "samples_used": result.samples_used,
                    "curve_points": curve_points,
                    "temperature_c": result.temperature_c,
                }))
            }
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }
}

/// Adapter that satisfies all three [`auto_focus`] traits
/// (`FocuserOps`, `CaptureOps`, `MeasureOps`) by delegating to the
/// existing [`McpHandler`] helpers (`do_move_focuser_blocking`,
/// `do_capture`, `measure_via_document` + cache `put_section`).
///
/// Keeps the compound tool's wiring close to the corresponding
/// primitive tools: same bounds-check / poll semantics on focuser
/// motion, same FITS write / cache insert / event emission on
/// capture, same `image_analysis` section persistence on measure.
///
/// `progress` carries the per-request `ProgressSink` (or `None` when
/// the client did not supply a `progressToken`) so each inner
/// blocking helper emits `notifications/progress` against the same
/// token used by the compound tool's own call.
pub(crate) struct AutoFocusAdapter<'a> {
    pub(crate) handler: &'a McpHandler,
    pub(crate) camera_id: String,
    pub(crate) focuser_id: String,
    pub(crate) progress: Option<ProgressSink>,
}

impl AutoFocusAdapter<'_> {
    fn emitter(&self) -> Option<&dyn ProgressEmitter> {
        self.progress.as_ref().map(|s| s as &dyn ProgressEmitter)
    }
}

#[async_trait::async_trait]
impl imaging::tools::auto_focus::FocuserOps for AutoFocusAdapter<'_> {
    async fn move_to(&self, position: i32) -> std::result::Result<i32, String> {
        self.handler
            .do_move_focuser_blocking(&self.focuser_id, position, self.emitter())
            .await
    }
}

#[async_trait::async_trait]
impl imaging::tools::auto_focus::CaptureOps for AutoFocusAdapter<'_> {
    async fn capture(&self, duration: Duration) -> std::result::Result<String, String> {
        let (_image_path, document_id) = self
            .handler
            .do_capture(&self.camera_id, duration, self.emitter())
            .await?;
        Ok(document_id)
    }
}

#[async_trait::async_trait]
impl imaging::tools::auto_focus::MeasureOps for AutoFocusAdapter<'_> {
    async fn measure(
        &self,
        document_id: &str,
        min_area: usize,
        max_area: usize,
        threshold_sigma: f64,
    ) -> std::result::Result<imaging::tools::auto_focus::HfrSample, String> {
        let resolved = ResolvedParams {
            threshold_sigma,
            min_area,
            max_area,
        };
        let result = self
            .handler
            .measure_via_document(document_id, &resolved)
            .await
            .map_err(|e| e.to_string())?;
        // Persist the per-frame `image_analysis` section, matching the
        // standalone `measure_basic` tool's side effect — auto_focus is
        // explicitly composed of measure_basic calls per the contract.
        let value = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
        if let Err(e) = self
            .handler
            .image_cache
            .put_section(document_id, "image_analysis", value)
            .await
        {
            debug!(error = %e, document_id, "failed to persist image_analysis section");
        }
        Ok(imaging::tools::auto_focus::HfrSample {
            hfr: result.hfr,
            star_count: result.star_count,
        })
    }
}
