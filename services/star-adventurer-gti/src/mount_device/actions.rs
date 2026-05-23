//! Driver-specific ASCOM `Action` handlers for [`MountDevice`].
//!
//! The standard ASCOM Telescope interface has no concept of an
//! "unpark from a named physical pose," so the three operations that
//! the design's [§Unpark from AP position] section defines are exposed
//! as vendor `Action(name, parameters)` calls, advertised through
//! `SupportedActions`. The `impl Device` block in [`super::device`]
//! dispatches to the handlers here.
//!
//! All three take the single `parameters` string as an
//! `ap_park_0..ap_park_5` token (see [`parse_ap_park`]). The two
//! persisting Actions (`SetUnparkFromApPosition`, `SetPreferredApPark`)
//! refuse when the driver was started without `--config`, mirroring the
//! `SetPark` capability gate — there is nowhere to persist to. The
//! recovery Action (`UnparkFromApPosition`) writes no config and does
//! not need a config path.
//!
//! [§Unpark from AP position]: ../../../../docs/services/star-adventurer-gti.md#unpark-from-ap-position

use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};

use crate::config::ApPark;

use super::park_persistence::write_mount_fields_to_config;
use super::MountDevice;

/// `SupportedActions` entry: persist a new `unpark_from_ap_position`.
pub(super) const ACTION_SET_UNPARK_FROM_AP_POSITION: &str = "SetUnparkFromApPosition";
/// `SupportedActions` entry: persist a new `preferred_ap_park`.
pub(super) const ACTION_SET_PREFERRED_AP_PARK: &str = "SetPreferredApPark";
/// `SupportedActions` entry: recovery unpark from a named physical pose.
pub(super) const ACTION_UNPARK_FROM_AP_POSITION: &str = "UnparkFromApPosition";

/// The driver-specific Action names, in `SupportedActions` order.
pub(super) const SUPPORTED_ACTIONS: [&str; 3] = [
    ACTION_SET_UNPARK_FROM_AP_POSITION,
    ACTION_SET_PREFERRED_AP_PARK,
    ACTION_UNPARK_FROM_AP_POSITION,
];

/// Parse an `Action` parameter string into an [`ApPark`]. Accepts the
/// six `ap_park_N` tokens (whitespace-trimmed); anything else is an
/// `INVALID_VALUE`.
fn parse_ap_park(parameter: &str) -> ASCOMResult<ApPark> {
    match parameter.trim() {
        "ap_park_0" => Ok(ApPark::ApPark0),
        "ap_park_1" => Ok(ApPark::ApPark1),
        "ap_park_2" => Ok(ApPark::ApPark2),
        "ap_park_3" => Ok(ApPark::ApPark3),
        "ap_park_4" => Ok(ApPark::ApPark4),
        "ap_park_5" => Ok(ApPark::ApPark5),
        other => Err(ASCOMError::new(
            ASCOMErrorCode::INVALID_VALUE,
            format!("unknown AP park {other:?}; expected one of ap_park_0..ap_park_5"),
        )),
    }
}

/// Canonical `ap_park_N` string for an [`ApPark`] — the persisted JSON
/// value and the Action return string.
fn ap_park_str(park: ApPark) -> &'static str {
    match park {
        ApPark::ApPark0 => "ap_park_0",
        ApPark::ApPark1 => "ap_park_1",
        ApPark::ApPark2 => "ap_park_2",
        ApPark::ApPark3 => "ap_park_3",
        ApPark::ApPark4 => "ap_park_4",
        ApPark::ApPark5 => "ap_park_5",
    }
}

impl MountDevice {
    /// Persist `key = value` to the on-disk config via the atomic-rename
    /// helper, off the async runtime. Both persisting Actions route
    /// through here; the caller has already verified `config_file_path`
    /// is `Some`.
    async fn persist_mount_ap_park(&self, key: &'static str, park: ApPark) -> ASCOMResult<()> {
        let path = self.config_file_path.as_ref().ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::INVALID_OPERATION,
                format!("{key} requires the driver to be started with --config <path>"),
            )
        })?;
        let path = path.clone();
        let value = serde_json::Value::String(ap_park_str(park).to_string());
        tokio::task::spawn_blocking(move || write_mount_fields_to_config(&path, &[(key, value)]))
            .await
            .map_err(|e| {
                ASCOMError::new(
                    ASCOMErrorCode::INVALID_OPERATION,
                    format!("{key} write task join error: {e}"),
                )
            })?
            .map_err(ASCOMError::from)
    }

    /// `SetUnparkFromApPosition(park)` — persist a new
    /// `unpark_from_ap_position`. Lazy: the value takes effect on the
    /// *next* fresh-power-up auto-seed (`seed_after_connect` re-reads it
    /// from disk); the current session's encoder is left untouched.
    /// Operators wanting an immediate apply use `UnparkFromApPosition`.
    pub(super) async fn handle_set_unpark_from_ap_position(
        &self,
        parameter: &str,
    ) -> ASCOMResult<String> {
        let park = parse_ap_park(parameter)?;
        self.persist_mount_ap_park("unpark_from_ap_position", park)
            .await?;
        tracing::debug!(
            unpark_from_ap_position = ?park,
            "SetUnparkFromApPosition persisted to config file"
        );
        Ok(ap_park_str(park).to_string())
    }

    /// `SetPreferredApPark(park)` — set the `Park()` target. Rejects
    /// `ap_park_0` (not a slew target). Persists to config and, when
    /// connected, re-resolves the live park target so the change applies
    /// to the next `Park()` without a reconnect; an explicit raw
    /// `park_*_ticks` override still wins per-axis.
    pub(super) async fn handle_set_preferred_ap_park(
        &self,
        parameter: &str,
    ) -> ASCOMResult<String> {
        let park = parse_ap_park(parameter)?;
        if park == ApPark::ApPark0 {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                "preferred AP park cannot be ap_park_0 (\"current position\" is not a slew target)",
            ));
        }
        self.persist_mount_ap_park("preferred_ap_park", park)
            .await?;
        // Re-resolve the live park target when connected so the change
        // applies this session. `read_connect_config` re-reads the
        // (just-persisted) value plus any raw `park_*_ticks` override
        // from disk, so the raw-ticks-win rule holds. When disconnected
        // the next connect resolves it.
        if self.session.read().await.is_some() {
            let cfg = self.read_connect_config().await?;
            self.load_park_target_after_connect(&cfg).await?;
        }
        tracing::debug!(preferred_ap_park = ?park, "SetPreferredApPark persisted to config file");
        Ok(ap_park_str(park).to_string())
    }

    /// `UnparkFromApPosition(park)` — recovery unpark. The operator
    /// asserts the OTA is physically at `park`; the driver makes the
    /// firmware encoder match, *regardless* of the current encoder
    /// state, then clears `AtPark`.
    ///
    /// Refuses when disconnected (`NOT_CONNECTED`), not parked, or
    /// slewing (`INVALID_OPERATION`). For `ap_park_0` ("current
    /// position") there is no encoder change — it is the standard
    /// `Unpark()` end state. For `ap_park_1..ap_park_5` it runs the
    /// [`MountDevice::reset_mount_encoders`] safe-stop-then-seed
    /// sequence before clearing `AtPark`.
    pub(super) async fn handle_unpark_from_ap_position(
        &self,
        parameter: &str,
    ) -> ASCOMResult<String> {
        let park = parse_ap_park(parameter)?;
        self.ensure_connected().await?;
        {
            let state = self.state.read().await;
            if !state.at_park {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_OPERATION,
                    "UnparkFromApPosition refused: mount is not parked",
                ));
            }
            if state.slew_in_progress {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_OPERATION,
                    "UnparkFromApPosition refused: slew in progress",
                ));
            }
        }
        if park == ApPark::ApPark0 {
            // "Current position": no encoder change — equivalent to a
            // standard `Unpark()`. The operator will plate-solve + sync.
            tracing::debug!("UnparkFromApPosition(ap_park_0): clearing AtPark, no encoder change");
        } else {
            // The operator-confirmed physical pose: make the firmware
            // encoder match it regardless of current state.
            let (ra_ticks, dec_ticks) = self
                .ap_park_target_ticks(park)
                .await
                .ok_or(ASCOMError::NOT_CONNECTED)?;
            let guard = self.session.read().await;
            let session = guard.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;
            self.reset_mount_encoders(session, ra_ticks, dec_ticks)
                .await?;
            tracing::info!(
                unpark_from_ap_position = ?park,
                seeded_ra_ticks = ra_ticks,
                seeded_dec_ticks = dec_ticks,
                "UnparkFromApPosition reset firmware encoder to the named AP park"
            );
        }
        self.state.write().await.at_park = false;
        Ok(ap_park_str(park).to_string())
    }
}
