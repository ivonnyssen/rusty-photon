use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::handler::McpHandler;
use super::super::{resolve_device, tool_error, tool_success};
use crate::imaging;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CenterOnTargetToolParams {
    /// Camera that captures each iteration's frame.
    #[serde(default)]
    pub camera_id: Option<String>,
    /// Target right ascension, decimal hours, [0, 24).
    #[serde(default)]
    pub ra: Option<f64>,
    /// Target declination, decimal degrees, [-90, 90].
    #[serde(default)]
    pub dec: Option<f64>,
    /// Per-iteration exposure (humantime string).
    #[serde(default, with = "humantime_serde::option")]
    #[schemars(with = "Option<String>")]
    pub duration: Option<Duration>,
    /// Convergence threshold on the great-circle residual between the
    /// solved center and (ra, dec), in arcseconds.
    #[serde(default)]
    pub tolerance_arcsec: Option<f64>,
    /// Hard cap on the number of iterations. Capped at MAX_ATTEMPTS
    /// (50) before any motion.
    #[serde(default)]
    pub max_attempts: Option<usize>,
}

#[tool_router(router = tool_router_center_on_target, vis = "pub")]
impl McpHandler {
    #[tool(
        description = "Iteratively capture, plate-solve, sync (iter 1 only), and slew until the great-circle residual between the solved field-center and (ra, dec) is at or below tolerance_arcsec. Singular mount required. See `center_on_target` Contract in rp.md."
    )]
    pub(crate) async fn center_on_target(
        &self,
        Parameters(params): Parameters<CenterOnTargetToolParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // Body validation in input order so the missing-parameter
        // outline always points at the first missing field — same
        // convention as `auto_focus` / `measure_basic`.
        let camera_id = match params.camera_id.as_deref() {
            Some(s) => s.to_string(),
            None => return Ok(tool_error!("missing required parameter: camera_id")),
        };
        let ra = match params.ra {
            Some(v) => v,
            None => return Ok(tool_error!("missing required parameter: ra")),
        };
        let dec = match params.dec {
            Some(v) => v,
            None => return Ok(tool_error!("missing required parameter: dec")),
        };
        let duration = match params.duration {
            Some(d) => d,
            None => return Ok(tool_error!("missing required parameter: duration")),
        };
        let tolerance_arcsec = match params.tolerance_arcsec {
            Some(v) => v,
            None => return Ok(tool_error!("missing required parameter: tolerance_arcsec")),
        };
        let max_attempts = match params.max_attempts {
            Some(v) => v,
            None => return Ok(tool_error!("missing required parameter: max_attempts")),
        };

        // Resolve devices early so the device-resolution error
        // scenarios trip before any numeric-range or motion errors.
        let (_cam_entry, _cam) = resolve_device!(self, find_camera, &camera_id, "camera");
        // Mount resolution: same shape as `do_sync_mount` /
        // `do_slew_blocking` would surface, just hoisted here so the
        // BDD "no mount configured" / "mount not connected" scenarios
        // see the error before the loop body runs.
        if let Err(e) = self.resolve_mount() {
            return Ok(tool_error!("{}", e));
        }

        self.event_bus.emit(
            "centering_started",
            serde_json::json!({
                "camera_id": camera_id,
                "ra": ra,
                "dec": dec,
                "tolerance_arcsec": tolerance_arcsec,
                "max_attempts": max_attempts,
            }),
        );

        let cot_params = imaging::tools::center_on_target::CenterOnTargetParams {
            ra,
            dec,
            duration,
            tolerance_arcsec,
            max_attempts,
        };

        let adapter = CenterOnTargetAdapter {
            handler: self,
            camera_id: camera_id.clone(),
        };

        let event_bus = self.event_bus.clone();
        let camera_id_for_event = camera_id.clone();
        let emit_iteration = move |rec: &imaging::tools::center_on_target::IterationRecord| {
            let action = serde_json::to_value(rec.action).unwrap_or(serde_json::Value::Null);
            event_bus.emit(
                "centering_iteration",
                serde_json::json!({
                    "camera_id": camera_id_for_event,
                    "document_id": rec.document_id,
                    "residual_arcsec": rec.residual_arcsec,
                    "solved_ra": rec.solved_ra,
                    "solved_dec": rec.solved_dec,
                    "action": action,
                }),
            );
        };

        match imaging::tools::center_on_target::run_center_on_target(
            &adapter,
            &adapter,
            &adapter,
            cot_params,
            emit_iteration,
        )
        .await
        {
            Ok(result) => {
                self.event_bus.emit(
                    "centering_complete",
                    serde_json::json!({
                        "camera_id": camera_id,
                        "final_error_arcsec": result.final_error_arcsec,
                        "attempts": result.attempts,
                        "final_ra": result.final_ra,
                        "final_dec": result.final_dec,
                    }),
                );
                let iterations =
                    serde_json::to_value(&result.iterations).unwrap_or(serde_json::Value::Null);
                Ok(tool_success!({
                    "final_error_arcsec": result.final_error_arcsec,
                    "attempts": result.attempts,
                    "final_ra": result.final_ra,
                    "final_dec": result.final_dec,
                    "iterations": iterations,
                }))
            }
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }
}

/// Adapter satisfying the three [`center_on_target`] traits
/// (`CaptureOps`, `PlateSolveOps`, `MountOps`) by delegating to the
/// existing [`McpHandler`] helpers.
///
/// `PlateSolveOps` calls back into the in-process `plate_solve`
/// handler with `use_mount_hints: true`, so the hours→degrees
/// conversion lives in exactly one place (the `plate_solve` Contract).
/// `MountOps::sync_to` calls `do_sync_mount` after dividing the
/// solved degrees by 15 to match Alpaca's RA-in-hours convention.
/// `MountOps::slew_to` calls `do_slew_blocking` with the operator-
/// configured `settle_after_slew` so iteration cadence honours
/// rig-specific mechanical settle.
pub(crate) struct CenterOnTargetAdapter<'a> {
    pub(crate) handler: &'a McpHandler,
    pub(crate) camera_id: String,
}

#[async_trait::async_trait]
impl imaging::tools::center_on_target::CaptureOps for CenterOnTargetAdapter<'_> {
    async fn capture(&self, duration: Duration) -> std::result::Result<String, String> {
        let (_image_path, document_id) = self.handler.do_capture(&self.camera_id, duration).await?;
        Ok(document_id)
    }
}

#[async_trait::async_trait]
impl imaging::tools::center_on_target::PlateSolveOps for CenterOnTargetAdapter<'_> {
    async fn solve(
        &self,
        document_id: &str,
    ) -> std::result::Result<imaging::tools::center_on_target::SolveOutcome, String> {
        // Inline call into the in-process `plate_solve` path. Going
        // through the MCP transport would re-encode the JSON params
        // and pay an extra HTTP round-trip — not what compound tools
        // want (mirrors `auto_focus`'s direct `measure_via_document`
        // call, where the auto-focus loop similarly skips the MCP
        // transport for its inner `measure_basic` calls).
        let client = self
            .handler
            .plate_solver
            .as_ref()
            .cloned()
            .ok_or_else(|| "plate_solve: plate solver not configured".to_string())?;

        let doc = self
            .handler
            .image_cache
            .resolve_document(document_id)
            .await
            .ok_or_else(|| format!("plate_solve: document not found: {}", document_id))?;

        // Source the pointing hint from the mount, paralleling
        // plate_solve's `use_mount_hints: true` path. Failure modes
        // surface verbatim (no mount configured / disconnected /
        // Alpaca read error) — center_on_target needs a connected
        // mount anyway, and we already validated that up front.
        let (ra_hint, dec_hint) = self
            .handler
            .read_mount_hints_for_plate_solve()
            .await
            .map(|(ra, dec)| (Some(ra), Some(dec)))
            .unwrap_or((None, None));

        let request = rp_plate_solver::SolveRequest {
            fits_path: doc.file_path.clone(),
            ra_hint,
            dec_hint,
            fov_hint_deg: None,
            search_radius_deg: self.handler.plate_solver_default_search_radius_deg,
            timeout: None,
        };

        let outcome = client.solve(request).await.map_err(|e| match e {
            rp_plate_solver::SolveError::ServiceUnreachable(reason) => {
                format!("plate_solve: service unreachable: {}", reason)
            }
            rp_plate_solver::SolveError::Wrapper {
                code,
                message,
                details,
            } => {
                if details.is_null() {
                    format!("plate_solve: {}: {}", code, message)
                } else {
                    format!("plate_solve: {}: {} (details: {})", code, message, details)
                }
            }
            rp_plate_solver::SolveError::Internal(reason) => {
                format!("plate_solve: internal: {}", reason)
            }
        })?;

        // Persist the per-iteration `wcs` section to the captured
        // document — same side-effect the standalone `plate_solve`
        // tool would write. Failure is logged and ignored.
        let payload = serde_json::json!({
            "ra_center": outcome.ra_center,
            "dec_center": outcome.dec_center,
            "pixel_scale_arcsec": outcome.pixel_scale_arcsec,
            "rotation_deg": outcome.rotation_deg,
            "solver": outcome.solver,
        });
        if let Err(e) = self
            .handler
            .image_cache
            .put_section(document_id, "wcs", payload)
            .await
        {
            tracing::debug!(error = %e, document_id, "failed to persist wcs section");
        }

        Ok(imaging::tools::center_on_target::SolveOutcome {
            ra_center_deg: outcome.ra_center,
            dec_center_deg: outcome.dec_center,
        })
    }
}

#[async_trait::async_trait]
impl imaging::tools::center_on_target::MountOps for CenterOnTargetAdapter<'_> {
    async fn sync_to(&self, ra_deg: f64, dec_deg: f64) -> std::result::Result<(), String> {
        // The driver works in degrees; Alpaca's RA is hours.
        let ra_hours = ra_deg / 15.0;
        self.handler.do_sync_mount(ra_hours, dec_deg).await
    }
    async fn slew_to(&self, ra_hours: f64, dec_deg: f64) -> std::result::Result<(), String> {
        let settle = self
            .handler
            .equipment
            .find_mount()
            .and_then(|m| m.config.settle_after_slew)
            .unwrap_or_default();
        self.handler
            .do_slew_blocking(ra_hours, dec_deg, settle)
            .await
            .map(|_| ())
    }
}
