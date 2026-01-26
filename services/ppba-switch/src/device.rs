//! PPBA Switch device implementation
//!
//! This module implements the ASCOM Alpaca Device and Switch traits
//! for the Pegasus Astro Pocket Powerbox Advance Gen2.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Device, Switch};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::error::{PpbaError, Result};
use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use crate::protocol::{
    parse_power_stats_response, parse_status_response, validate_ping_response,
    validate_set_response, PpbaCommand, PpbaPowerStats, PpbaStatus,
};
use crate::serial::TokioSerialPortFactory;
use crate::switches::{get_switch_info, SwitchId, MAX_SWITCH};

/// Cached state from the PPBA device
#[derive(Debug, Clone, Default)]
struct CachedState {
    /// Last known device status (from PA command)
    status: Option<PpbaStatus>,
    /// Last known power statistics (from PS command)
    power_stats: Option<PpbaPowerStats>,
    /// USB hub state (tracked separately)
    usb_hub_enabled: bool,
}

/// PPBA Switch device for ASCOM Alpaca
pub struct PpbaSwitchDevice {
    config: Config,
    connected: Arc<RwLock<bool>>,
    cached_state: Arc<RwLock<CachedState>>,
    reader: Arc<Mutex<Option<Box<dyn SerialReader>>>>,
    writer: Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
    polling_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    serial_factory: Arc<dyn SerialPortFactory>,
}

impl fmt::Debug for PpbaSwitchDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PpbaSwitchDevice")
            .field("config", &self.config)
            .field("connected", &self.connected)
            .field("cached_state", &self.cached_state)
            .finish_non_exhaustive()
    }
}

impl PpbaSwitchDevice {
    /// Create a new PPBA switch device with the default serial port factory
    pub fn new(config: Config) -> Self {
        Self::with_serial_factory(config, Arc::new(TokioSerialPortFactory::new()))
    }

    /// Create a new PPBA switch device with a custom serial port factory
    ///
    /// This is primarily used for testing with mock implementations.
    pub fn with_serial_factory(config: Config, factory: Arc<dyn SerialPortFactory>) -> Self {
        Self {
            config,
            connected: Arc::new(RwLock::new(false)),
            cached_state: Arc::new(RwLock::new(CachedState::default())),
            reader: Arc::new(Mutex::new(None)),
            writer: Arc::new(Mutex::new(None)),
            polling_handle: Arc::new(Mutex::new(None)),
            serial_factory: factory,
        }
    }

    /// Connect to the PPBA device
    async fn connect_device(&self) -> Result<()> {
        let timeout = Duration::from_secs(self.config.serial.timeout_seconds);

        let pair: SerialPair = self
            .serial_factory
            .open(
                &self.config.serial.port,
                self.config.serial.baud_rate,
                timeout,
            )
            .await?;

        // Store the reader and writer
        {
            let mut reader_guard = self.reader.lock().await;
            *reader_guard = Some(pair.reader);
        }
        {
            let mut writer_guard = self.writer.lock().await;
            *writer_guard = Some(pair.writer);
        }

        // Verify connection with ping
        self.send_command(PpbaCommand::Ping).await?;

        // Get initial status
        self.refresh_status().await?;
        self.refresh_power_stats().await?;

        info!("Connected to PPBA device on {}", self.config.serial.port);

        Ok(())
    }

    /// Disconnect from the PPBA device
    async fn disconnect_device(&self) {
        // Clear reader and writer
        {
            let mut reader_guard = self.reader.lock().await;
            *reader_guard = None;
        }
        {
            let mut writer_guard = self.writer.lock().await;
            *writer_guard = None;
        }

        info!("Disconnected from PPBA device");
    }

    /// Send a command and get the response
    async fn send_command(&self, command: PpbaCommand) -> Result<String> {
        let command_str = command.to_command_string();
        debug!("Sending command: {}", command_str);

        // Write the command
        {
            let mut writer_guard = self.writer.lock().await;
            let writer = writer_guard.as_mut().ok_or(PpbaError::NotConnected)?;
            writer.write_message(&command_str).await?;
        }

        // Read the response
        let response = {
            let mut reader_guard = self.reader.lock().await;
            let reader = reader_guard.as_mut().ok_or(PpbaError::NotConnected)?;
            reader
                .read_line()
                .await?
                .ok_or(PpbaError::Communication("Connection closed".to_string()))?
        };

        debug!("Received response: {}", response);

        // Validate response based on command type
        match &command {
            PpbaCommand::Ping => validate_ping_response(&response)?,
            PpbaCommand::Status | PpbaCommand::PowerStats | PpbaCommand::FirmwareVersion => {
                // These commands return data, validation happens during parsing
            }
            _ => validate_set_response(&command, &response)?,
        }

        Ok(response)
    }

    /// Refresh the device status (PA command)
    async fn refresh_status(&self) -> Result<()> {
        let response = self.send_command(PpbaCommand::Status).await?;
        let status = parse_status_response(&response)?;

        let mut cached = self.cached_state.write().await;
        cached.status = Some(status);

        Ok(())
    }

    /// Refresh power statistics (PS command)
    async fn refresh_power_stats(&self) -> Result<()> {
        let response = self.send_command(PpbaCommand::PowerStats).await?;
        let stats = parse_power_stats_response(&response)?;

        let mut cached = self.cached_state.write().await;
        cached.power_stats = Some(stats);

        Ok(())
    }

    /// Start background polling for status updates
    async fn start_polling(&self) {
        let config = self.config.clone();
        let cached_state = Arc::clone(&self.cached_state);
        let connected = Arc::clone(&self.connected);
        let reader = Arc::clone(&self.reader);
        let writer = Arc::clone(&self.writer);

        let handle = tokio::spawn(async move {
            let mut poll_interval =
                interval(Duration::from_secs(config.serial.polling_interval_seconds));

            loop {
                poll_interval.tick().await;

                // Check if still connected
                if !*connected.read().await {
                    debug!("Polling stopped: device disconnected");
                    break;
                }

                // Refresh status
                if let Err(e) = Self::poll_status(&reader, &writer, &cached_state).await {
                    warn!("Failed to poll PPBA status: {}", e);
                }

                // Refresh power stats
                if let Err(e) = Self::poll_power_stats(&reader, &writer, &cached_state).await {
                    warn!("Failed to poll PPBA power stats: {}", e);
                }
            }
        });

        let mut polling_handle = self.polling_handle.lock().await;
        *polling_handle = Some(handle);
    }

    /// Poll status (for use in background task)
    async fn poll_status(
        reader: &Arc<Mutex<Option<Box<dyn SerialReader>>>>,
        writer: &Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
        cached_state: &Arc<RwLock<CachedState>>,
    ) -> Result<()> {
        let command_str = PpbaCommand::Status.to_command_string();

        // Write command
        {
            let mut writer_guard = writer.lock().await;
            if let Some(w) = writer_guard.as_mut() {
                w.write_message(&command_str).await?;
            } else {
                return Err(PpbaError::NotConnected);
            }
        }

        // Read response
        let response = {
            let mut reader_guard = reader.lock().await;
            if let Some(r) = reader_guard.as_mut() {
                r.read_line()
                    .await?
                    .ok_or(PpbaError::Communication("Connection closed".to_string()))?
            } else {
                return Err(PpbaError::NotConnected);
            }
        };

        let status = parse_status_response(&response)?;
        let mut cached = cached_state.write().await;
        cached.status = Some(status);

        Ok(())
    }

    /// Poll power stats (for use in background task)
    async fn poll_power_stats(
        reader: &Arc<Mutex<Option<Box<dyn SerialReader>>>>,
        writer: &Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
        cached_state: &Arc<RwLock<CachedState>>,
    ) -> Result<()> {
        let command_str = PpbaCommand::PowerStats.to_command_string();

        // Write command
        {
            let mut writer_guard = writer.lock().await;
            if let Some(w) = writer_guard.as_mut() {
                w.write_message(&command_str).await?;
            } else {
                return Err(PpbaError::NotConnected);
            }
        }

        // Read response
        let response = {
            let mut reader_guard = reader.lock().await;
            if let Some(r) = reader_guard.as_mut() {
                r.read_line()
                    .await?
                    .ok_or(PpbaError::Communication("Connection closed".to_string()))?
            } else {
                return Err(PpbaError::NotConnected);
            }
        };

        let stats = parse_power_stats_response(&response)?;
        let mut cached = cached_state.write().await;
        cached.power_stats = Some(stats);

        Ok(())
    }

    /// Stop background polling
    async fn stop_polling(&self) {
        let mut handle = self.polling_handle.lock().await;
        if let Some(h) = handle.take() {
            h.abort();
            debug!("Polling task aborted");
        }
    }

    /// Get the current switch value for a given switch ID
    async fn get_switch_value_internal(&self, id: u16) -> Result<f64> {
        let switch_id = SwitchId::from_id(id).ok_or(PpbaError::InvalidSwitchId(id))?;
        let cached = self.cached_state.read().await;

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
    async fn set_switch_value_internal(&self, id: u16, value: f64) -> Result<()> {
        let switch_id = SwitchId::from_id(id).ok_or(PpbaError::InvalidSwitchId(id))?;
        let info = get_switch_info(id).ok_or(PpbaError::InvalidSwitchId(id))?;

        if !info.can_write {
            return Err(PpbaError::SwitchNotWritable(id));
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
                let enabled = value >= 0.5;
                let cmd = PpbaCommand::SetUsbHub(enabled);
                self.send_command(cmd).await?;
                // Update cached state
                let mut cached = self.cached_state.write().await;
                cached.usb_hub_enabled = enabled;
                return Ok(());
            }
            SwitchId::AutoDew => PpbaCommand::SetAutoDew(value >= 0.5),
            _ => return Err(PpbaError::SwitchNotWritable(id)),
        };

        self.send_command(command).await?;

        // Refresh status to get updated values
        self.refresh_status().await?;

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
        &self.config.device.name
    }

    fn unique_id(&self) -> &str {
        &self.config.device.unique_id
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.config.device.description.clone())
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        Ok(*self.connected.read().await)
    }

    async fn set_connected(&self, connected: bool) -> std::result::Result<(), ASCOMError> {
        if connected {
            // Connect to device
            self.connect_device().await.map_err(Self::to_ascom_error)?;

            // Set connected state
            {
                let mut conn_state = self.connected.write().await;
                *conn_state = true;
            }

            // Start polling
            self.start_polling().await;
        } else {
            // Stop polling first
            self.stop_polling().await;

            // Set disconnected state
            {
                let mut conn_state = self.connected.write().await;
                *conn_state = false;
            }

            // Disconnect device
            self.disconnect_device().await;
        }

        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("PPBA Switch Driver for Pegasus Astro Pocket Powerbox Advance Gen2".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl Switch for PpbaSwitchDevice {
    async fn max_switch(&self) -> ASCOMResult<usize> {
        Ok(MAX_SWITCH as usize)
    }

    async fn can_write(&self, id: usize) -> ASCOMResult<bool> {
        let id = id as u16;
        let info = get_switch_info(id)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(info.can_write)
    }

    async fn get_switch(&self, id: usize) -> ASCOMResult<bool> {
        if !*self.connected.read().await {
            return Err(ASCOMError::new(
                ASCOMErrorCode::NOT_CONNECTED,
                "Not connected",
            ));
        }

        let value = self
            .get_switch_value_internal(id as u16)
            .await
            .map_err(Self::to_ascom_error)?;

        // Per ASCOM spec: False at minimum value, True above minimum
        let info = get_switch_info(id as u16)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(value > info.min_value)
    }

    async fn set_switch(&self, id: usize, state: bool) -> ASCOMResult<()> {
        if !*self.connected.read().await {
            return Err(ASCOMError::new(
                ASCOMErrorCode::NOT_CONNECTED,
                "Not connected",
            ));
        }

        let info = get_switch_info(id as u16)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;

        // Per ASCOM spec: True sets to max, False sets to min
        let value = if state {
            info.max_value
        } else {
            info.min_value
        };

        self.set_switch_value_internal(id as u16, value)
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn get_switch_description(&self, id: usize) -> ASCOMResult<String> {
        let info = get_switch_info(id as u16)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(info.description.to_string())
    }

    async fn get_switch_name(&self, id: usize) -> ASCOMResult<String> {
        let info = get_switch_info(id as u16)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(info.name.to_string())
    }

    async fn set_switch_name(&self, _id: usize, _name: String) -> ASCOMResult<()> {
        Err(ASCOMError::new(
            ASCOMErrorCode::NOT_IMPLEMENTED,
            "Setting switch names is not supported",
        ))
    }

    async fn get_switch_value(&self, id: usize) -> ASCOMResult<f64> {
        if !*self.connected.read().await {
            return Err(ASCOMError::new(
                ASCOMErrorCode::NOT_CONNECTED,
                "Not connected",
            ));
        }

        self.get_switch_value_internal(id as u16)
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn set_switch_value(&self, id: usize, value: f64) -> ASCOMResult<()> {
        if !*self.connected.read().await {
            return Err(ASCOMError::new(
                ASCOMErrorCode::NOT_CONNECTED,
                "Not connected",
            ));
        }

        self.set_switch_value_internal(id as u16, value)
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn min_switch_value(&self, id: usize) -> ASCOMResult<f64> {
        let info = get_switch_info(id as u16)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(info.min_value)
    }

    async fn max_switch_value(&self, id: usize) -> ASCOMResult<f64> {
        let info = get_switch_info(id as u16)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(info.max_value)
    }

    async fn switch_step(&self, id: usize) -> ASCOMResult<f64> {
        let info = get_switch_info(id as u16)
            .ok_or_else(|| ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, "Invalid switch ID"))?;
        Ok(info.step)
    }

    async fn can_async(&self, id: usize) -> ASCOMResult<bool> {
        // Validate switch ID
        let id = id as u16;
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
        // Validate switch ID
        let id = id as u16;
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
        // Validate switch ID first
        let id_u16 = id as u16;
        if id_u16 >= MAX_SWITCH {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Invalid switch ID: {}", id_u16),
            ));
        }

        // We don't support async operations, so there's nothing to cancel.
        // Per ASCOM spec, this should return OK if there's no operation in progress.
        Ok(())
    }

    async fn set_async(&self, id: usize, state: bool) -> ASCOMResult<()> {
        // Validate switch ID first
        let id_u16 = id as u16;
        if id_u16 >= MAX_SWITCH {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Invalid switch ID: {}", id_u16),
            ));
        }

        // Per ASCOM spec: SetAsync should work even if CanAsync returns false,
        // it just completes immediately. We delegate to the synchronous method.
        self.set_switch(id, state).await
    }

    async fn set_async_value(&self, id: usize, value: f64) -> ASCOMResult<()> {
        // Validate switch ID first
        let id_u16 = id as u16;
        if id_u16 >= MAX_SWITCH {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Invalid switch ID: {}", id_u16),
            ));
        }

        // Per ASCOM spec: SetAsyncValue should work even if CanAsync returns false,
        // it just completes immediately. We delegate to the synchronous method.
        self.set_switch_value(id, value).await
    }
}
