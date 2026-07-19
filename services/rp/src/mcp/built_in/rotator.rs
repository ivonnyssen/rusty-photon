//! Rotator tool category: `move_rotator`, `get_rotator_position`
//! (rp.md § Rotator Tool Details).
//!
//! Both tools address the device as `rotator_id` *or* `train_id` —
//! exactly one — where `train_id` resolves through the optical-train
//! model and requires the train to contain exactly one rotator.
//! `move_rotator` moves to an absolute **sky** angle (the ASCOM
//! `Position` frame) and blocks polling `IsMoving` until idle under a
//! fixed 120 s ceiling: there is no per-rotator rate config yet, so
//! the `move_rotator_started` envelope carries no predictive
//! deadline. `moved_trains` on the result lists every train
//! containing the rotator — informational in this phase (the
//! rotate-while-guiding ladder is plan phase T4).

use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tracing::debug;

use super::super::handler::McpHandler;
use super::super::{resolve_device, tool_error, tool_success};
use crate::equipment::trains::TrainDeviceKind;
use crate::events::EventEnvelope;

/// Ceiling on the post-move `IsMoving` poll. Rotators have no rate
/// config to size a predictive deadline from; 120 s mirrors the
/// focuser fallback ceiling and covers a worst-case half-turn on the
/// slowest amateur rotators.
const ROTATOR_MOVE_DEADLINE: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MoveRotatorParams {
    /// Roster rotator id. Exactly one of `rotator_id` / `train_id`.
    #[serde(default)]
    pub rotator_id: Option<String>,
    /// Optical-train id; resolves the train's sole rotator.
    #[serde(default)]
    pub train_id: Option<String>,
    /// Target absolute sky angle in degrees, `0.0 <= angle < 360.0`
    /// (the ASCOM Position frame).
    #[serde(default)]
    pub angle: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RotatorPositionParams {
    /// Roster rotator id. Exactly one of `rotator_id` / `train_id`.
    #[serde(default)]
    pub rotator_id: Option<String>,
    /// Optical-train id; resolves the train's sole rotator.
    #[serde(default)]
    pub train_id: Option<String>,
}

#[tool_router(router = tool_router_rotator, vis = "pub")]
impl McpHandler {
    #[tool(
        description = "Move the rotator to an absolute sky angle in degrees (0.0 <= angle < 360.0, the ASCOM Position frame), blocking until it reports idle. Address as rotator_id or train_id (the train's sole rotator). Returns the read-back sky and mechanical angles plus moved_trains, the optical trains containing the rotator."
    )]
    pub(crate) async fn move_rotator(
        &self,
        Parameters(params): Parameters<MoveRotatorParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let rotator_id = match self.resolve_rotator_addressing(
            "move_rotator",
            params.rotator_id.as_deref(),
            params.train_id.as_deref(),
        ) {
            Ok(id) => id,
            Err(result) => return Ok(*result),
        };
        let angle = match params.angle {
            Some(a) => a,
            None => return Ok(tool_error!("missing required parameter: angle")),
        };
        if !angle.is_finite() || !(0.0..360.0).contains(&angle) {
            return Ok(tool_error!(
                "move_rotator: angle out of range: {} (expected 0.0 <= angle < 360.0)",
                angle
            ));
        }
        let (_entry, rot) = resolve_device!(self, find_rotator, &rotator_id, "rotator");
        let moved_trains: Vec<String> = self
            .trains
            .trains_with_device(&rotator_id)
            .iter()
            .map(|t| t.id.clone())
            .collect();

        let operation_id = uuid::Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();
        self.event_bus.emit_operation(EventEnvelope::started(
            "move_rotator",
            &operation_id,
            started_at,
            serde_json::json!({ "rotator_id": rotator_id, "angle": angle }),
        ));

        let move_result: Result<(f64, f64), String> = async {
            debug!(rotator_id, angle, "moving rotator");
            rot.move_absolute(angle)
                .await
                .map_err(|e| format!("failed to move rotator: {e}"))?;

            let deadline = std::time::Instant::now() + ROTATOR_MOVE_DEADLINE;
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                match rot.is_moving().await {
                    Ok(false) => break,
                    Ok(true) if std::time::Instant::now() < deadline => continue,
                    Ok(true) => return Err("timeout waiting for rotator to settle".to_string()),
                    Err(e) => return Err(format!("error polling rotator is_moving: {e}")),
                }
            }

            let position = rot
                .position()
                .await
                .map_err(|e| format!("failed to read rotator position: {e}"))?;
            let mechanical = rot
                .mechanical_position()
                .await
                .map_err(|e| format!("failed to read rotator mechanical position: {e}"))?;
            Ok((position, mechanical))
        }
        .await;

        match move_result {
            Ok((position, mechanical)) => {
                self.event_bus.emit_operation(EventEnvelope::complete(
                    "move_rotator",
                    &operation_id,
                    started_at,
                    serde_json::json!({
                        "rotator_id": rotator_id,
                        "angle": position,
                        "mechanical_angle": mechanical,
                        "moved_trains": moved_trains,
                    }),
                ));
                Ok(tool_success!({
                    "rotator_id": rotator_id,
                    "angle": position,
                    "mechanical_angle": mechanical,
                    "moved_trains": moved_trains,
                }))
            }
            Err(e) => {
                self.event_bus.emit_operation(EventEnvelope::failed(
                    "move_rotator",
                    &operation_id,
                    started_at,
                    &e,
                ));
                Ok(tool_error!("{}", e))
            }
        }
    }

    #[tool(
        description = "Read the rotator's sky angle, mechanical angle, and motion state. Address as rotator_id or train_id (the train's sole rotator). Cheap; safe to poll."
    )]
    pub(crate) async fn get_rotator_position(
        &self,
        Parameters(params): Parameters<RotatorPositionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let rotator_id = match self.resolve_rotator_addressing(
            "get_rotator_position",
            params.rotator_id.as_deref(),
            params.train_id.as_deref(),
        ) {
            Ok(id) => id,
            Err(result) => return Ok(*result),
        };
        let (_entry, rot) = resolve_device!(self, find_rotator, &rotator_id, "rotator");

        let position = match rot.position().await {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("failed to read rotator position: {}", e)),
        };
        let mechanical = match rot.mechanical_position().await {
            Ok(p) => p,
            Err(e) => {
                return Ok(tool_error!(
                    "failed to read rotator mechanical position: {}",
                    e
                ))
            }
        };
        let is_moving = match rot.is_moving().await {
            Ok(m) => m,
            Err(e) => return Ok(tool_error!("failed to read rotator is_moving: {}", e)),
        };
        Ok(tool_success!({
            "rotator_id": rotator_id,
            "angle": position,
            "mechanical_angle": mechanical,
            "is_moving": is_moving,
        }))
    }
}

impl McpHandler {
    /// Resolve the `rotator_id` / `train_id` addressing shared by both
    /// rotator tools: exactly one must be present, and a train must
    /// contain exactly one rotator. Returns the resolved roster id, or
    /// the ready-to-return error `CallToolResult` (boxed —
    /// `clippy::result_large_err`).
    fn resolve_rotator_addressing(
        &self,
        tool: &str,
        rotator_id: Option<&str>,
        train_id: Option<&str>,
    ) -> Result<String, Box<CallToolResult>> {
        match (rotator_id, train_id) {
            (Some(_), Some(_)) | (None, None) => Err(Box::new(tool_error!(
                "{}: pass exactly one of rotator_id or train_id",
                tool
            ))),
            (Some(id), None) => Ok(id.to_string()),
            (None, Some(train_id)) => {
                let Some(train) = self.trains.train(train_id) else {
                    return Err(Box::new(tool_error!("train not found: {}", train_id)));
                };
                let rotators: Vec<&str> = train
                    .devices
                    .iter()
                    .filter(|d| d.kind == TrainDeviceKind::Rotator)
                    .map(|d| d.id.as_str())
                    .collect();
                match rotators.as_slice() {
                    [] => Err(Box::new(tool_error!("train '{}' has no rotator", train_id))),
                    [id] => Ok((*id).to_string()),
                    many => Err(Box::new(tool_error!(
                        "train '{}' has {} rotators; pass rotator_id",
                        train_id,
                        many.len()
                    ))),
                }
            }
        }
    }
}
