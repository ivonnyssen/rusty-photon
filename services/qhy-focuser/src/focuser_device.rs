//! QHY Q-Focuser device implementation
//!
//! Implements the ASCOM Alpaca Device and Focuser traits for the QHY Q-Focuser.

use std::fmt;
use std::sync::Arc;

use ascom_alpaca::api::{Device, Focuser};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::FocuserConfig;
use crate::error::QhyFocuserError;
use crate::serial_manager::SerialManager;

/// Guard macro that returns NOT_CONNECTED if the device is not connected.
macro_rules! ensure_connected {
    ($self:ident) => {
        if !$self.connected().await.is_ok_and(|connected| connected) {
            debug!("Focuser device not connected");
            return Err(ASCOMError::NOT_CONNECTED);
        }
    };
}

/// QHY Q-Focuser device for ASCOM Alpaca
pub struct QhyFocuserDevice {
    config: FocuserConfig,
    requested_connection: Arc<RwLock<bool>>,
    serial_manager: Arc<SerialManager>,
}

impl fmt::Debug for QhyFocuserDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QhyFocuserDevice")
            .field("config", &self.config)
            .field("requested_connection", &self.requested_connection)
            .finish_non_exhaustive()
    }
}

impl QhyFocuserDevice {
    /// Create a new QHY Q-Focuser device
    pub fn new(config: FocuserConfig, serial_manager: Arc<SerialManager>) -> Self {
        Self {
            config,
            requested_connection: Arc::new(RwLock::new(false)),
            serial_manager,
        }
    }

    /// Convert internal error to ASCOM error
    fn to_ascom_error(err: QhyFocuserError) -> ASCOMError {
        err.to_ascom_error()
    }
}

#[async_trait]
impl Device for QhyFocuserDevice {
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
        let serial_ok = self.serial_manager.is_available();
        Ok(requested && serial_ok)
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        if self.connected().await? == connected {
            return Ok(());
        }
        match connected {
            true => {
                self.serial_manager
                    .connect()
                    .await
                    .map_err(Self::to_ascom_error)?;
                *self.requested_connection.write().await = true;
                debug!("Focuser device connected");
            }
            false => {
                *self.requested_connection.write().await = false;
                self.serial_manager.disconnect().await;
                debug!("Focuser device disconnected");
            }
        }
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("QHY Q-Focuser Driver - ASCOM Alpaca interface for QHY EAF focuser".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl Focuser for QhyFocuserDevice {
    async fn absolute(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn is_moving(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);

        // Actively refresh position to detect move completion
        // rather than relying solely on background polling
        let state = self.serial_manager.get_cached_state().await;
        if state.is_moving {
            self.serial_manager
                .refresh_position()
                .await
                .map_err(Self::to_ascom_error)?;
        }

        let state = self.serial_manager.get_cached_state().await;
        Ok(state.is_moving)
    }

    async fn max_increment(&self) -> ASCOMResult<u32> {
        Ok(self.config.max_step)
    }

    async fn max_step(&self) -> ASCOMResult<u32> {
        Ok(self.config.max_step)
    }

    async fn position(&self) -> ASCOMResult<i32> {
        ensure_connected!(self);
        let state = self.serial_manager.get_cached_state().await;
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
        ensure_connected!(self);
        let state = self.serial_manager.get_cached_state().await;
        state.outer_temp.ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::INVALID_OPERATION,
                "Temperature not yet available",
            )
        })
    }

    async fn halt(&self) -> ASCOMResult<()> {
        ensure_connected!(self);
        self.serial_manager
            .abort()
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn move_(&self, position: i32) -> ASCOMResult<()> {
        ensure_connected!(self);

        // Validate range
        if position < 0 || position > self.config.max_step as i32 {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!(
                    "Position {} out of range [0, {}]",
                    position, self.config.max_step
                ),
            ));
        }

        self.serial_manager
            .move_absolute(position as i64)
            .await
            .map_err(Self::to_ascom_error)
    }
}
