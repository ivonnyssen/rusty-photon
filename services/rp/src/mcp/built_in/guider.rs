//! Guider tool category: `start_guiding`, `stop_guiding`, `dither`,
//! `pause_guiding`, `resume_guiding`, `get_guiding_stats`.
//!
//! All six proxy to the guider rp-managed service (the `phd2-guider`
//! binary's `serve` mode) through the `rp-guider` HTTP client on
//! `McpHandler::guider`; `None` there means every tool errors with
//! "guider not configured". Quantities are **guide-camera pixels**
//! (`*_px`, `settle_pixels`) — the service rejects arcseconds-style
//! thresholds because a pixel scale only exists after PHD2
//! calibration.
//!
//! The settle-blocking operations emit operation-event triples that
//! terminate in `*_settled` rather than `*_complete`:
//! `guide_started` / `guide_settled` / `guide_failed` and
//! `dither_started` / `dither_settled` / `dither_failed`.
//! `stop_guiding` emits the `guide_stopped` point event with
//! `reason: "requested"` (the safety enforcer emits the same event
//! with `reason: "safety"`).

use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::handler::McpHandler;
use super::super::{tool_error, tool_success};
use crate::events::EventEnvelope;

/// The guider service's settle backstop grace: it fails a wedged
/// settle wait `settle.timeout` plus this margin after the RPC (see
/// `phd2-guider.md` § "HTTP Service Mode"). Mirrored here to size the
/// `max_duration_ms` deadline carried on `guide_started` /
/// `dither_started`.
const SETTLE_BACKSTOP_GRACE: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartGuidingParams {
    /// Force a fresh PHD2 calibration before guiding starts.
    /// Defaults to false (reuse the existing calibration).
    #[serde(default)]
    pub recalibrate: Option<bool>,
    /// Settle threshold in guide-camera pixels. Falls back to
    /// `guider.settle_pixels` from rp config, then to the guider
    /// service's own default.
    #[serde(default)]
    pub settle_pixels: Option<f64>,
    /// How long guiding must hold within the threshold (humantime
    /// string). Same fallback chain as `settle_pixels`.
    #[serde(default, with = "humantime_serde::option")]
    #[schemars(with = "Option<String>")]
    pub settle_time: Option<Duration>,
    /// Deadline for the settle to complete (humantime string). Same
    /// fallback chain as `settle_pixels`.
    #[serde(default, with = "humantime_serde::option")]
    #[schemars(with = "Option<String>")]
    pub settle_timeout: Option<Duration>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StopGuidingParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DitherParams {
    /// Dither offset in guide-camera pixels. Falls back to
    /// `guider.dither_pixels` from rp config; an error when neither
    /// is set.
    #[serde(default)]
    pub pixels: Option<f64>,
    /// Restrict the dither to the RA axis (declination drift stays
    /// untouched). Defaults to false.
    #[serde(default)]
    pub ra_only: Option<bool>,
    /// Settle threshold in guide-camera pixels (fallback chain as in
    /// `start_guiding`).
    #[serde(default)]
    pub settle_pixels: Option<f64>,
    /// How long guiding must hold within the threshold (humantime
    /// string).
    #[serde(default, with = "humantime_serde::option")]
    #[schemars(with = "Option<String>")]
    pub settle_time: Option<Duration>,
    /// Deadline for the settle to complete (humantime string).
    #[serde(default, with = "humantime_serde::option")]
    #[schemars(with = "Option<String>")]
    pub settle_timeout: Option<Duration>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PauseGuidingParams {
    /// When true, also pause guide-camera looping ("full" pause);
    /// otherwise only guide corrections pause and the camera keeps
    /// looping. Defaults to false.
    #[serde(default)]
    pub full: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResumeGuidingParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetGuidingStatsParams {}

#[tool_router(router = tool_router_guider, vis = "pub")]
impl McpHandler {
    #[tool(
        description = "Start the guiding loop via the guider rp-managed service and block until the post-start settle completes. Settle overrides (settle_pixels, settle_time, settle_timeout) fall back to rp's guider config, then to the service's own defaults. Returns the rolling RMS in guide-camera pixels."
    )]
    pub(crate) async fn start_guiding(
        &self,
        Parameters(params): Parameters<StartGuidingParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(client) = self.guider.clone() else {
            return Ok(tool_error!("start_guiding: guider not configured"));
        };
        let settle = self.merge_settle(
            params.settle_pixels,
            params.settle_time,
            params.settle_timeout,
        );
        let recalibrate = params.recalibrate.unwrap_or(false);

        let operation_id = uuid::Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();
        let started_payload = serde_json::json!({
            "recalibrate": recalibrate,
            "settle_pixels": settle.as_ref().and_then(|s| s.pixels),
            "settle_time": humantime_or_null(settle.as_ref().and_then(|s| s.time)),
            "settle_timeout": humantime_or_null(settle.as_ref().and_then(|s| s.timeout)),
        });
        self.event_bus.emit_operation(with_settle_deadlines(
            EventEnvelope::started("guide", &operation_id, started_at, started_payload),
            settle.as_ref(),
        ));

        match client
            .start_guiding(rp_guider::StartGuidingRequest {
                recalibrate,
                settle,
            })
            .await
        {
            Ok(outcome) => {
                self.event_bus.emit_operation(EventEnvelope::settled(
                    "guide",
                    &operation_id,
                    started_at,
                    settled_payload(&outcome),
                ));
                Ok(tool_success!({
                    "state": outcome.state,
                    "rms_ra_px": outcome.rms_ra_px,
                    "rms_dec_px": outcome.rms_dec_px,
                    "total_rms_px": outcome.total_rms_px,
                    "sample_count": outcome.sample_count,
                }))
            }
            Err(e) => {
                let message = guider_error_text("start_guiding", &e);
                self.event_bus.emit_operation(EventEnvelope::failed(
                    "guide",
                    &operation_id,
                    started_at,
                    &message,
                ));
                Ok(tool_error!("{}", message))
            }
        }
    }

    #[tool(
        description = "Stop the guiding loop via the guider rp-managed service, blocking until the service confirms it is down. Idempotent: stopping an already-stopped guider succeeds. Emits guide_stopped with reason \"requested\"."
    )]
    pub(crate) async fn stop_guiding(
        &self,
        Parameters(_params): Parameters<StopGuidingParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(client) = self.guider.clone() else {
            return Ok(tool_error!("stop_guiding: guider not configured"));
        };
        match client.stop_guiding().await {
            Ok(()) => {
                self.event_bus.emit(
                    "guide_stopped",
                    serde_json::json!({ "reason": "requested" }),
                );
                Ok(tool_success!({ "state": "stopped" }))
            }
            Err(e) => Ok(tool_error!("{}", guider_error_text("stop_guiding", &e))),
        }
    }

    #[tool(
        description = "Dither the guide star by `pixels` guide-camera pixels via the guider rp-managed service and block until guiding re-settles. `pixels` falls back to rp's guider.dither_pixels config. ra_only restricts the offset to the RA axis. Settle overrides as in start_guiding."
    )]
    pub(crate) async fn dither(
        &self,
        Parameters(params): Parameters<DitherParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(client) = self.guider.clone() else {
            return Ok(tool_error!("dither: guider not configured"));
        };
        let Some(amount_px) = params.pixels.or(self.guider_defaults.dither_pixels) else {
            return Ok(tool_error!(
                "dither: missing required argument: provide pixels or configure guider.dither_pixels"
            ));
        };
        let settle = self.merge_settle(
            params.settle_pixels,
            params.settle_time,
            params.settle_timeout,
        );
        let ra_only = params.ra_only.unwrap_or(false);

        let operation_id = uuid::Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();
        let started_payload = serde_json::json!({
            "pixels": amount_px,
            "ra_only": ra_only,
            "settle_pixels": settle.as_ref().and_then(|s| s.pixels),
            "settle_time": humantime_or_null(settle.as_ref().and_then(|s| s.time)),
            "settle_timeout": humantime_or_null(settle.as_ref().and_then(|s| s.timeout)),
        });
        self.event_bus.emit_operation(with_settle_deadlines(
            EventEnvelope::started("dither", &operation_id, started_at, started_payload),
            settle.as_ref(),
        ));

        match client
            .dither(rp_guider::DitherRequest {
                amount_px,
                ra_only,
                settle,
            })
            .await
        {
            Ok(outcome) => {
                self.event_bus.emit_operation(EventEnvelope::settled(
                    "dither",
                    &operation_id,
                    started_at,
                    settled_payload(&outcome),
                ));
                Ok(tool_success!({
                    "state": outcome.state,
                    "rms_ra_px": outcome.rms_ra_px,
                    "rms_dec_px": outcome.rms_dec_px,
                    "total_rms_px": outcome.total_rms_px,
                    "sample_count": outcome.sample_count,
                }))
            }
            Err(e) => {
                let message = guider_error_text("dither", &e);
                self.event_bus.emit_operation(EventEnvelope::failed(
                    "dither",
                    &operation_id,
                    started_at,
                    &message,
                ));
                Ok(tool_error!("{}", message))
            }
        }
    }

    #[tool(
        description = "Pause guide corrections via the guider rp-managed service (e.g. during readout). full=true also pauses guide-camera looping."
    )]
    pub(crate) async fn pause_guiding(
        &self,
        Parameters(params): Parameters<PauseGuidingParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(client) = self.guider.clone() else {
            return Ok(tool_error!("pause_guiding: guider not configured"));
        };
        match client.pause_guiding(params.full.unwrap_or(false)).await {
            Ok(()) => Ok(tool_success!({ "state": "paused" })),
            Err(e) => Ok(tool_error!("{}", guider_error_text("pause_guiding", &e))),
        }
    }

    #[tool(description = "Resume guiding after pause_guiding.")]
    pub(crate) async fn resume_guiding(
        &self,
        Parameters(_params): Parameters<ResumeGuidingParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(client) = self.guider.clone() else {
            return Ok(tool_error!("resume_guiding: guider not configured"));
        };
        match client.resume_guiding().await {
            Ok(()) => Ok(tool_success!({ "state": "resumed" })),
            Err(e) => Ok(tool_error!("{}", guider_error_text("resume_guiding", &e))),
        }
    }

    #[tool(
        description = "Read the current guiding state and rolling RMS statistics (guide-camera pixels) from the guider rp-managed service. Cheap; safe to poll."
    )]
    pub(crate) async fn get_guiding_stats(
        &self,
        Parameters(_params): Parameters<GetGuidingStatsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(client) = self.guider.clone() else {
            return Ok(tool_error!("get_guiding_stats: guider not configured"));
        };
        match client.guiding_stats().await {
            Ok(stats) => Ok(tool_success!({
                "app_state": stats.app_state,
                "guiding": stats.guiding,
                "rms_ra_px": stats.rms_ra_px,
                "rms_dec_px": stats.rms_dec_px,
                "total_rms_px": stats.total_rms_px,
                "snr": stats.snr,
                "star_mass": stats.star_mass,
                "sample_count": stats.sample_count,
            })),
            Err(e) => Ok(tool_error!(
                "{}",
                guider_error_text("get_guiding_stats", &e)
            )),
        }
    }
}

impl McpHandler {
    /// Merge per-call settle parameters over the rp-config defaults,
    /// field by field. `None` when every field ends up unset — the
    /// wire then omits `settle` entirely and the guider service's own
    /// `settling` config applies.
    fn merge_settle(
        &self,
        pixels: Option<f64>,
        time: Option<Duration>,
        timeout: Option<Duration>,
    ) -> Option<rp_guider::SettleOverride> {
        let settle = rp_guider::SettleOverride {
            pixels: pixels.or(self.guider_defaults.settle_pixels),
            time: time.or(self.guider_defaults.settle_time),
            timeout: timeout.or(self.guider_defaults.settle_timeout),
        };
        (!settle.is_empty()).then_some(settle)
    }
}

/// Attach the settle deadline to a `*_started` envelope when the
/// resolved settle pins a timeout: predicted = the hold time (or the
/// timeout when no hold time is known), max = the timeout plus the
/// service's backstop grace. Without a resolved timeout rp cannot
/// know the service-side default, so the deadline fields stay
/// omitted (same posture as operations without predictions).
///
/// The service does not itself validate `settle_time <=
/// settle_timeout`, so a misconfigured hold time longer than the
/// timeout is clamped to the timeout here — `predicted_duration_ms`
/// must never exceed `max_duration_ms` (Sentinel treats that as a
/// contract violation).
fn with_settle_deadlines(
    envelope: EventEnvelope,
    settle: Option<&rp_guider::SettleOverride>,
) -> EventEnvelope {
    let Some(timeout) = settle.and_then(|s| s.timeout) else {
        return envelope;
    };
    let predicted = settle.and_then(|s| s.time).unwrap_or(timeout).min(timeout);
    let max = timeout.saturating_add(SETTLE_BACKSTOP_GRACE);
    envelope.with_deadlines(
        u64::try_from(predicted.as_millis()).unwrap_or(u64::MAX),
        u64::try_from(max.as_millis()).unwrap_or(u64::MAX),
    )
}

/// Payload shared by `guide_settled` / `dither_settled`: the settled
/// RMS snapshot in guide-camera pixels.
fn settled_payload(outcome: &rp_guider::SettledOutcome) -> serde_json::Value {
    serde_json::json!({
        "rms_ra_px": outcome.rms_ra_px,
        "rms_dec_px": outcome.rms_dec_px,
        "total_rms_px": outcome.total_rms_px,
        "sample_count": outcome.sample_count,
    })
}

/// Map a client error onto the tool-error text, mirroring
/// `do_plate_solve`'s formatting: unreachable / structured envelope
/// (code + message, details when present) / internal.
fn guider_error_text(tool: &str, e: &rp_guider::GuiderError) -> String {
    match e {
        rp_guider::GuiderError::ServiceUnreachable(reason) => {
            format!("{tool}: service unreachable: {reason}")
        }
        rp_guider::GuiderError::Service {
            code,
            message,
            details,
        } => {
            if details.is_null() {
                format!("{tool}: {code}: {message}")
            } else {
                format!("{tool}: {code}: {message} (details: {details})")
            }
        }
        rp_guider::GuiderError::Internal(reason) => format!("{tool}: internal: {reason}"),
    }
}

/// Humantime string for an optional duration; JSON `null` when unset
/// so `*_started` payloads keep a stable key set.
fn humantime_or_null(duration: Option<Duration>) -> serde_json::Value {
    match duration {
        Some(d) => serde_json::Value::String(humantime::format_duration(d).to_string()),
        None => serde_json::Value::Null,
    }
}
