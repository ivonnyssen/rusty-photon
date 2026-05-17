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
use tracing::debug;

use crate::config::SwitchConfig;
use crate::error::FalconRotatorError;
use crate::serial_manager::SerialManager;

/// Number of switches advertised by this device. The design doc pins this at 2
/// (id 0 = voltage, id 1 = limit-hit); any other id is out of range.
const SWITCH_COUNT: usize = 2;

/// Id 0: input voltage (raw ADC count from the Falcon's `VS` command).
const SWITCH_ID_VOLTAGE: usize = 0;
/// Id 1: limit-hit flag (mirrors `FA.limit_detect`).
const SWITCH_ID_LIMIT: usize = 1;

/// Voltage-switch metadata pinned by the design doc's Switch layout table:
/// `MaxSwitchValue = 1023` assumes a 10-bit ADC on the Falcon's MCU; widening
/// it is a follow-up tracked in the design doc once hardware characterisation
/// is in hand.
const VOLTAGE_SWITCH_NAME: &str = "Input Voltage (raw)";
const VOLTAGE_SWITCH_DESCRIPTION: &str =
    "Raw input-voltage ADC count from the Falcon's VS command; scale not yet calibrated";
const VOLTAGE_MIN_VALUE: f64 = 0.0;
const VOLTAGE_MAX_VALUE: f64 = 1023.0;
const VOLTAGE_STEP: f64 = 1.0;

/// Limit-hit-switch metadata: boolean (0/1) mirror of `FA.limit_detect`.
const LIMIT_SWITCH_NAME: &str = "Limit Hit";
const LIMIT_SWITCH_DESCRIPTION: &str = "Mirrors FA.limit_detect for the most recent status read";
const LIMIT_MIN_VALUE: f64 = 0.0;
const LIMIT_MAX_VALUE: f64 = 1.0;
const LIMIT_STEP: f64 = 1.0;

/// Guard that returns `NOT_CONNECTED` if the device is not connected. Mirrors
/// `ppba-driver`'s `ensure_connected!` macro: every device-bound Switch method
/// runs this **before** id validation so a disconnected client always sees
/// `NOT_CONNECTED` (1031) regardless of the id passed in, matching the design
/// doc's [Error Model](../../../docs/services/falcon-rotator.md#error-model).
macro_rules! ensure_connected {
    ($self:ident) => {
        if !$self.connected().await.is_ok_and(|connected| connected) {
            debug!("Falcon Status Switch device not connected");
            return Err(ASCOMError::NOT_CONNECTED);
        }
    };
}

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
        // Hold the write lock across the whole check-and-modify so two
        // concurrent `Connected=true` requests for this switch don't both
        // increment the shared SerialManager refcount before either sets
        // the per-device flag. Same fix shape as `FalconRotatorDevice` —
        // see PR #241 round-5 review.
        let mut requested = self.requested_connection.write().await;
        let serial_ok = self.serial_manager.is_available();
        let already = *requested && serial_ok;
        if already == connected {
            return Ok(());
        }
        match connected {
            true => {
                self.serial_manager
                    .connect()
                    .await
                    .map_err(Self::to_ascom_error)?;
                *requested = true;
            }
            false => {
                *requested = false;
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
        ensure_connected!(self);
        validate_id(id)?;
        Ok(false)
    }

    async fn get_switch_name(&self, id: usize) -> ASCOMResult<String> {
        ensure_connected!(self);
        validate_id(id)?;
        let name = match id {
            SWITCH_ID_VOLTAGE => VOLTAGE_SWITCH_NAME,
            SWITCH_ID_LIMIT => LIMIT_SWITCH_NAME,
            _ => unreachable!("validate_id rejects ids >= SWITCH_COUNT"),
        };
        Ok(name.to_string())
    }

    async fn get_switch_description(&self, id: usize) -> ASCOMResult<String> {
        ensure_connected!(self);
        validate_id(id)?;
        let description = match id {
            SWITCH_ID_VOLTAGE => VOLTAGE_SWITCH_DESCRIPTION,
            SWITCH_ID_LIMIT => LIMIT_SWITCH_DESCRIPTION,
            _ => unreachable!("validate_id rejects ids >= SWITCH_COUNT"),
        };
        Ok(description.to_string())
    }

    async fn get_switch(&self, id: usize) -> ASCOMResult<bool> {
        ensure_connected!(self);
        validate_id(id)?;
        // ASCOM rule: GetSwitch returns false at MinSwitchValue, true otherwise.
        // For the voltage switch (Min = 0) that means "true iff raw > 0".
        // For the limit-hit switch (Min = 0, Max = 1) the 0.5 threshold is
        // the conventional midpoint test, matching the design doc's contract.
        let value = self.get_switch_value(id).await?;
        let threshold = match id {
            SWITCH_ID_VOLTAGE => VOLTAGE_MIN_VALUE,
            SWITCH_ID_LIMIT => 0.5,
            _ => unreachable!("validate_id rejects ids >= SWITCH_COUNT"),
        };
        Ok(value > threshold)
    }

    async fn get_switch_value(&self, id: usize) -> ASCOMResult<f64> {
        ensure_connected!(self);
        validate_id(id)?;
        match id {
            SWITCH_ID_VOLTAGE => self
                .serial_manager
                .read_voltage_raw()
                .await
                .map(f64::from)
                .map_err(Self::to_ascom_error),
            SWITCH_ID_LIMIT => self
                .serial_manager
                .read_status()
                .await
                .map(|s| if s.limit_detect { 1.0 } else { 0.0 })
                .map_err(Self::to_ascom_error),
            _ => unreachable!("validate_id rejects ids >= SWITCH_COUNT"),
        }
    }

    async fn min_switch_value(&self, id: usize) -> ASCOMResult<f64> {
        ensure_connected!(self);
        validate_id(id)?;
        Ok(match id {
            SWITCH_ID_VOLTAGE => VOLTAGE_MIN_VALUE,
            SWITCH_ID_LIMIT => LIMIT_MIN_VALUE,
            _ => unreachable!("validate_id rejects ids >= SWITCH_COUNT"),
        })
    }

    async fn max_switch_value(&self, id: usize) -> ASCOMResult<f64> {
        ensure_connected!(self);
        validate_id(id)?;
        Ok(match id {
            SWITCH_ID_VOLTAGE => VOLTAGE_MAX_VALUE,
            SWITCH_ID_LIMIT => LIMIT_MAX_VALUE,
            _ => unreachable!("validate_id rejects ids >= SWITCH_COUNT"),
        })
    }

    async fn switch_step(&self, id: usize) -> ASCOMResult<f64> {
        ensure_connected!(self);
        validate_id(id)?;
        Ok(match id {
            SWITCH_ID_VOLTAGE => VOLTAGE_STEP,
            SWITCH_ID_LIMIT => LIMIT_STEP,
            _ => unreachable!("validate_id rejects ids >= SWITCH_COUNT"),
        })
    }

    async fn state_change_complete(&self, id: usize) -> ASCOMResult<bool> {
        ensure_connected!(self);
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
        ensure_connected!(self);
        validate_id(id)?;
        Err(ASCOMError::new(
            ASCOMErrorCode::INVALID_OPERATION,
            "Falcon status switches are read-only",
        ))
    }

    async fn set_switch_value(&self, id: usize, _value: f64) -> ASCOMResult<()> {
        ensure_connected!(self);
        validate_id(id)?;
        Err(ASCOMError::new(
            ASCOMErrorCode::INVALID_OPERATION,
            "Falcon status switches are read-only",
        ))
    }

    async fn set_switch_name(&self, id: usize, _name: String) -> ASCOMResult<()> {
        ensure_connected!(self);
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
    use std::time::Duration;

    use async_trait::async_trait;

    use super::*;
    use crate::config::Config;
    use crate::io::{SerialPair, SerialPortFactory};

    /// Test-only no-op factory: never used at runtime (the connection-guard
    /// tests assert behaviour while disconnected, so `open` is never called).
    struct NoopFactory;

    #[async_trait]
    impl SerialPortFactory for NoopFactory {
        async fn open(
            &self,
            _port: &str,
            _baud_rate: u32,
            _timeout: Duration,
        ) -> crate::error::Result<SerialPair> {
            unimplemented!("test factory should never open a port")
        }

        async fn port_exists(&self, _port: &str) -> bool {
            true
        }
    }

    fn disconnected_device() -> FalconStatusSwitchDevice {
        let config = Config::default();
        let manager = Arc::new(SerialManager::new(config, Arc::new(NoopFactory)));
        FalconStatusSwitchDevice::new(SwitchConfig::default(), manager)
    }

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

    #[tokio::test]
    async fn can_write_returns_not_connected_when_disconnected() {
        let device = disconnected_device();
        let err = device.can_write(0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn state_change_complete_returns_not_connected_when_disconnected() {
        let device = disconnected_device();
        let err = device.state_change_complete(0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn set_switch_returns_not_connected_when_disconnected() {
        let device = disconnected_device();
        let err = device.set_switch(0, true).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn set_switch_value_returns_not_connected_when_disconnected() {
        let device = disconnected_device();
        let err = device.set_switch_value(0, 0.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn set_switch_name_returns_not_connected_when_disconnected() {
        let device = disconnected_device();
        let err = device
            .set_switch_name(0, "x".to_string())
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn connection_guard_precedes_id_validation() {
        // Disconnected + out-of-range id should report NOT_CONNECTED, not
        // INVALID_VALUE — the guard runs first per the design doc's error model.
        let device = disconnected_device();
        let err = device.set_switch_value(99, 0.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }
}

/// Connected-device tests for the seven Switch getters. Gated on
/// `feature = "mock"` so the rich `MockSerialPortFactory` can stand in for
/// the real Falcon — matching the qhy-focuser / serial_manager precedent.
#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod mock_tests {
    use super::*;
    use crate::config::Config;
    use crate::io::SerialPortFactory;
    use crate::mock::MockSerialPortFactory;

    async fn connected_device() -> (FalconStatusSwitchDevice, Arc<MockSerialPortFactory>) {
        let config = Config::default();
        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = Arc::new(SerialManager::new(
            config,
            Arc::clone(&factory) as Arc<dyn SerialPortFactory>,
        ));
        let device = FalconStatusSwitchDevice::new(SwitchConfig::default(), manager);
        device.set_connected(true).await.unwrap();
        (device, factory)
    }

    // ---- max_switch -----------------------------------------------------

    #[tokio::test]
    async fn max_switch_reports_two() {
        let (device, _) = connected_device().await;
        assert_eq!(device.max_switch().await.unwrap(), SWITCH_COUNT);
    }

    // ---- get_switch_name -----------------------------------------------

    #[tokio::test]
    async fn get_switch_name_id_0_is_input_voltage_raw() {
        let (device, _) = connected_device().await;
        let name = device.get_switch_name(SWITCH_ID_VOLTAGE).await.unwrap();
        assert_eq!(name, VOLTAGE_SWITCH_NAME);
    }

    #[tokio::test]
    async fn get_switch_name_id_1_is_limit_hit() {
        let (device, _) = connected_device().await;
        let name = device.get_switch_name(SWITCH_ID_LIMIT).await.unwrap();
        assert_eq!(name, LIMIT_SWITCH_NAME);
    }

    #[tokio::test]
    async fn get_switch_name_out_of_range_returns_invalid_value() {
        let (device, _) = connected_device().await;
        let err = device.get_switch_name(2).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    // ---- get_switch_description ----------------------------------------

    #[tokio::test]
    async fn get_switch_description_id_0_mentions_voltage() {
        let (device, _) = connected_device().await;
        let desc = device
            .get_switch_description(SWITCH_ID_VOLTAGE)
            .await
            .unwrap();
        assert!(
            desc.to_lowercase().contains("voltage"),
            "expected description to mention voltage, got: {desc}"
        );
    }

    #[tokio::test]
    async fn get_switch_description_id_1_mentions_limit() {
        let (device, _) = connected_device().await;
        let desc = device
            .get_switch_description(SWITCH_ID_LIMIT)
            .await
            .unwrap();
        assert!(
            desc.to_lowercase().contains("limit"),
            "expected description to mention limit, got: {desc}"
        );
    }

    // ---- min / max / step (pins the design-doc Switch layout table) ----

    #[tokio::test]
    async fn voltage_switch_range_is_zero_to_1023_step_1() {
        let (device, _) = connected_device().await;
        assert_eq!(
            device.min_switch_value(SWITCH_ID_VOLTAGE).await.unwrap(),
            0.0
        );
        assert_eq!(
            device.max_switch_value(SWITCH_ID_VOLTAGE).await.unwrap(),
            1023.0
        );
        assert_eq!(device.switch_step(SWITCH_ID_VOLTAGE).await.unwrap(), 1.0);
    }

    #[tokio::test]
    async fn limit_switch_range_is_zero_to_one_step_1() {
        let (device, _) = connected_device().await;
        assert_eq!(device.min_switch_value(SWITCH_ID_LIMIT).await.unwrap(), 0.0);
        assert_eq!(device.max_switch_value(SWITCH_ID_LIMIT).await.unwrap(), 1.0);
        assert_eq!(device.switch_step(SWITCH_ID_LIMIT).await.unwrap(), 1.0);
    }

    #[tokio::test]
    async fn metadata_getters_reject_out_of_range_id() {
        let (device, _) = connected_device().await;
        for err in [
            device.min_switch_value(2).await.unwrap_err(),
            device.max_switch_value(2).await.unwrap_err(),
            device.switch_step(2).await.unwrap_err(),
            device.get_switch_description(2).await.unwrap_err(),
        ] {
            assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
        }
    }

    // ---- get_switch_value ----------------------------------------------

    #[tokio::test]
    async fn get_switch_value_id_0_returns_raw_voltage() {
        let (device, factory) = connected_device().await;
        factory.set_voltage_raw(812).await;
        let value = device.get_switch_value(SWITCH_ID_VOLTAGE).await.unwrap();
        assert_eq!(value, 812.0);
    }

    #[tokio::test]
    async fn get_switch_value_id_1_is_one_when_limit_detected() {
        let (device, factory) = connected_device().await;
        factory.set_limit_detect(true).await;
        let value = device.get_switch_value(SWITCH_ID_LIMIT).await.unwrap();
        assert_eq!(value, 1.0);
    }

    #[tokio::test]
    async fn get_switch_value_id_1_is_zero_when_limit_clear() {
        let (device, _) = connected_device().await;
        // limit_detect defaults to false in the mock.
        let value = device.get_switch_value(SWITCH_ID_LIMIT).await.unwrap();
        assert_eq!(value, 0.0);
    }

    #[tokio::test]
    async fn get_switch_value_issues_vs_for_id_0() {
        let (device, factory) = connected_device().await;
        factory.clear_command_log().await;
        let _ = device.get_switch_value(SWITCH_ID_VOLTAGE).await.unwrap();
        let log = factory.command_log().await;
        assert!(
            log.iter().any(|c| c == "VS"),
            "expected VS on the wire, got: {log:?}"
        );
    }

    #[tokio::test]
    async fn get_switch_value_issues_fa_for_id_1() {
        let (device, factory) = connected_device().await;
        factory.clear_command_log().await;
        let _ = device.get_switch_value(SWITCH_ID_LIMIT).await.unwrap();
        let log = factory.command_log().await;
        assert!(
            log.iter().any(|c| c == "FA"),
            "expected FA on the wire, got: {log:?}"
        );
    }

    // ---- get_switch (boolean projection of get_switch_value) -----------

    #[tokio::test]
    async fn get_switch_id_0_is_true_when_raw_above_zero() {
        let (device, factory) = connected_device().await;
        factory.set_voltage_raw(1).await;
        assert!(device.get_switch(SWITCH_ID_VOLTAGE).await.unwrap());
    }

    #[tokio::test]
    async fn get_switch_id_0_is_false_when_raw_is_zero() {
        let (device, factory) = connected_device().await;
        factory.set_voltage_raw(0).await;
        assert!(!device.get_switch(SWITCH_ID_VOLTAGE).await.unwrap());
    }

    #[tokio::test]
    async fn get_switch_id_1_is_true_when_limit_set() {
        let (device, factory) = connected_device().await;
        factory.set_limit_detect(true).await;
        assert!(device.get_switch(SWITCH_ID_LIMIT).await.unwrap());
    }

    #[tokio::test]
    async fn get_switch_id_1_is_false_when_limit_clear() {
        let (device, _) = connected_device().await;
        assert!(!device.get_switch(SWITCH_ID_LIMIT).await.unwrap());
    }

    // ---- write rejections (already enforced for disconnected; check
    // INVALID_OPERATION fires when the device IS connected) -------------

    #[tokio::test]
    async fn set_switch_returns_invalid_operation_when_connected() {
        let (device, _) = connected_device().await;
        let err = device
            .set_switch(SWITCH_ID_VOLTAGE, true)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[tokio::test]
    async fn set_switch_value_returns_invalid_operation_when_connected() {
        let (device, _) = connected_device().await;
        let err = device
            .set_switch_value(SWITCH_ID_VOLTAGE, 0.0)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[tokio::test]
    async fn set_switch_name_returns_invalid_operation_when_connected() {
        let (device, _) = connected_device().await;
        let err = device
            .set_switch_name(SWITCH_ID_VOLTAGE, "x".to_string())
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }
}
