//! PPBA Switch device implementation
//!
//! This module implements the ASCOM Alpaca Device and Switch traits
//! for the Pegasus Astro Pocket Powerbox Advance Gen2.

use std::fmt;
use std::sync::Arc;

use ascom_alpaca::api::{Device, Switch};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::SwitchConfig;
use crate::error::{PpbaError, Result};
use crate::protocol::PpbaCommand;
use crate::serial_manager::SerialManager;
use crate::switches::{SwitchId, MAX_SWITCH};

/// Guard macro that returns NOT_CONNECTED if the device is not connected.
macro_rules! ensure_connected {
    ($self:ident) => {
        if !$self.connected().await.is_ok_and(|connected| connected) {
            debug!("Switch device not connected");
            return Err(ASCOMError::NOT_CONNECTED);
        }
    };
}

/// PPBA Switch device for ASCOM Alpaca
pub struct PpbaSwitchDevice {
    config: SwitchConfig,
    requested_connection: Arc<RwLock<bool>>,
    serial_manager: Arc<SerialManager>,
}

impl fmt::Debug for PpbaSwitchDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PpbaSwitchDevice")
            .field("config", &self.config)
            .field("requested_connection", &self.requested_connection)
            .finish_non_exhaustive()
    }
}

impl PpbaSwitchDevice {
    /// Create a new PPBA switch device
    pub fn new(config: SwitchConfig, serial_manager: Arc<SerialManager>) -> Self {
        Self {
            config,
            requested_connection: Arc::new(RwLock::new(false)),
            serial_manager,
        }
    }

    /// Get the current switch value for a given switch ID
    async fn get_switch_value_internal(&self, id: usize) -> Result<f64> {
        let switch_id = SwitchId::from_id(id).ok_or(PpbaError::InvalidSwitchId(id))?;
        let cached = self.serial_manager.get_cached_state().await;

        match switch_id {
            // Controllable switches
            SwitchId::Quad12V => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(if status.quad_12v { 1.0 } else { 0.0 })
            }
            SwitchId::AdjustableOutput => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(if status.adjustable_output { 1.0 } else { 0.0 })
            }
            SwitchId::DewHeaterA => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(status.dew_a as f64)
            }
            SwitchId::DewHeaterB => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(status.dew_b as f64)
            }
            SwitchId::UsbHub => Ok(if cached.usb_hub_enabled { 1.0 } else { 0.0 }),
            SwitchId::AutoDew => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(if status.auto_dew { 1.0 } else { 0.0 })
            }

            // Read-only switches - Power Statistics
            SwitchId::AverageCurrent => {
                let stats = cached.power_stats.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(stats.average_amps)
            }
            SwitchId::AmpHours => {
                let stats = cached.power_stats.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(stats.amp_hours)
            }
            SwitchId::WattHours => {
                let stats = cached.power_stats.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(stats.watt_hours)
            }
            SwitchId::Uptime => {
                let stats = cached.power_stats.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(stats.uptime_hours())
            }

            // Read-only switches - Sensor Data
            SwitchId::InputVoltage => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(status.voltage)
            }
            SwitchId::TotalCurrent => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(status.current)
            }
            SwitchId::Temperature => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(status.temperature)
            }
            SwitchId::Humidity => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(status.humidity)
            }
            SwitchId::Dewpoint => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(status.dewpoint)
            }
            SwitchId::PowerWarning => {
                let status = cached.status.as_ref().ok_or(PpbaError::NotConnected)?;
                Ok(if status.power_warning { 1.0 } else { 0.0 })
            }
        }
    }

    /// Set a switch value
    async fn set_switch_value_internal(&self, id: usize, value: f64) -> Result<()> {
        let switch_id = SwitchId::from_id(id).ok_or(PpbaError::InvalidSwitchId(id))?;
        let info = switch_id.info();

        if !info.can_write {
            return Err(PpbaError::SwitchNotWritable(id));
        }

        // Additional check for dew heaters: verify auto-dew is OFF
        // Refresh state first to ensure we have current auto-dew status
        if matches!(switch_id, SwitchId::DewHeaterA | SwitchId::DewHeaterB) {
            // Refresh status to get current auto-dew state from device
            self.serial_manager.refresh_status().await?;

            let cached = self.serial_manager.get_cached_state().await;
            if let Some(status) = &cached.status {
                if status.auto_dew {
                    return Err(PpbaError::AutoDewEnabled(id));
                }
            }
        }

        // Validate value range
        if value < info.min_value || value > info.max_value {
            return Err(PpbaError::InvalidValue(format!(
                "Value {} out of range [{}, {}] for switch {}",
                value, info.min_value, info.max_value, info.name
            )));
        }

        let command = match switch_id {
            SwitchId::Quad12V => PpbaCommand::SetQuad12V(value >= 0.5),
            SwitchId::AdjustableOutput => PpbaCommand::SetAdjustable(value >= 0.5),
            SwitchId::DewHeaterA => PpbaCommand::SetDewA(value.round() as u8),
            SwitchId::DewHeaterB => PpbaCommand::SetDewB(value.round() as u8),
            SwitchId::UsbHub => {
                // USB hub state is not included in PA status response,
                // so we need to track it manually
                let enabled = value >= 0.5;
                self.serial_manager
                    .send_command(PpbaCommand::SetUsbHub(enabled))
                    .await?;
                self.serial_manager.set_usb_hub_state(enabled).await;
                return Ok(());
            }
            SwitchId::AutoDew => PpbaCommand::SetAutoDew(value >= 0.5),
            _ => return Err(PpbaError::SwitchNotWritable(id)),
        };

        self.serial_manager.send_command(command).await?;

        // Refresh status to get updated values
        self.serial_manager.refresh_status().await?;

        Ok(())
    }

    /// Convert internal error to ASCOM error
    fn to_ascom_error(err: PpbaError) -> ASCOMError {
        match err {
            PpbaError::NotConnected => {
                ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, err.to_string())
            }
            PpbaError::InvalidSwitchId(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, err.to_string())
            }
            PpbaError::SwitchNotWritable(_) => {
                ASCOMError::new(ASCOMErrorCode::NOT_IMPLEMENTED, err.to_string())
            }
            PpbaError::AutoDewEnabled(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_OPERATION, err.to_string())
            }
            PpbaError::InvalidValue(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, err.to_string())
            }
            _ => ASCOMError::invalid_operation(err.to_string()),
        }
    }
}

#[async_trait]
impl Device for PpbaSwitchDevice {
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
                debug!("Switch device connected");
            }
            false => {
                *self.requested_connection.write().await = false;
                self.serial_manager.disconnect().await;
                debug!("Switch device disconnected");
            }
        }
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok(
            "PPBA Driver - Switch interface for Pegasus Astro Pocket Powerbox Advance Gen2"
                .to_string(),
        )
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl Switch for PpbaSwitchDevice {
    async fn max_switch(&self) -> ASCOMResult<usize> {
        Ok(MAX_SWITCH)
    }

    async fn can_write(&self, id: usize) -> ASCOMResult<bool> {
        ensure_connected!(self);

        // Validate switch ID
        let switch_id = SwitchId::from_id(id)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;

        // For dew heaters, writability depends on auto-dew state
        if matches!(switch_id, SwitchId::DewHeaterA | SwitchId::DewHeaterB) {
            // Check if cache is populated, refresh if not
            let cached = self.serial_manager.get_cached_state().await;
            if cached.status.is_none() {
                // Cache not populated yet, refresh it
                self.serial_manager
                    .refresh_status()
                    .await
                    .map_err(Self::to_ascom_error)?;

                // Get updated cache
                let cached = self.serial_manager.get_cached_state().await;
                if let Some(status) = &cached.status {
                    // Writable only when auto-dew is OFF
                    return Ok(!status.auto_dew);
                }
            } else if let Some(status) = &cached.status {
                // Writable only when auto-dew is OFF
                return Ok(!status.auto_dew);
            }
        }

        // All other switches use static can_write from their info
        Ok(switch_id.info().can_write)
    }

    async fn get_switch(&self, id: usize) -> ASCOMResult<bool> {
        ensure_connected!(self);

        let value = self
            .get_switch_value_internal(id)
            .await
            .map_err(Self::to_ascom_error)?;

        // Per ASCOM spec: False at minimum value, True above minimum
        let switch_id = SwitchId::from_id(id)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(value > switch_id.info().min_value)
    }

    async fn set_switch(&self, id: usize, state: bool) -> ASCOMResult<()> {
        ensure_connected!(self);

        let switch_id = SwitchId::from_id(id)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        let info = switch_id.info();

        // Per ASCOM spec: True sets to max, False sets to min
        let value = if state {
            info.max_value
        } else {
            info.min_value
        };

        self.set_switch_value_internal(id, value)
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn get_switch_description(&self, id: usize) -> ASCOMResult<String> {
        ensure_connected!(self);
        let switch_id = SwitchId::from_id(id)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(switch_id.info().description.to_string())
    }

    async fn get_switch_name(&self, id: usize) -> ASCOMResult<String> {
        ensure_connected!(self);
        let switch_id = SwitchId::from_id(id)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(switch_id.info().name.to_string())
    }

    async fn set_switch_name(&self, _id: usize, _name: String) -> ASCOMResult<()> {
        Err(ASCOMError::new(
            ASCOMErrorCode::NOT_IMPLEMENTED,
            "Setting switch names is not supported",
        ))
    }

    async fn get_switch_value(&self, id: usize) -> ASCOMResult<f64> {
        ensure_connected!(self);

        self.get_switch_value_internal(id)
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn set_switch_value(&self, id: usize, value: f64) -> ASCOMResult<()> {
        ensure_connected!(self);

        self.set_switch_value_internal(id, value)
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn min_switch_value(&self, id: usize) -> ASCOMResult<f64> {
        ensure_connected!(self);
        let switch_id = SwitchId::from_id(id)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(switch_id.info().min_value)
    }

    async fn max_switch_value(&self, id: usize) -> ASCOMResult<f64> {
        ensure_connected!(self);
        let switch_id = SwitchId::from_id(id)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(switch_id.info().max_value)
    }

    async fn switch_step(&self, id: usize) -> ASCOMResult<f64> {
        ensure_connected!(self);
        let switch_id = SwitchId::from_id(id)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(switch_id.info().step)
    }

    async fn can_async(&self, id: usize) -> ASCOMResult<bool> {
        ensure_connected!(self);
        // Validate switch ID
        if id >= MAX_SWITCH {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Invalid switch ID: {}", id),
            ));
        }
        // We don't support async operations
        Ok(false)
    }

    async fn state_change_complete(&self, id: usize) -> ASCOMResult<bool> {
        ensure_connected!(self);
        // Validate switch ID
        if id >= MAX_SWITCH {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Invalid switch ID: {}", id),
            ));
        }
        // We don't support async operations, so state changes are always complete
        Ok(true)
    }

    async fn cancel_async(&self, id: usize) -> ASCOMResult<()> {
        ensure_connected!(self);
        // Validate switch ID first
        if id >= MAX_SWITCH {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Invalid switch ID: {}", id),
            ));
        }

        // We don't support async operations, so there's nothing to cancel.
        // Per ASCOM spec, this should return OK if there's no operation in progress.
        Ok(())
    }

    async fn set_async(&self, id: usize, state: bool) -> ASCOMResult<()> {
        ensure_connected!(self);
        // Validate switch ID first
        if id >= MAX_SWITCH {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Invalid switch ID: {}", id),
            ));
        }

        // Per ASCOM spec: SetAsync should work even if CanAsync returns false,
        // it just completes immediately. We delegate to the synchronous method.
        self.set_switch(id, state).await
    }

    async fn set_async_value(&self, id: usize, value: f64) -> ASCOMResult<()> {
        ensure_connected!(self);
        // Validate switch ID first
        if id >= MAX_SWITCH {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Invalid switch ID: {}", id),
            ));
        }

        // Per ASCOM spec: SetAsyncValue should work even if CanAsync returns false,
        // it just completes immediately. We delegate to the synchronous method.
        self.set_switch_value(id, value).await
    }
}
