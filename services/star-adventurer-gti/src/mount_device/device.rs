//! ASCOM `IDevice` trait implementation for [`MountDevice`].
//!
//! Mostly trivial getters that pass through to [`MountConfig`]; the
//! interesting method is `set_connected` which drives the shared
//! transport's session refcount and, on a 0→1 transition, runs the
//! post-acquire fallible hooks (`seed_after_connect` then
//! `load_park_target_after_connect`) with structural rollback via
//! `session.close().await` on any error.

use ascom_alpaca::api::Device;
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use tracing::debug;

use super::actions::{
    ACTION_SET_PREFERRED_AP_PARK, ACTION_SET_UNPARK_FROM_AP_POSITION,
    ACTION_UNPARK_FROM_AP_POSITION, SUPPORTED_ACTIONS,
};
use super::MountDevice;

#[async_trait]
impl Device for MountDevice {
    fn static_name(&self) -> &str {
        &self.config.name
    }

    fn unique_id(&self) -> &str {
        &self.config.unique_id
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.config.description.clone())
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        Ok(self.session.read().await.is_some() && self.manager.is_available())
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        // Holding the session write lock for the entire check-and-modify
        // ensures two concurrent `Connected=true` requests can't both
        // observe `None` and both call `acquire()`. The session slot
        // replacing the old `requested_connection` bool means the flag
        // and the resource are the same value — there is no second
        // source to desync from the shared transport's refcount.
        let mut slot = self.session.write().await;
        match (connected, slot.is_some()) {
            (true, false) => {
                let session = self
                    .manager
                    .transport()
                    .acquire()
                    .await
                    .map_err(Self::ascom_session_err)?;
                // Post-acquire fallible work. Order matters:
                // `seed_after_connect` runs FIRST so the snapshot
                // reflects the configured AP park's logical encoder
                // values before `load_park_target_after_connect`
                // resolves its park target. On any failure,
                // `session.close().await` synchronously closes —
                // propagating its result so the user sees a real error
                // instead of a swallowed warning (the pre-migration
                // "rollback-disconnect failed" log branch is gone).
                if let Err(e) = self.seed_after_connect(&session).await {
                    session.close().await.map_err(Self::ascom_transport_err)?;
                    return Err(e);
                }
                if let Err(e) = self.load_park_target_after_connect().await {
                    session.close().await.map_err(Self::ascom_transport_err)?;
                    return Err(e);
                }
                *slot = Some(session);
            }
            (false, true) => {
                if let Some(session) = slot.take() {
                    session.close().await.map_err(Self::ascom_transport_err)?;
                }
                // Disconnect resets the per-session client state but leaves
                // mechanical state (`at_park`) intact — the mount's encoder
                // doesn't move just because we closed the socket. See
                // [`super::DriverState::reset_for_disconnect`] for the field-
                // by-field rationale.
                self.state.write().await.reset_for_disconnect();
            }
            _ => {}
        }
        debug!(connected, "set_connected");
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("Star Adventurer GTi Driver - ASCOM Alpaca Telescope for Sky-Watcher GEM".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    /// The driver-specific vendor Actions. See [`super::actions`] and
    /// the design doc's
    /// [§Custom Actions for runtime control](../../../../docs/services/star-adventurer-gti.md#custom-actions-for-runtime-control).
    async fn supported_actions(&self) -> ASCOMResult<Vec<String>> {
        Ok(SUPPORTED_ACTIONS.iter().map(|s| (*s).to_string()).collect())
    }

    /// Dispatch a vendor `Action`. Each handler takes the single
    /// `parameters` string as an `ap_park_0..ap_park_5` token. An
    /// unrecognised name returns `ACTION_NOT_IMPLEMENTED` per the ASCOM
    /// `Action` contract.
    async fn action(&self, action: String, parameters: String) -> ASCOMResult<String> {
        match action.as_str() {
            ACTION_SET_UNPARK_FROM_AP_POSITION => {
                self.handle_set_unpark_from_ap_position(&parameters).await
            }
            ACTION_SET_PREFERRED_AP_PARK => self.handle_set_preferred_ap_park(&parameters).await,
            ACTION_UNPARK_FROM_AP_POSITION => {
                self.handle_unpark_from_ap_position(&parameters).await
            }
            other => Err(ASCOMError::new(
                ASCOMErrorCode::ACTION_NOT_IMPLEMENTED,
                format!("unknown action {other:?}"),
            )),
        }
    }
}
