//! Pegasus Scops OAG focuser device implementation.
//!
//! Implements the ASCOM Alpaca `Device` + `Focuser` traits. Connection state is
//! the device's `Session<ScopsCodec>` slot — when it's `Some`, we hold a live
//! handle to the shared transport; when it's `None`, we don't. The session
//! existing **is** the "Connected" state.

use std::sync::Arc;

use ascom_alpaca::api::{Device, Focuser};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use rusty_photon_driver::ConfigActionCtx;
use rusty_photon_shared_transport::Session;
use tokio::sync::RwLock;
use tracing::debug;

use crate::codec::ScopsCodec;
use crate::config::FocuserConfig;
use crate::config_actions::ScopsFocuserDriver;
use crate::error::ScopsOagError;
use crate::manager::FocuserManager;

/// Guard macro that returns NOT_CONNECTED if the device is not connected.
macro_rules! ensure_connected {
    ($self:ident) => {
        if !$self.connected().await.is_ok_and(|connected| connected) {
            debug!("Focuser device not connected");
            return Err(ASCOMError::NOT_CONNECTED);
        }
    };
}

/// Pegasus Scops OAG focuser device for ASCOM Alpaca.
#[derive(derive_more::Debug)]
pub struct ScopsFocuserDevice {
    config: FocuserConfig,
    /// `Some` between successful connect and explicit disconnect. The session
    /// existing is the truth — no second-source bool to desync.
    #[debug(skip)]
    session: Arc<RwLock<Option<Session<ScopsCodec>>>>,
    #[debug(skip)]
    manager: Arc<FocuserManager>,
    /// `Some` when built through `ServerBuilder` with a config source (the
    /// normal path); `None` for focused unit-test devices.
    #[debug(skip)]
    config_ctx: Option<ConfigActionCtx<ScopsFocuserDriver>>,
}

impl ScopsFocuserDevice {
    pub fn new(config: FocuserConfig, manager: Arc<FocuserManager>) -> Self {
        Self {
            config,
            session: Arc::new(RwLock::new(None)),
            manager,
            config_ctx: None,
        }
    }

    /// Attach the config-action context, enabling `config.get` / `config.apply`
    /// / `config.schema` on this device.
    pub fn with_config_actions(mut self, ctx: ConfigActionCtx<ScopsFocuserDriver>) -> Self {
        self.config_ctx = Some(ctx);
        self
    }
}

#[async_trait]
impl Device for ScopsFocuserDevice {
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
        // The write lock spans the whole check-and-modify so two concurrent
        // `Connected=true` requests can't both observe `None` and both call
        // `acquire()` (issue #250).
        let mut slot = self.session.write().await;
        match (connected, slot.is_some()) {
            (true, false) => {
                let session = self
                    .manager
                    .transport()
                    .acquire()
                    .await
                    .map_err(ScopsOagError::from)?;
                *slot = Some(session);
                debug!("Focuser device connected");
            }
            (false, true) => {
                if let Some(session) = slot.take() {
                    session.close().await.map_err(ScopsOagError::from)?;
                }
                debug!("Focuser device disconnected");
            }
            _ => {}
        }
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok(
            "Pegasus Scops OAG Focuser - ASCOM Alpaca driver for the Pegasus Astro Scops OAG"
                .to_string(),
        )
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    async fn supported_actions(&self) -> ASCOMResult<Vec<String>> {
        Ok(rusty_photon_driver::supported_actions(&self.config_ctx))
    }

    async fn action(&self, action: String, parameters: String) -> ASCOMResult<String> {
        rusty_photon_driver::dispatch::<ScopsFocuserDriver>(&self.config_ctx, action, parameters)
            .await
    }
}

#[async_trait]
impl Focuser for ScopsFocuserDevice {
    async fn absolute(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn is_moving(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);

        // Actively refresh status so move-completion is observable without
        // waiting up to one polling interval.
        let cached = self.manager.get_cached_state().await;
        if cached.is_moving {
            let guard = self.session.read().await;
            let session = guard.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;
            self.manager.refresh_status(session).await?;
        }
        Ok(self.manager.get_cached_state().await.is_moving)
    }

    async fn max_increment(&self) -> ASCOMResult<u32> {
        Ok(self.config.max_step)
    }

    async fn max_step(&self) -> ASCOMResult<u32> {
        Ok(self.config.max_step)
    }

    async fn position(&self) -> ASCOMResult<i32> {
        ensure_connected!(self);
        let state = self.manager.get_cached_state().await;
        let position = state.position.ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::INVALID_OPERATION,
                "Position not yet available",
            )
        })?;
        Ok(position as i32)
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
        // The Scops OAG has no temperature probe — see docs/services/pa-scops-oag.md.
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn halt(&self) -> ASCOMResult<()> {
        ensure_connected!(self);
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;
        self.manager.abort(session).await?;
        Ok(())
    }

    async fn move_(&self, position: i32) -> ASCOMResult<()> {
        ensure_connected!(self);

        if position < 0 || position > self.config.max_step as i32 {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!(
                    "Position {} out of range [0, {}]",
                    position, self.config.max_step
                ),
            ));
        }

        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;
        self.manager.move_absolute(session, position as i64).await?;
        Ok(())
    }
}
