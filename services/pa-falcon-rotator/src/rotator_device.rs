//! Falcon Rotator ASCOM device implementation
//!
//! Wraps `SerialManager` behind the ASCOM `Device` + `Rotator` traits. Every
//! property read maps to one serial command — see the design doc's
//! [Why no cache](../../../docs/services/falcon-rotator.md#why-no-cache)
//! section for the rationale.

use std::fmt;
use std::sync::Arc;

use ascom_alpaca::api::{Device, Rotator};
use ascom_alpaca::{ASCOMError, ASCOMResult};
use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::config::RotatorConfig;
use crate::error::FalconRotatorError;
use crate::serial_manager::SerialManager;

/// Falcon Rotator device for ASCOM Alpaca.
pub struct FalconRotatorDevice {
    config: RotatorConfig,
    requested_connection: Arc<RwLock<bool>>,
    serial_manager: Arc<SerialManager>,
}

impl fmt::Debug for FalconRotatorDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FalconRotatorDevice")
            .field("config", &self.config)
            .field("requested_connection", &self.requested_connection)
            .finish_non_exhaustive()
    }
}

impl FalconRotatorDevice {
    pub fn new(config: RotatorConfig, serial_manager: Arc<SerialManager>) -> Self {
        Self {
            config,
            requested_connection: Arc::new(RwLock::new(false)),
            serial_manager,
        }
    }

    fn to_ascom_error(err: FalconRotatorError) -> ASCOMError {
        err.to_ascom_error()
    }
}

#[async_trait]
impl Device for FalconRotatorDevice {
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
            }
            false => {
                *self.requested_connection.write().await = false;
                self.serial_manager.disconnect().await;
            }
        }
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("Pegasus Falcon Rotator Driver - ASCOM Alpaca interface".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl Rotator for FalconRotatorDevice {
    async fn can_reverse(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn is_moving(&self) -> ASCOMResult<bool> {
        unimplemented!("FalconRotatorDevice::is_moving is implemented in Phase 3d")
    }

    async fn position(&self) -> ASCOMResult<f64> {
        unimplemented!("FalconRotatorDevice::position is implemented in Phase 3d")
    }

    async fn mechanical_position(&self) -> ASCOMResult<f64> {
        unimplemented!("FalconRotatorDevice::mechanical_position is implemented in Phase 3d")
    }

    async fn target_position(&self) -> ASCOMResult<f64> {
        unimplemented!("FalconRotatorDevice::target_position is implemented in Phase 3d")
    }

    async fn reverse(&self) -> ASCOMResult<bool> {
        unimplemented!("FalconRotatorDevice::reverse is implemented in Phase 3d")
    }

    async fn set_reverse(&self, _reverse: bool) -> ASCOMResult<()> {
        unimplemented!("FalconRotatorDevice::set_reverse is implemented in Phase 3d")
    }

    async fn step_size(&self) -> ASCOMResult<f64> {
        // Vendor product page: 86.6 steps per degree → 1.0 / 86.6 ≈ 0.01155°.
        Ok(0.01155)
    }

    async fn halt(&self) -> ASCOMResult<()> {
        unimplemented!("FalconRotatorDevice::halt is implemented in Phase 3d")
    }

    async fn move_(&self, _position: f64) -> ASCOMResult<()> {
        unimplemented!("FalconRotatorDevice::move_ is implemented in Phase 3d")
    }

    async fn move_absolute(&self, _position: f64) -> ASCOMResult<()> {
        unimplemented!("FalconRotatorDevice::move_absolute is implemented in Phase 3d")
    }

    async fn move_mechanical(&self, _position: f64) -> ASCOMResult<()> {
        unimplemented!("FalconRotatorDevice::move_mechanical is implemented in Phase 3d")
    }

    async fn sync(&self, _position: f64) -> ASCOMResult<()> {
        unimplemented!("FalconRotatorDevice::sync is implemented in Phase 3d")
    }
}
