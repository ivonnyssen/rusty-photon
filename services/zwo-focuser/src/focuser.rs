//! `ZwoFocuser` — the ASCOM `Device` + `Focuser` implementation over the
//! [`FocuserHandle`](crate::backend::FocuserHandle) seam.
//!
//! Behaviour follows `docs/services/zwo-focuser.md`: `Absolute` is always
//! `true` and `move_` is absolute-only (no relative-move prior art exists
//! anywhere in this codebase); `TempComp`/`TempCompAvailable`/`SetTempComp`
//! stay stubbed, matching `qhy-focuser`/`pa-scops-oag`; `Temperature` returns
//! the live `EAFGetTemp` reading (the EAF has a sensor, unlike the Scops OAG).
//!
//! Every blocking SDK call runs on `spawn_blocking`, mirroring `zwo-camera`'s
//! `ZwoCamera` — the EAF SDK is blocking C FFI, so calling it directly from an
//! async handler could stall other Alpaca requests.

use std::sync::Arc;

use ascom_alpaca::api::{Device, Focuser};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};

use crate::backend::{BackendError, FocuserHandle};
use crate::config::DeviceOverride;
use crate::config_actions::ZwoFocuserDriver;
use rusty_photon_driver::ConfigActionCtx;

/// Map a [`BackendError`] to the generic ASCOM error for an SDK-call failure.
/// Call sites that need a more specific code (e.g. `NOT_CONNECTED` on open
/// failure) map the error themselves instead of going through this helper.
fn sdk_err(e: BackendError) -> ASCOMError {
    ASCOMError::invalid_operation(e.0)
}

/// One ASCOM Focuser device per discovered EAF.
#[derive(Clone, derive_more::Debug)]
pub struct ZwoFocuser {
    #[debug(skip)]
    handle: Arc<dyn FocuserHandle>,
    /// The working travel limit (`EAFGetMaxStep`) — what `MaxStep`/
    /// `MaxIncrement` report and `Move` validates against. The firmware stops
    /// at this limit, so the `EAF_INFO::MaxStep` ceiling must not be used for
    /// range checks.
    max_step: u32,
    unique_id: String,
    name: String,
    description: String,
    #[debug(skip)]
    config_ctx: Option<ConfigActionCtx<ZwoFocuserDriver>>,
}

impl ZwoFocuser {
    /// Build a device from an SDK handle and an optional per-serial config
    /// override. The ASCOM `UniqueID` is the handle's serial-derived id;
    /// `name`/`description` fall back to SDK-derived defaults.
    pub fn new(handle: Arc<dyn FocuserHandle>, overrides: Option<&DeviceOverride>) -> Self {
        let info = handle.info();
        let max_step = handle.max_step();
        let unique_id = handle.unique_id();
        let name = overrides
            .and_then(|o| o.name.clone())
            .unwrap_or_else(|| info.name.clone());
        let description = overrides
            .and_then(|o| o.description.clone())
            .unwrap_or_else(|| format!("ZWO EAF focuser ({})", info.name));
        Self {
            handle,
            max_step,
            unique_id,
            name,
            description,
            config_ctx: None,
        }
    }

    /// Attach config-action wiring (enables `config.get`/`apply`/`schema`).
    #[must_use]
    pub fn with_config_actions(mut self, ctx: ConfigActionCtx<ZwoFocuserDriver>) -> Self {
        self.config_ctx = Some(ctx);
        self
    }

    fn ensure_connected(&self) -> ASCOMResult<()> {
        if self.handle.is_open() {
            Ok(())
        } else {
            Err(ASCOMError::NOT_CONNECTED)
        }
    }

    fn connect(&self) -> ASCOMResult<()> {
        self.handle.open().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        tracing::debug!(focuser = %self.unique_id, "focuser connected");
        Ok(())
    }

    fn disconnect(&self) -> ASCOMResult<()> {
        self.handle.close().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        tracing::debug!(focuser = %self.unique_id, "focuser disconnected");
        Ok(())
    }

    /// Run a blocking SDK-seam call off the async executor. The EAF FFI calls
    /// do USB I/O, so running them directly on a Tokio worker could stall
    /// other Alpaca requests; offload them like the connect path.
    async fn on_handle<T, F>(&self, f: F) -> ASCOMResult<T>
    where
        F: FnOnce(&dyn FocuserHandle) -> ASCOMResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let handle = Arc::clone(&self.handle);
        tokio::task::spawn_blocking(move || f(handle.as_ref()))
            .await
            .map_err(|e| ASCOMError::invalid_operation(format!("SDK task failed: {e}")))?
    }
}

#[async_trait::async_trait]
impl Device for ZwoFocuser {
    fn static_name(&self) -> &str {
        &self.name
    }

    fn unique_id(&self) -> &str {
        &self.unique_id
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        Ok(self.handle.is_open())
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        if self.handle.is_open() == connected {
            return Ok(());
        }
        // `connect`/`disconnect` do blocking SDK I/O (`EAFOpen`/`EAFClose`), so
        // offload them off the executor (ZwoFocuser is cheap to clone: it is
        // `Arc`-backed).
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            if connected {
                this.connect()
            } else {
                this.disconnect()
            }
        })
        .await
        .map_err(|e| ASCOMError::invalid_operation(format!("connect task failed: {e}")))?
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.description.clone())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("rusty-photon zwo-focuser".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    async fn supported_actions(&self) -> ASCOMResult<Vec<String>> {
        Ok(rusty_photon_driver::supported_actions(&self.config_ctx))
    }

    async fn action(&self, action: String, parameters: String) -> ASCOMResult<String> {
        rusty_photon_driver::dispatch::<ZwoFocuserDriver>(&self.config_ctx, action, parameters)
            .await
    }
}

#[async_trait::async_trait]
impl Focuser for ZwoFocuser {
    async fn absolute(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn is_moving(&self) -> ASCOMResult<bool> {
        self.ensure_connected()?;
        self.on_handle(|h| h.is_moving().map_err(sdk_err)).await
    }

    async fn max_increment(&self) -> ASCOMResult<u32> {
        Ok(self.max_step)
    }

    async fn max_step(&self) -> ASCOMResult<u32> {
        Ok(self.max_step)
    }

    async fn position(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        self.on_handle(|h| h.position().map_err(sdk_err)).await
    }

    async fn step_size(&self) -> ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn temp_comp(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn set_temp_comp(&self, _temp_comp: bool) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn temp_comp_available(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn temperature(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        let temp = self.on_handle(|h| h.temperature().map_err(sdk_err)).await?;
        Ok(f64::from(temp))
    }

    async fn halt(&self) -> ASCOMResult<()> {
        self.ensure_connected()?;
        self.on_handle(|h| h.stop().map_err(sdk_err)).await
    }

    async fn move_(&self, position: i32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if position < 0 || u32::try_from(position).unwrap_or(u32::MAX) > self.max_step {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Position {} out of range [0, {}]", position, self.max_step),
            ));
        }
        self.on_handle(move |h| h.move_to(position).map_err(sdk_err))
            .await
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::backend::mock::MockFocuserHandle;
    use std::sync::atomic::Ordering;

    fn device(handle: MockFocuserHandle) -> ZwoFocuser {
        ZwoFocuser::new(Arc::new(handle), None)
    }

    #[tokio::test]
    async fn absolute_is_always_true() {
        assert!(device(MockFocuserHandle::default())
            .absolute()
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn max_step_and_max_increment_report_the_cached_info() {
        let d = device(MockFocuserHandle::default().with_max_step(1234));
        assert_eq!(d.max_step().await.unwrap(), 1234);
        assert_eq!(d.max_increment().await.unwrap(), 1234);
    }

    #[tokio::test]
    async fn operations_while_disconnected_are_rejected() {
        let d = device(MockFocuserHandle::default());
        assert_eq!(
            d.position().await.unwrap_err().code,
            ASCOMError::NOT_CONNECTED.code
        );
        assert_eq!(
            d.is_moving().await.unwrap_err().code,
            ASCOMError::NOT_CONNECTED.code
        );
        assert_eq!(
            d.halt().await.unwrap_err().code,
            ASCOMError::NOT_CONNECTED.code
        );
        assert_eq!(
            d.move_(0).await.unwrap_err().code,
            ASCOMError::NOT_CONNECTED.code
        );
    }

    #[tokio::test]
    async fn connect_move_and_position_round_trip() {
        let d = device(MockFocuserHandle::default());
        d.set_connected(true).await.unwrap();
        assert!(d.connected().await.unwrap());
        d.move_(500).await.unwrap();
        assert!(d.is_moving().await.unwrap());
        assert!(!d.is_moving().await.unwrap());
        assert_eq!(d.position().await.unwrap(), 500);
        d.set_connected(false).await.unwrap();
        assert!(!d.connected().await.unwrap());
    }

    #[tokio::test]
    async fn move_out_of_range_is_rejected_without_calling_the_sdk() {
        let d = device(MockFocuserHandle::default().with_max_step(1000));
        d.set_connected(true).await.unwrap();
        assert_eq!(
            d.move_(-1).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
        assert_eq!(
            d.move_(1001).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
        // No move was actually issued to the handle.
        assert_eq!(d.position().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn halt_stops_an_in_progress_move() {
        let d = device(MockFocuserHandle::default());
        d.set_connected(true).await.unwrap();
        d.move_(200).await.unwrap();
        d.halt().await.unwrap();
        assert!(!d.is_moving().await.unwrap());
    }

    #[tokio::test]
    async fn temperature_returns_the_live_reading() {
        let d = device(MockFocuserHandle::default());
        d.set_connected(true).await.unwrap();
        assert_eq!(d.temperature().await.unwrap(), 20.0);
    }

    #[tokio::test]
    async fn temperature_failure_is_surfaced() {
        let handle = MockFocuserHandle::default();
        handle.fail_temperature.store(true, Ordering::SeqCst);
        let d = device(handle);
        d.set_connected(true).await.unwrap();
        assert!(d.temperature().await.is_err());
    }

    #[tokio::test]
    async fn temp_comp_and_step_size_are_stubbed() {
        let d = device(MockFocuserHandle::default());
        assert!(!d.temp_comp().await.unwrap());
        assert!(!d.temp_comp_available().await.unwrap());
        assert_eq!(
            d.set_temp_comp(true).await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            d.step_size().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test]
    async fn name_and_description_use_config_overrides() {
        let overrides = DeviceOverride {
            name: Some("Main Focuser".to_string()),
            description: Some("On the Askar 60F".to_string()),
        };
        let d = ZwoFocuser::new(Arc::new(MockFocuserHandle::default()), Some(&overrides));
        assert_eq!(d.static_name(), "Main Focuser");
        assert_eq!(d.description().await.unwrap(), "On the Askar 60F");
    }
}
