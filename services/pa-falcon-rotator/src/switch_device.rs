//! Falcon Status Switch ASCOM device implementation
//!
//! Exposes two read-only switches alongside the main Rotator device:
//! - id `0`: input voltage (raw ADC count from `VS`)
//! - id `1`: limit-hit boolean from `FA.limit_detect`
//!
//! See the design doc's
//! [Status Switch Device](../../../docs/services/falcon-rotator.md#status-switch-device)
//! section for the contract.

use std::fmt;
use std::sync::Arc;

use ascom_alpaca::api::{Device, Switch};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::config::SwitchConfig;
use crate::error::FalconRotatorError;
use crate::serial_manager::SerialManager;

/// Number of switches advertised by this device. The design doc pins this at 2
/// (id 0 = voltage, id 1 = limit-hit); any other id is out of range.
const SWITCH_COUNT: usize = 2;

/// Reject switch ids outside `0..SWITCH_COUNT` with `INVALID_VALUE` per the
/// ASCOM convention: id-range validation precedes operation-permission
/// checks, so out-of-range ids never hit `INVALID_OPERATION` paths.
fn validate_id(id: usize) -> ASCOMResult<()> {
    if id >= SWITCH_COUNT {
        Err(ASCOMError::new(
            ASCOMErrorCode::INVALID_VALUE,
            format!("Switch id {id} out of range (valid: 0..{SWITCH_COUNT})"),
        ))
    } else {
        Ok(())
    }
}

/// Falcon Status Switch device for ASCOM Alpaca.
pub struct FalconStatusSwitchDevice {
    config: SwitchConfig,
    requested_connection: Arc<RwLock<bool>>,
    serial_manager: Arc<SerialManager>,
}

impl fmt::Debug for FalconStatusSwitchDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FalconStatusSwitchDevice")
            .field("config", &self.config)
            .field("requested_connection", &self.requested_connection)
            .finish_non_exhaustive()
    }
}

impl FalconStatusSwitchDevice {
    pub fn new(config: SwitchConfig, serial_manager: Arc<SerialManager>) -> Self {
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
impl Device for FalconStatusSwitchDevice {
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
        Ok("Pegasus Falcon Rotator Status Switch - ASCOM Alpaca interface".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl Switch for FalconStatusSwitchDevice {
    async fn max_switch(&self) -> ASCOMResult<usize> {
        Ok(SWITCH_COUNT)
    }

    async fn can_write(&self, id: usize) -> ASCOMResult<bool> {
        validate_id(id)?;
        Ok(false)
    }

    async fn get_switch_name(&self, _id: usize) -> ASCOMResult<String> {
        unimplemented!("FalconStatusSwitchDevice::get_switch_name is implemented in Phase 3e")
    }

    async fn get_switch_description(&self, _id: usize) -> ASCOMResult<String> {
        unimplemented!(
            "FalconStatusSwitchDevice::get_switch_description is implemented in Phase 3e"
        )
    }

    async fn get_switch(&self, _id: usize) -> ASCOMResult<bool> {
        unimplemented!("FalconStatusSwitchDevice::get_switch is implemented in Phase 3e")
    }

    async fn get_switch_value(&self, _id: usize) -> ASCOMResult<f64> {
        unimplemented!("FalconStatusSwitchDevice::get_switch_value is implemented in Phase 3e")
    }

    async fn min_switch_value(&self, _id: usize) -> ASCOMResult<f64> {
        unimplemented!("FalconStatusSwitchDevice::min_switch_value is implemented in Phase 3e")
    }

    async fn max_switch_value(&self, _id: usize) -> ASCOMResult<f64> {
        unimplemented!("FalconStatusSwitchDevice::max_switch_value is implemented in Phase 3e")
    }

    async fn switch_step(&self, _id: usize) -> ASCOMResult<f64> {
        unimplemented!("FalconStatusSwitchDevice::switch_step is implemented in Phase 3e")
    }

    async fn state_change_complete(&self, id: usize) -> ASCOMResult<bool> {
        validate_id(id)?;
        // Read-only switches never change asynchronously.
        Ok(true)
    }

    // Both advertised switches are read-only (`CanWrite = false`). The Switch
    // trait's defaults return `NOT_IMPLEMENTED`, but the design doc's Switch
    // layout section pins the contract to `INVALID_OPERATION` — overriding
    // the three write surfaces here so the wire-level error code matches.
    //
    // ASCOM convention: id-range validation happens first, so a write against
    // a bogus id returns `INVALID_VALUE` rather than the operation-rejection
    // code.

    async fn set_switch(&self, id: usize, _state: bool) -> ASCOMResult<()> {
        validate_id(id)?;
        Err(ASCOMError::new(
            ASCOMErrorCode::INVALID_OPERATION,
            "Falcon status switches are read-only",
        ))
    }

    async fn set_switch_value(&self, id: usize, _value: f64) -> ASCOMResult<()> {
        validate_id(id)?;
        Err(ASCOMError::new(
            ASCOMErrorCode::INVALID_OPERATION,
            "Falcon status switches are read-only",
        ))
    }

    async fn set_switch_name(&self, id: usize, _name: String) -> ASCOMResult<()> {
        validate_id(id)?;
        Err(ASCOMError::new(
            ASCOMErrorCode::INVALID_OPERATION,
            "Falcon status switch names are fixed",
        ))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn validate_id_accepts_zero() {
        validate_id(0).unwrap();
    }

    #[test]
    fn validate_id_accepts_one() {
        validate_id(1).unwrap();
    }

    #[test]
    fn validate_id_rejects_two() {
        let err = validate_id(2).unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
        assert!(
            err.message.contains("Switch id 2 out of range"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn validate_id_rejects_large_id() {
        let err = validate_id(usize::MAX).unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }
}
