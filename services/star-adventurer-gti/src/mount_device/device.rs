//! ASCOM `IDevice` trait implementation for [`MountDevice`].
//!
//! Mostly trivial getters that pass through to [`MountConfig`]; the
//! interesting method is `set_connected` which drives the
//! `TransportManager` ref-count and, on a 0→1 transition, runs the
//! post-connect hooks (`seed_home_pose_after_connect` then
//! `load_park_target_after_connect`) with rollback-on-error.

use ascom_alpaca::api::Device;
use ascom_alpaca::ASCOMResult;
use async_trait::async_trait;
use tracing::debug;

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
        let requested = *self.requested_connection.read().await;
        Ok(requested && self.transport.is_available())
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        let mut req = self.requested_connection.write().await;
        if *req == connected {
            return Ok(());
        }
        if connected {
            self.transport.connect().await.map_err(Self::ascom)?;
            // Post-connect work that can fail (config-file read, parameter
            // cache lookup, encoder seed) runs in functions that the
            // caller can roll back on any error — otherwise the transport
            // ref-count would stay incremented while `*req` remained
            // false, leaking a connection. Per the Copilot review on
            // PR #221 (comment 3238682044).
            //
            // Order matters: `seed_home_pose_after_connect` runs FIRST so
            // the snapshot reflects the home_pose's logical encoder values
            // before `load_park_target_after_connect` picks its default
            // park target from the snapshot. Otherwise the handshake's
            // pre-seed reading (firmware-zero on a fresh power-up) would
            // become the park fallback and `Park` would drive the mount
            // to mech_HA = 0h / mech_dec = 0° instead of the home pose.
            if let Err(e) = self.seed_home_pose_after_connect().await {
                if let Err(disc_err) = self.transport.disconnect().await {
                    tracing::warn!("disconnect during set_connected rollback failed: {disc_err}");
                }
                return Err(e);
            }
            if let Err(e) = self.load_park_target_after_connect().await {
                if let Err(disc_err) = self.transport.disconnect().await {
                    tracing::warn!("disconnect during set_connected rollback failed: {disc_err}");
                }
                return Err(e);
            }
            *req = true;
        } else {
            self.transport.disconnect().await.map_err(Self::ascom)?;
            *req = false;
            // Disconnect resets the per-session client state but leaves
            // mechanical state (`at_park`) intact — the mount's encoder
            // doesn't move just because we closed the socket. See
            // [`super::DriverState::reset_for_disconnect`] for the field-
            // by-field rationale.
            self.state.write().await.reset_for_disconnect();
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
}
