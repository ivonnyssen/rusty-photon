//! Auto-focus tool category: the `auto_focus` V-curve compound tool
//! and the train-aware `refocus_train` expansion (rp.md § Optical
//! Trains, §`auto_focus` Contract, §`refocus_train` Contract).

use std::collections::HashSet;
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
use super::super::{tool_error, tool_success};
use crate::config::{TrainAutoFocusConfig, TrainPurpose};
use crate::events::EventEnvelope;
use crate::imaging;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AutoFocusToolParams {
    /// Camera that captures each sweep frame. Pass together with
    /// `focuser_id`; mutually exclusive with `train_id`.
    #[serde(default)]
    pub camera_id: Option<String>,
    /// Focuser to sweep.
    #[serde(default)]
    pub focuser_id: Option<String>,
    /// Optical-train id: resolves the train's terminal camera and
    /// focuser, with the sweep parameters falling back to the train's
    /// `auto_focus` config block. Mutually exclusive with the explicit
    /// pair. The guiding train is refused (guide-train auto-focus
    /// reads PHD2 metrics — plan phase T4).
    #[serde(default)]
    pub train_id: Option<String>,
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
    #[serde(default)]
    pub threshold_sigma: Option<f64>,
    /// Minimum number of non-null HFR samples for the parabolic fit.
    /// Default 5.
    #[serde(default)]
    pub min_fit_points: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RefocusTrainParams {
    /// The train whose refocus trigger to expand.
    #[serde(default)]
    pub train_id: Option<String>,
    /// Free-form trigger description recorded on `refocus_started`
    /// and the result (e.g. "temperature_drift"). Default "manual".
    #[serde(default)]
    pub reason: Option<String>,
}

/// One fully-resolved AF step of a `refocus_train` expansion.
struct PlannedAfStep {
    focuser_id: String,
    train_id: String,
    camera_id: String,
    af_params: imaging::tools::auto_focus::AutoFocusParams,
}

#[tool_router(router = tool_router_auto_focus, vis = "pub")]
impl McpHandler {
    #[tool(
        description = "V-curve auto-focus: sweep ± half_width around the focuser's current position, capture and run measure_basic at each step, fit a parabola in HFR, and move the focuser to the fitted minimum. Address the devices as camera_id + focuser_id, or as train_id (the train's terminal camera + focuser, sweep parameters falling back to the train's auto_focus config block)."
    )]
    pub(crate) async fn auto_focus(
        &self,
        Parameters(params): Parameters<AutoFocusToolParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let progress_sink = ProgressSink::from_request_context(&ctx);
        self.auto_focus_inner(params, progress_sink).await
    }

    #[tool(
        description = "Expand one refocus trigger on an optical train into the dependency-ordered auto-focus sequence: shared focusers upstream-first (each run in the train where it is terminal), then the train's own terminal focuser. Sweep parameters come from each run train's auto_focus config block; guiding is paused around the sequence when a step moves a guiding-train focuser."
    )]
    pub(crate) async fn refocus_train(
        &self,
        Parameters(params): Parameters<RefocusTrainParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let progress_sink = ProgressSink::from_request_context(&ctx);
        self.refocus_train_inner(params, progress_sink).await
    }

    /// Body of the `auto_focus` MCP tool, split out so unit tests can
    /// pass `None` for the progress sink without constructing a real
    /// rmcp `Peer` (its constructor is `pub(crate)` in rmcp 1.7).
    pub(crate) async fn auto_focus_inner(
        &self,
        mut params: AutoFocusToolParams,
        progress_sink: Option<ProgressSink>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // Addressing: exactly one of the explicit pair or train_id.
        // Train addressing resolves the train's terminal devices and
        // merges the train's `auto_focus` config block under the
        // per-call parameters field by field, before the
        // "missing required parameter" checks below.
        let (camera_id, focuser_id) = if let Some(train_id) = params.train_id.take() {
            if params.camera_id.is_some() || params.focuser_id.is_some() {
                return Ok(tool_error!(
                    "auto_focus: train_id is mutually exclusive with camera_id and focuser_id"
                ));
            }
            let Some(train) = self.trains.train(&train_id) else {
                return Ok(tool_error!("train not found: {}", train_id));
            };
            if train.purpose == TrainPurpose::Guiding {
                return Ok(tool_error!(
                    "auto_focus: train '{}' is the guiding train — guide-train auto-focus \
                     reads PHD2 metrics and arrives with the guiding integration (plan phase T4)",
                    train_id
                ));
            }
            let Some(camera_id) = train.camera_id() else {
                return Ok(tool_error!("train '{}' has no camera", train_id));
            };
            let Some(focuser_id) = train.terminal_focuser() else {
                return Ok(tool_error!("train '{}' has no focuser", train_id));
            };
            let (camera_id, focuser_id) = (camera_id.to_string(), focuser_id.to_string());
            if let Some(block) = &train.auto_focus {
                merge_block_into_params(&mut params, block);
            }
            (camera_id, focuser_id)
        } else {
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
            (camera_id, focuser_id)
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

        let af_params = imaging::tools::auto_focus::AutoFocusParams {
            duration,
            step_size,
            half_width,
            min_area,
            max_area,
            threshold_sigma: params.threshold_sigma.unwrap_or(5.0),
            min_fit_points: params.min_fit_points.unwrap_or(5),
        };

        match self
            .run_auto_focus_step(&camera_id, &focuser_id, af_params, progress_sink)
            .await
        {
            Ok(result) => {
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

    /// Body of the `refocus_train` MCP tool — see `auto_focus_inner`
    /// for why the split exists.
    pub(crate) async fn refocus_train_inner(
        &self,
        params: RefocusTrainParams,
        progress_sink: Option<ProgressSink>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(train_id) = params.train_id else {
            return Ok(tool_error!("missing required parameter: train_id"));
        };
        let reason = params.reason.unwrap_or_else(|| "manual".to_string());

        // Expansion + per-step resolution, all before any motion or
        // event: an invalid expansion never touches hardware.
        if self.trains.train(&train_id).is_none() {
            return Ok(tool_error!("train not found: {}", train_id));
        }
        let steps = self.trains.af_sequence(&train_id).unwrap_or_default();
        if steps.is_empty() {
            return Ok(tool_error!(
                "refocus_train: train '{}' has no focusers",
                train_id
            ));
        }
        if let Some(guiding) = self.trains.guiding_train() {
            if steps.iter().any(|s| s.train_id == guiding.id) {
                return Ok(tool_error!(
                    "refocus_train: the expansion includes an auto-focus step in the guiding \
                     train '{}' — guide-train auto-focus reads PHD2 metrics and arrives with \
                     the guiding integration (plan phase T4)",
                    guiding.id
                ));
            }
        }
        let mut planned = Vec::with_capacity(steps.len());
        for step in &steps {
            let Some(run_train) = self.trains.train(&step.train_id) else {
                return Ok(tool_error!("train not found: {}", step.train_id));
            };
            let Some(block) = &run_train.auto_focus else {
                return Ok(tool_error!(
                    "refocus_train: train '{}' has no auto_focus config block \
                     (required for the step focusing '{}')",
                    step.train_id,
                    step.focuser_id
                ));
            };
            let Some(camera_id) = run_train.camera_id() else {
                return Ok(tool_error!("train '{}' has no camera", step.train_id));
            };
            planned.push(PlannedAfStep {
                focuser_id: step.focuser_id.clone(),
                train_id: step.train_id.clone(),
                camera_id: camera_id.to_string(),
                af_params: af_params_from_block(block),
            });
        }

        // Guiding handshake decision (rp.md §`refocus_train` Contract):
        // pause only when a step moves a guiding-train focuser AND the
        // guider is configured AND it reports an active loop. A stats
        // read that fails or reports not-guiding skips the handshake —
        // a broken guider service must not block a refocus.
        let guiding_members: HashSet<&str> = self
            .trains
            .guiding_train()
            .map(|t| t.devices.iter().map(|d| d.id.as_str()).collect())
            .unwrap_or_default();
        let touches_guiding = planned
            .iter()
            .any(|s| guiding_members.contains(s.focuser_id.as_str()));
        let mut pause_client = None;
        if touches_guiding {
            if let Some(client) = self.guider.clone() {
                match client.guiding_stats().await {
                    Ok(stats) if stats.guiding => pause_client = Some(client),
                    Ok(_) => {
                        debug!(
                            train_id,
                            "guider reports not guiding; skipping pause handshake"
                        );
                    }
                    Err(e) => {
                        debug!(train_id, error = %e, "guider stats unreachable; skipping pause handshake");
                    }
                }
            }
        }
        let guiding_paused = pause_client.is_some();

        let operation_id = uuid::Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();
        self.event_bus.emit_operation(EventEnvelope::started(
            "refocus",
            &operation_id,
            started_at,
            serde_json::json!({
                "train_id": train_id,
                "reason": reason,
                "steps": planned
                    .iter()
                    .map(|s| serde_json::json!({
                        "focuser_id": s.focuser_id,
                        "train_id": s.train_id,
                    }))
                    .collect::<Vec<_>>(),
                "guiding_paused": guiding_paused,
            }),
        ));

        if let Some(client) = &pause_client {
            if let Err(e) = client.pause_guiding(false).await {
                let msg = format!("refocus_train: failed to pause guiding: {e}");
                self.event_bus.emit_operation(EventEnvelope::failed(
                    "refocus",
                    &operation_id,
                    started_at,
                    &msg,
                ));
                return Ok(tool_error!("{}", msg));
            }
        }

        let mut completed = Vec::with_capacity(planned.len());
        for (i, step) in planned.iter().enumerate() {
            debug!(
                train_id,
                focuser_id = %step.focuser_id,
                run_train = %step.train_id,
                "running refocus step {} of {}",
                i + 1,
                planned.len()
            );
            match self
                .run_auto_focus_step(
                    &step.camera_id,
                    &step.focuser_id,
                    step.af_params.clone(),
                    progress_sink.clone(),
                )
                .await
            {
                Ok(result) => completed.push(serde_json::json!({
                    "focuser_id": step.focuser_id,
                    "train_id": step.train_id,
                    "camera_id": step.camera_id,
                    "best_position": result.best_position,
                    "best_hfr": result.best_hfr,
                    "samples_used": result.samples_used,
                })),
                Err(e) => {
                    // Later steps depend on this one having landed, so
                    // stop here — but never leave guiding paused.
                    let resume_note = match &pause_client {
                        Some(client) => match client.resume_guiding().await {
                            Ok(()) => String::new(),
                            Err(re) => format!("; also failed to resume guiding: {re}"),
                        },
                        None => String::new(),
                    };
                    let msg = format!(
                        "refocus_train: step {} (focuser '{}' in train '{}') failed: {e}{resume_note}",
                        i + 1,
                        step.focuser_id,
                        step.train_id,
                    );
                    self.event_bus.emit_operation(EventEnvelope::failed(
                        "refocus",
                        &operation_id,
                        started_at,
                        &msg,
                    ));
                    return Ok(tool_error!("{}", msg));
                }
            }
        }

        if let Some(client) = &pause_client {
            if let Err(e) = client.resume_guiding().await {
                let msg =
                    format!("refocus_train: all steps completed but resuming guiding failed: {e}");
                self.event_bus.emit_operation(EventEnvelope::failed(
                    "refocus",
                    &operation_id,
                    started_at,
                    &msg,
                ));
                return Ok(tool_error!("{}", msg));
            }
        }

        self.event_bus.emit_operation(EventEnvelope::complete(
            "refocus",
            &operation_id,
            started_at,
            serde_json::json!({
                "train_id": train_id,
                "steps": completed,
            }),
        ));
        Ok(tool_success!({
            "train_id": train_id,
            "reason": reason,
            "guiding_paused": guiding_paused,
            "steps": completed,
        }))
    }

    /// One full V-curve run for a resolved camera + focuser pair: the
    /// shared body of `auto_focus` and each `refocus_train` step.
    /// Resolves the devices, reads the starting position and
    /// temperature, emits the `focus_started` / `focus_complete` /
    /// `focus_failed` triple, and drives the sweep through
    /// [`AutoFocusAdapter`].
    async fn run_auto_focus_step(
        &self,
        camera_id: &str,
        focuser_id: &str,
        af_params: imaging::tools::auto_focus::AutoFocusParams,
        progress_sink: Option<ProgressSink>,
    ) -> Result<imaging::tools::auto_focus::AutoFocusResult, String> {
        // Resolve devices early — the standard "<kind> not found" /
        // "<kind> not connected" errors, camera before focuser to
        // match input order in the contract. The camera is resolved
        // purely for the connection check; `do_capture` re-resolves.
        let cam_entry = self
            .equipment
            .find_camera(camera_id)
            .ok_or_else(|| format!("camera not found: {camera_id}"))?;
        if cam_entry.device.is_none() {
            return Err(format!("camera not connected: {camera_id}"));
        }
        let foc_entry = self
            .equipment
            .find_focuser(focuser_id)
            .ok_or_else(|| format!("focuser not found: {focuser_id}"))?;
        let foc = foc_entry
            .device
            .as_ref()
            .cloned()
            .ok_or_else(|| format!("focuser not connected: {focuser_id}"))?;

        // Read the current focuser position + temperature exactly once
        // each (per the Contract algorithm step 1) and thread the values
        // through to both `focus_started` *and* `run_auto_focus` so the
        // event payload and the result's `temperature_c`/sweep-grid
        // origin can never disagree. Temperature is informational only:
        // any read failure (NOT_IMPLEMENTED or transient) becomes
        // `temperature_c: null`; we don't abort an auto-focus run over
        // a missing thermistor.
        let starting_position = foc
            .position()
            .await
            .map_err(|e| format!("failed to read focuser position: {e}"))?;
        let starting_temperature_c: Option<f64> = foc.temperature().await.ok();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();
        self.event_bus.emit_operation(EventEnvelope::started(
            "focus",
            &operation_id,
            started_at,
            serde_json::json!({
                "camera_id": camera_id,
                "focuser_id": focuser_id,
                "position": starting_position,
                "temperature": starting_temperature_c,
            }),
        ));

        let bounds = (foc_entry.config.min_position, foc_entry.config.max_position);

        // Store the per-request sink on the adapter so every
        // inner `do_capture` / `do_move_focuser_blocking` call emits
        // progress through the same `progressToken`. See
        // `mcp::progress` for the rmcp 300 s session keep-alive race
        // this guards against.
        let adapter = AutoFocusAdapter {
            handler: self,
            camera_id: camera_id.to_string(),
            focuser_id: focuser_id.to_string(),
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
                self.event_bus.emit_operation(EventEnvelope::complete(
                    "focus",
                    &operation_id,
                    started_at,
                    serde_json::json!({
                        "camera_id": camera_id,
                        "focuser_id": focuser_id,
                        "position": result.best_position,
                        "hfr": result.best_hfr,
                        "samples_used": result.samples_used,
                    }),
                ));
                Ok(result)
            }
            Err(e) => {
                self.event_bus.emit_operation(EventEnvelope::failed(
                    "focus",
                    &operation_id,
                    started_at,
                    &e.to_string(),
                ));
                Err(e.to_string())
            }
        }
    }
}

/// Fill sweep parameters the call omitted from the train's
/// `auto_focus` config block — per-call values win field by field.
fn merge_block_into_params(params: &mut AutoFocusToolParams, block: &TrainAutoFocusConfig) {
    params.duration = params.duration.or(Some(block.duration));
    params.step_size = params.step_size.or(Some(block.step_size.value()));
    params.half_width = params.half_width.or(Some(block.half_width.value()));
    params.min_area = params.min_area.or(Some(block.min_area));
    params.max_area = params.max_area.or(Some(block.max_area));
    params.threshold_sigma = params.threshold_sigma.or(block.threshold_sigma);
    params.min_fit_points = params.min_fit_points.or(block.min_fit_points);
}

/// The full sweep-parameter set from a train's `auto_focus` block —
/// what each `refocus_train` step runs with.
fn af_params_from_block(
    block: &TrainAutoFocusConfig,
) -> imaging::tools::auto_focus::AutoFocusParams {
    imaging::tools::auto_focus::AutoFocusParams {
        duration: block.duration,
        step_size: block.step_size.value(),
        half_width: block.half_width.value(),
        min_area: block.min_area,
        max_area: block.max_area,
        threshold_sigma: block.threshold_sigma.unwrap_or(5.0),
        min_fit_points: block.min_fit_points.unwrap_or(5),
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
