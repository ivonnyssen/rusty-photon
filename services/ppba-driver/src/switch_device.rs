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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    //! Unit tests for PpbaSwitchDevice ASCOM error mapping and edge cases
    //!
    //! These tests exercise error paths in the Switch device that are only
    //! reachable through internal failures (factory errors, bad pings) or
    //! specific invalid inputs, covering `to_ascom_error` branches and the
    //! Debug implementation.

    use super::*;
    use crate::config::Config;
    use crate::error::PpbaError;
    use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
    use crate::serial_manager::SerialManager;
    use crate::switches::MAX_SWITCH;
    use ascom_alpaca::api::{Device, Switch};
    use ascom_alpaca::ASCOMErrorCode;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    // ============================================================================
    // Mock Serial Infrastructure
    // ============================================================================

    struct MockSerialReader {
        responses: Arc<Mutex<Vec<String>>>,
        index: Arc<Mutex<usize>>,
    }

    impl MockSerialReader {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses)),
                index: Arc::new(Mutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl SerialReader for MockSerialReader {
        async fn read_line(&mut self) -> crate::error::Result<Option<String>> {
            let responses = self.responses.lock().await;
            let mut index = self.index.lock().await;
            if *index < responses.len() {
                let response = responses[*index].clone();
                *index += 1;
                Ok(Some(response))
            } else {
                *index = 0;
                if !responses.is_empty() {
                    Ok(Some(responses[0].clone()))
                } else {
                    Ok(None)
                }
            }
        }
    }

    struct MockSerialWriter;

    #[async_trait]
    impl SerialWriter for MockSerialWriter {
        async fn write_message(&mut self, _message: &str) -> crate::error::Result<()> {
            Ok(())
        }
    }

    struct MockSerialPortFactory {
        responses: Vec<String>,
    }

    impl MockSerialPortFactory {
        fn new(responses: Vec<String>) -> Self {
            Self { responses }
        }
    }

    #[async_trait]
    impl SerialPortFactory for MockSerialPortFactory {
        async fn open(
            &self,
            _port: &str,
            _baud_rate: u32,
            _timeout: Duration,
        ) -> crate::error::Result<SerialPair> {
            Ok(SerialPair {
                reader: Box::new(MockSerialReader::new(self.responses.clone())),
                writer: Box::new(MockSerialWriter),
            })
        }

        async fn port_exists(&self, _port: &str) -> bool {
            true
        }
    }

    struct FailingMockSerialPortFactory;

    #[async_trait]
    impl SerialPortFactory for FailingMockSerialPortFactory {
        async fn open(
            &self,
            _port: &str,
            _baud_rate: u32,
            _timeout: Duration,
        ) -> crate::error::Result<SerialPair> {
            Err(PpbaError::ConnectionFailed(
                "Mock factory error".to_string(),
            ))
        }

        async fn port_exists(&self, _port: &str) -> bool {
            false
        }
    }

    fn standard_connection_responses() -> Vec<String> {
        vec![
            "PPBA_OK".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
        ]
    }

    fn create_switch_device(factory: Arc<dyn SerialPortFactory>) -> PpbaSwitchDevice {
        let config = Config::default();
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        PpbaSwitchDevice::new(config.switch, serial_manager)
    }

    // ============================================================================
    // Connection Error Mapping Tests
    // ============================================================================

    #[tokio::test]
    async fn test_switch_connect_factory_error_maps_to_invalid_operation() {
        let device = create_switch_device(Arc::new(FailingMockSerialPortFactory));

        let err = device.set_connected(true).await.unwrap_err();
        // ConnectionFailed maps to INVALID_OPERATION via to_ascom_error's catch-all
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(err.message.contains("Connection failed"));
    }

    #[tokio::test]
    async fn test_switch_connect_bad_ping_maps_to_invalid_operation() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![
            "BAD_RESPONSE".to_string(), // bad ping
        ]));
        let device = create_switch_device(factory);

        let err = device.set_connected(true).await.unwrap_err();
        // InvalidResponse maps to INVALID_OPERATION via to_ascom_error's catch-all
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    // ============================================================================
    // Switch Value Error Mapping Tests
    // ============================================================================

    #[tokio::test]
    async fn test_switch_get_value_invalid_id_maps_to_invalid_value() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        let err = device.get_switch_value(99).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_switch_set_value_invalid_id_maps_to_invalid_value() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        let err = device.set_switch_value(99, 0.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_switch_set_value_read_only_maps_to_not_implemented() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // Switch 10 (InputVoltage) is read-only
        let err = device.set_switch_value(10, 5.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_switch_set_value_out_of_range_maps_to_invalid_value() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // Switch 0 (Quad12V) is boolean: min=0, max=1. Value 5.0 is out of range.
        let err = device.set_switch_value(0, 5.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_switch_set_value_auto_dew_enabled_maps_to_invalid_operation() {
        // Auto-dew ON: status field auto_dew=1
        let factory = Arc::new(MockSerialPortFactory::new(vec![
            "PPBA_OK".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(), // auto_dew=1
            "PS:2.5:10.5:126.0:3600000".to_string(),
            // Polling responses
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            // refresh_status response for the set_switch_value_internal call
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
        ]));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // Switch 2 (DewHeaterA) should fail when auto-dew is enabled
        let err = device.set_switch_value(2, 128.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(err.message.contains("auto-dew"));

        device.set_connected(false).await.unwrap();
    }

    // ============================================================================
    // Not Connected Guard Tests
    // ============================================================================

    #[tokio::test]
    async fn test_switch_operations_fail_when_not_connected() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![]));
        let device = create_switch_device(factory);

        // All operations requiring connection should return NOT_CONNECTED
        assert_eq!(
            device.get_switch(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.get_switch_value(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.set_switch(0, true).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.set_switch_value(0, 1.0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.can_write(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.get_switch_name(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.get_switch_description(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.min_switch_value(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.max_switch_value(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.switch_step(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
    }

    #[tokio::test]
    async fn test_switch_async_operations_fail_when_not_connected() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![]));
        let device = create_switch_device(factory);

        assert_eq!(
            device.can_async(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.state_change_complete(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.cancel_async(0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.set_async(0, true).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.set_async_value(0, 1.0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
    }

    // ============================================================================
    // Async Switch Delegation Tests
    // ============================================================================

    #[tokio::test]
    async fn test_switch_set_async_delegates_to_set_switch() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![
            "PPBA_OK".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            // Response for the set command
            "P1:1".to_string(),
            // refresh_status after set
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            // Polling responses
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
        ]));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // set_async should delegate to set_switch and succeed
        device.set_async(0, true).await.unwrap();

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_switch_set_async_value_delegates_to_set_switch_value() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![
            "PPBA_OK".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            // Response for the set command
            "P1:1".to_string(),
            // refresh_status after set
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            // Polling responses
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
        ]));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // set_async_value should delegate to set_switch_value and succeed
        device.set_async_value(0, 1.0).await.unwrap();

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_switch_async_invalid_id_maps_to_invalid_value() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        assert_eq!(
            device.can_async(MAX_SWITCH).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
        assert_eq!(
            device
                .state_change_complete(MAX_SWITCH)
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::INVALID_VALUE
        );
        assert_eq!(
            device.cancel_async(MAX_SWITCH).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
        assert_eq!(
            device.set_async(MAX_SWITCH, true).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
        assert_eq!(
            device
                .set_async_value(MAX_SWITCH, 1.0)
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::INVALID_VALUE
        );

        device.set_connected(false).await.unwrap();
    }

    // ============================================================================
    // Switch Value Read Tests (covers get_switch_value_internal branches)
    // ============================================================================

    #[tokio::test]
    async fn test_switch_read_all_status_switches() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // Controllable switches from PA status
        // Status: voltage=12.5, current=3.2, temp=25.0, humidity=60, dewpoint=15.5,
        //         quad=1, adj=0, dewA=128, dewB=64, autodew=0, warn=0
        assert!((device.get_switch_value(0).await.unwrap() - 1.0).abs() < f64::EPSILON); // Quad12V on
        assert!((device.get_switch_value(1).await.unwrap() - 0.0).abs() < f64::EPSILON); // Adj off
        assert!((device.get_switch_value(2).await.unwrap() - 128.0).abs() < f64::EPSILON); // DewA
        assert!((device.get_switch_value(3).await.unwrap() - 64.0).abs() < f64::EPSILON); // DewB
        assert!((device.get_switch_value(4).await.unwrap() - 0.0).abs() < f64::EPSILON); // USB hub off
        assert!((device.get_switch_value(5).await.unwrap() - 0.0).abs() < f64::EPSILON); // AutoDew off

        // Read-only sensor switches from PA status
        assert!((device.get_switch_value(10).await.unwrap() - 12.5).abs() < 0.01); // Voltage
        assert!((device.get_switch_value(11).await.unwrap() - 3.2).abs() < 0.01); // Current
        assert!((device.get_switch_value(12).await.unwrap() - 25.0).abs() < 0.01); // Temperature
        assert!((device.get_switch_value(13).await.unwrap() - 60.0).abs() < 0.01); // Humidity
        assert!((device.get_switch_value(14).await.unwrap() - 15.5).abs() < 0.01); // Dewpoint
        assert!((device.get_switch_value(15).await.unwrap() - 0.0).abs() < f64::EPSILON); // PowerWarn

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_switch_read_power_stat_switches() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // Power stats: average_amps=2.5, amp_hours=10.5, watt_hours=126.0, uptime=3600000ms
        assert!((device.get_switch_value(6).await.unwrap() - 2.5).abs() < 0.01); // AvgCurrent
        assert!((device.get_switch_value(7).await.unwrap() - 10.5).abs() < 0.01); // AmpHours
        assert!((device.get_switch_value(8).await.unwrap() - 126.0).abs() < 0.01); // WattHours
        assert!((device.get_switch_value(9).await.unwrap() - 1.0).abs() < 0.01); // Uptime (1 hour)

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_switch_get_boolean_state() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // get_switch returns true if value > min_value
        assert!(device.get_switch(0).await.unwrap()); // Quad12V=1 > 0
        assert!(!device.get_switch(1).await.unwrap()); // Adj=0, not > 0

        device.set_connected(false).await.unwrap();
    }

    // ============================================================================
    // Switch Metadata Read Tests (covers ASCOM trait methods when connected)
    // ============================================================================

    #[tokio::test]
    async fn test_switch_metadata_when_connected() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // can_write
        assert!(device.can_write(0).await.unwrap()); // Quad12V is writable
        assert!(!device.can_write(10).await.unwrap()); // InputVoltage is read-only

        // name and description
        let name = device.get_switch_name(0).await.unwrap();
        assert!(!name.is_empty());
        let desc = device.get_switch_description(0).await.unwrap();
        assert!(!desc.is_empty());

        // min/max/step
        let min = device.min_switch_value(0).await.unwrap();
        let max = device.max_switch_value(0).await.unwrap();
        let step = device.switch_step(0).await.unwrap();
        assert!((min - 0.0).abs() < f64::EPSILON);
        assert!((max - 1.0).abs() < f64::EPSILON);
        assert!((step - 1.0).abs() < f64::EPSILON);

        // can_async / state_change_complete
        assert!(!device.can_async(0).await.unwrap());
        assert!(device.state_change_complete(0).await.unwrap());
        device.cancel_async(0).await.unwrap();

        device.set_connected(false).await.unwrap();
    }

    // ============================================================================
    // Switch Write Tests (covers set_switch_value_internal command branches)
    // ============================================================================

    #[tokio::test]
    async fn test_switch_set_controllable_switches() {
        // Provide enough responses for connect + multiple set commands + refresh after each
        let factory = Arc::new(MockSerialPortFactory::new(vec![
            // Connect handshake
            "PPBA_OK".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            // Set Quad12V(false): command response + refresh_status
            "P1:0".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:0:0:0".to_string(),
            // Set AdjustableOutput(true): command response + refresh_status
            "P2:1".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:1:128:64:0:0:0".to_string(),
            // Set DewHeaterA: refresh_status (auto-dew check) + command + refresh_status
            "PPBA:12.5:3.2:25.0:60:15.5:0:1:128:64:0:0:0".to_string(),
            "P3:200".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:1:200:64:0:0:0".to_string(),
            // Set DewHeaterB: refresh_status (auto-dew check) + command + refresh_status
            "PPBA:12.5:3.2:25.0:60:15.5:0:1:200:64:0:0:0".to_string(),
            "P4:100".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:1:200:100:0:0:0".to_string(),
            // Set UsbHub: command only (no refresh_status)
            "PU:1".to_string(),
            // Set AutoDew: command + refresh_status
            "PD:1".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:1:200:100:1:0:0".to_string(),
            // Polling responses
            "PPBA:12.5:3.2:25.0:60:15.5:0:1:200:100:1:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
        ]));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // Set each controllable switch
        device.set_switch_value(0, 0.0).await.unwrap(); // Quad12V off
        device.set_switch_value(1, 1.0).await.unwrap(); // Adjustable on
        device.set_switch_value(2, 200.0).await.unwrap(); // DewA PWM
        device.set_switch_value(3, 100.0).await.unwrap(); // DewB PWM
        device.set_switch_value(4, 1.0).await.unwrap(); // USB hub on
        device.set_switch_value(5, 1.0).await.unwrap(); // AutoDew on

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_switch_set_switch_boolean() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![
            "PPBA_OK".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            // set_switch(0, false) -> set_switch_value(0, 0.0): command + refresh
            "P1:0".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:0:0:0".to_string(),
            // Polling responses
            "PPBA:12.5:3.2:25.0:60:15.5:0:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
        ]));
        let device = create_switch_device(factory);
        device.set_connected(true).await.unwrap();

        // set_switch uses boolean -> min/max value conversion
        device.set_switch(0, false).await.unwrap();

        device.set_connected(false).await.unwrap();
    }

    // ============================================================================
    // Miscellaneous Tests
    // ============================================================================

    #[tokio::test]
    async fn test_switch_max_switch() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![]));
        let device = create_switch_device(factory);

        assert_eq!(device.max_switch().await.unwrap(), MAX_SWITCH);
    }

    #[tokio::test]
    async fn test_switch_set_switch_name_not_implemented() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![]));
        let device = create_switch_device(factory);

        let err = device
            .set_switch_name(0, "test".to_string())
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn test_switch_device_debug_format() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![]));
        let device = create_switch_device(factory);

        let debug_str = format!("{:?}", device);
        assert!(debug_str.contains("PpbaSwitchDevice"));
        assert!(debug_str.contains("config"));
        assert!(debug_str.contains("requested_connection"));
        assert!(debug_str.contains(".."));
    }

    #[tokio::test]
    async fn test_switch_device_info() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![]));
        let device = create_switch_device(factory);

        let info = device.driver_info().await.unwrap();
        assert!(info.contains("PPBA"));

        let version = device.driver_version().await.unwrap();
        assert!(!version.is_empty());

        let description = device.description().await.unwrap();
        assert!(!description.is_empty());
    }
}
