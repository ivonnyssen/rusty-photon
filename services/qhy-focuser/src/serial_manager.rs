//! Shared serial port manager for the QHY Q-Focuser
//!
//! Manages a shared serial port connection with reference counting,
//! background polling for position and temperature, and cached device state.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::config::{Config, SerialConfig};
use crate::error::{QhyFocuserError, Result};
use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use crate::protocol::{
    parse_position_response, parse_response, parse_temperature_response, parse_version_response,
    Command,
};

/// Cached state from the QHY Q-Focuser device
#[derive(Debug, Clone, Default)]
pub struct CachedState {
    /// Current focuser position
    pub position: Option<i64>,
    /// Target position for current move
    pub target_position: Option<i64>,
    /// Whether the focuser is currently moving
    pub is_moving: bool,
    /// Outer temperature in degrees Celsius
    pub outer_temp: Option<f64>,
    /// Chip temperature in degrees Celsius
    pub chip_temp: Option<f64>,
    /// Input voltage in volts
    pub voltage: Option<f64>,
    /// Firmware version string
    pub firmware_version: Option<String>,
    /// Board version string
    pub board_version: Option<String>,
}

/// Shared serial port manager for the QHY Q-Focuser
pub struct SerialManager {
    config: SerialConfig,
    speed: u8,
    connection_count: Arc<AtomicU32>,
    serial_available: Arc<AtomicBool>,
    cached_state: Arc<RwLock<CachedState>>,
    reader: Arc<Mutex<Option<Box<dyn SerialReader>>>>,
    writer: Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
    command_lock: Arc<Mutex<()>>,
    polling_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    shutdown_tx: watch::Sender<bool>,
    serial_factory: Arc<dyn SerialPortFactory>,
}

impl SerialManager {
    /// Create a new serial port manager
    pub fn new(config: Config, serial_factory: Arc<dyn SerialPortFactory>) -> Self {
        let (shutdown_tx, _) = watch::channel(false);

        Self {
            speed: config.focuser.speed,
            config: config.serial,
            connection_count: Arc::new(AtomicU32::new(0)),
            serial_available: Arc::new(AtomicBool::new(false)),
            cached_state: Arc::new(RwLock::new(CachedState::default())),
            reader: Arc::new(Mutex::new(None)),
            writer: Arc::new(Mutex::new(None)),
            command_lock: Arc::new(Mutex::new(())),
            polling_handle: Arc::new(Mutex::new(None)),
            shutdown_tx,
            serial_factory,
        }
    }

    /// Connect to the serial port
    ///
    /// Increments the connection reference count. If this is the first connection,
    /// opens the serial port, performs handshake, and starts polling.
    pub async fn connect(&self) -> Result<()> {
        let count = self.connection_count.fetch_add(1, Ordering::SeqCst);

        if count == 0 {
            debug!("First device connecting, opening serial port");

            let timeout = Duration::from_secs(self.config.timeout_seconds);

            let pair: SerialPair = self
                .serial_factory
                .open(&self.config.port, self.config.baud_rate, timeout)
                .await?;

            *self.reader.lock().await = Some(pair.reader);
            *self.writer.lock().await = Some(pair.writer);

            // Handshake: get version
            self.perform_handshake().await?;

            self.serial_available.store(true, Ordering::SeqCst);

            info!(
                "Serial port opened on {} (connection count: 1)",
                self.config.port
            );

            // Start polling
            self.start_polling().await;
        } else {
            debug!(
                "Additional device connecting (connection count: {})",
                count + 1
            );
        }

        Ok(())
    }

    /// Disconnect from the serial port
    pub async fn disconnect(&self) {
        let prev_count =
            match self
                .connection_count
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    if count > 0 {
                        Some(count - 1)
                    } else {
                        None
                    }
                }) {
                Ok(prev) => prev,
                Err(_) => {
                    debug!("disconnect() called with connection count already at 0");
                    return;
                }
            };

        if prev_count == 1 {
            debug!("Last device disconnecting, closing serial port");

            self.serial_available.store(false, Ordering::SeqCst);
            let _ = self.shutdown_tx.send(true);

            self.stop_polling().await;

            *self.reader.lock().await = None;
            *self.writer.lock().await = None;

            info!("Serial port closed (connection count: 0)");
        } else {
            debug!(
                "Device disconnecting (connection count: {})",
                prev_count - 1
            );
        }
    }

    /// Check if the serial port is available
    pub fn is_available(&self) -> bool {
        self.serial_available.load(Ordering::SeqCst)
    }

    /// Get a copy of the current cached state
    pub async fn get_cached_state(&self) -> CachedState {
        self.cached_state.read().await.clone()
    }

    /// Send a command to the device and return the raw response
    pub async fn send_command(&self, command: Command) -> Result<String> {
        if !self.is_available() {
            return Err(QhyFocuserError::NotConnected);
        }

        self.send_command_internal(command).await
    }

    /// Move to an absolute position
    pub async fn move_absolute(&self, position: i64) -> Result<()> {
        if !self.is_available() {
            return Err(QhyFocuserError::NotConnected);
        }

        // Set target and moving state before sending command
        {
            let mut cached = self.cached_state.write().await;
            cached.target_position = Some(position);
            cached.is_moving = true;
        }

        let command = Command::AbsoluteMove { position };

        self.send_command_internal(command).await?;

        debug!("Move command sent to position {}", position);
        Ok(())
    }

    /// Refresh position from the device and update cached state.
    ///
    /// This is used by `is_moving()` to actively check move completion
    /// rather than relying solely on the background polling loop.
    pub async fn refresh_position(&self) -> Result<()> {
        if !self.is_available() {
            return Err(QhyFocuserError::NotConnected);
        }

        let response = self.send_command_internal(Command::GetPosition).await?;
        let position = parse_position_response(&response)?;

        let mut cached = self.cached_state.write().await;
        cached.position = Some(position.position);

        // Detect move completion
        if cached.is_moving {
            if let Some(target) = cached.target_position {
                if position.position == target {
                    debug!(
                        "Move complete: position {} reached target {}",
                        position.position, target
                    );
                    cached.is_moving = false;
                    cached.target_position = None;
                }
            }
        }

        Ok(())
    }

    /// Abort current movement
    pub async fn abort(&self) -> Result<()> {
        if !self.is_available() {
            return Err(QhyFocuserError::NotConnected);
        }

        self.send_command_internal(Command::Abort).await?;

        // Clear moving state
        {
            let mut cached = self.cached_state.write().await;
            cached.is_moving = false;
            cached.target_position = None;
        }

        debug!("Abort command sent");
        Ok(())
    }

    /// Set focuser speed
    pub async fn set_speed(&self, speed: u8) -> Result<()> {
        if !self.is_available() {
            return Err(QhyFocuserError::NotConnected);
        }

        self.send_command_internal(Command::SetSpeed { speed })
            .await?;
        debug!("Speed set to {}", speed);
        Ok(())
    }

    /// Set reverse direction
    pub async fn set_reverse(&self, enabled: bool) -> Result<()> {
        if !self.is_available() {
            return Err(QhyFocuserError::NotConnected);
        }

        self.send_command_internal(Command::SetReverse { enabled })
            .await?;
        debug!("Reverse set to {}", enabled);
        Ok(())
    }

    /// Perform handshake: get version, set speed, initial position and temperature
    async fn perform_handshake(&self) -> Result<()> {
        // Get version
        let version_response = self.send_command_internal(Command::GetVersion).await?;
        let version = parse_version_response(&version_response)?;
        debug!(
            "Firmware: {}, Board: {}",
            version.firmware_version, version.board_version
        );

        // Set configured speed
        self.send_command_internal(Command::SetSpeed { speed: self.speed })
            .await?;
        debug!("Speed set to {} during handshake", self.speed);

        // Get initial position
        let position_response = self.send_command_internal(Command::GetPosition).await?;
        let position = parse_position_response(&position_response)?;
        debug!("Initial position: {}", position.position);

        // Get initial temperature
        let temp_response = self.send_command_internal(Command::ReadTemperature).await?;
        let temp = parse_temperature_response(&temp_response)?;
        debug!(
            "Initial temp: {}°C, voltage: {}V",
            temp.outer_temp, temp.voltage
        );

        // Update cached state
        {
            let mut cached = self.cached_state.write().await;
            cached.firmware_version = Some(version.firmware_version);
            cached.board_version = Some(version.board_version);
            cached.position = Some(position.position);
            cached.outer_temp = Some(temp.outer_temp);
            cached.chip_temp = Some(temp.chip_temp);
            cached.voltage = Some(temp.voltage);
        }

        Ok(())
    }

    /// Maximum number of stale responses to discard before giving up
    const MAX_RESPONSE_RETRIES: usize = 5;

    /// Internal command sending (doesn't check connection state)
    async fn send_command_internal(&self, command: Command) -> Result<String> {
        let _cmd_guard = self.command_lock.lock().await;
        let expected_idx = command.cmd_id();
        let command_str = command.to_json_string();
        debug!("Sending command: {}", command_str);

        // Write the command
        {
            let mut writer_guard = self.writer.lock().await;
            let writer = writer_guard.as_mut().ok_or(QhyFocuserError::NotConnected)?;
            writer.write_message(&command_str).await?;
        }

        // Read responses, discarding any with mismatched idx.
        // The device may send unsolicited position updates during movement.
        let response = {
            let mut reader_guard = self.reader.lock().await;
            let reader = reader_guard.as_mut().ok_or(QhyFocuserError::NotConnected)?;
            Self::read_response_for(reader, expected_idx).await?
        };

        debug!("Received response: {}", response);
        Ok(response)
    }

    /// Read responses from the serial port until one matches `expected_idx`,
    /// discarding stale/unsolicited responses from the device.
    async fn read_response_for(
        reader: &mut Box<dyn SerialReader>,
        expected_idx: u8,
    ) -> Result<String> {
        for attempt in 0..Self::MAX_RESPONSE_RETRIES {
            let response = reader
                .read_line()
                .await?
                .ok_or(QhyFocuserError::Communication(
                    "Connection closed".to_string(),
                ))?;

            match parse_response(&response, expected_idx) {
                Ok(_) => return Ok(response),
                Err(_) => {
                    debug!(
                        "Discarding stale response (attempt {}, expected idx {}): {}",
                        attempt + 1,
                        expected_idx,
                        response
                    );
                }
            }
        }

        Err(QhyFocuserError::Communication(format!(
            "No response with idx {} after {} reads",
            expected_idx,
            Self::MAX_RESPONSE_RETRIES
        )))
    }

    /// Start background polling for position and temperature
    async fn start_polling(&self) {
        let polling_interval_ms = self.config.polling_interval_ms;
        let cached_state = Arc::clone(&self.cached_state);
        let serial_available = Arc::clone(&self.serial_available);
        let reader = Arc::clone(&self.reader);
        let writer = Arc::clone(&self.writer);
        let command_lock = Arc::clone(&self.command_lock);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let handle = tokio::spawn(async move {
            let mut poll_interval = interval(Duration::from_millis(polling_interval_ms));

            loop {
                tokio::select! {
                    _ = poll_interval.tick() => {}
                    _ = shutdown_rx.changed() => {
                        debug!("Polling stopped: shutdown signal received");
                        break;
                    }
                }

                if !serial_available.load(Ordering::SeqCst) {
                    debug!("Polling stopped: serial port closed");
                    break;
                }

                // Poll position
                if let Err(e) =
                    Self::poll_position(&command_lock, &reader, &writer, &cached_state).await
                {
                    warn!("Failed to poll position: {}", e);
                }

                // Poll temperature
                if let Err(e) =
                    Self::poll_temperature(&command_lock, &reader, &writer, &cached_state).await
                {
                    warn!("Failed to poll temperature: {}", e);
                }
            }
        });

        let mut polling_handle = self.polling_handle.lock().await;
        *polling_handle = Some(handle);
    }

    /// Poll position (for use in background task)
    async fn poll_position(
        command_lock: &Arc<Mutex<()>>,
        reader: &Arc<Mutex<Option<Box<dyn SerialReader>>>>,
        writer: &Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
        cached_state: &Arc<RwLock<CachedState>>,
    ) -> Result<()> {
        let _cmd_guard = command_lock.lock().await;
        let cmd = Command::GetPosition;
        let command_str = cmd.to_json_string();

        {
            let mut writer_guard = writer.lock().await;
            if let Some(w) = writer_guard.as_mut() {
                w.write_message(&command_str).await?;
            } else {
                return Err(QhyFocuserError::NotConnected);
            }
        }

        let response = {
            let mut reader_guard = reader.lock().await;
            if let Some(r) = reader_guard.as_mut() {
                Self::read_response_for(r, cmd.cmd_id()).await?
            } else {
                return Err(QhyFocuserError::NotConnected);
            }
        };

        let position = parse_position_response(&response)?;

        let mut cached = cached_state.write().await;
        cached.position = Some(position.position);

        // Detect move completion: position matches target
        if cached.is_moving {
            if let Some(target) = cached.target_position {
                if position.position == target {
                    debug!(
                        "Move complete: position {} reached target {}",
                        position.position, target
                    );
                    cached.is_moving = false;
                    cached.target_position = None;
                }
            }
        }

        Ok(())
    }

    /// Poll temperature (for use in background task)
    async fn poll_temperature(
        command_lock: &Arc<Mutex<()>>,
        reader: &Arc<Mutex<Option<Box<dyn SerialReader>>>>,
        writer: &Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
        cached_state: &Arc<RwLock<CachedState>>,
    ) -> Result<()> {
        let _cmd_guard = command_lock.lock().await;
        let cmd = Command::ReadTemperature;
        let command_str = cmd.to_json_string();

        {
            let mut writer_guard = writer.lock().await;
            if let Some(w) = writer_guard.as_mut() {
                w.write_message(&command_str).await?;
            } else {
                return Err(QhyFocuserError::NotConnected);
            }
        }

        let response = {
            let mut reader_guard = reader.lock().await;
            if let Some(r) = reader_guard.as_mut() {
                Self::read_response_for(r, cmd.cmd_id()).await?
            } else {
                return Err(QhyFocuserError::NotConnected);
            }
        };

        let temp = parse_temperature_response(&response)?;

        let mut cached = cached_state.write().await;
        cached.outer_temp = Some(temp.outer_temp);
        cached.chip_temp = Some(temp.chip_temp);
        cached.voltage = Some(temp.voltage);

        Ok(())
    }

    /// Stop background polling
    async fn stop_polling(&self) {
        let mut handle = self.polling_handle.lock().await;
        if let Some(h) = handle.take() {
            match tokio::time::timeout(Duration::from_secs(5), h).await {
                Ok(_) => debug!("Polling task stopped gracefully"),
                Err(_) => {
                    warn!("Polling task did not stop within 5 seconds, it will be dropped");
                }
            }
        }
    }
}

impl std::fmt::Debug for SerialManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialManager")
            .field("config", &self.config)
            .field("connection_count", &self.connection_count)
            .field("serial_available", &self.serial_available)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::SerialReader;

    /// Simple mock reader that returns responses in order, then None.
    struct MockReader {
        responses: Vec<String>,
        index: usize,
    }

    impl MockReader {
        fn new(responses: Vec<String>) -> Box<dyn SerialReader> {
            Box::new(Self {
                responses,
                index: 0,
            })
        }
    }

    #[async_trait::async_trait]
    impl SerialReader for MockReader {
        async fn read_line(&mut self) -> Result<Option<String>> {
            if self.index < self.responses.len() {
                let response = self.responses[self.index].clone();
                self.index += 1;
                Ok(Some(response))
            } else {
                Ok(None)
            }
        }
    }

    #[tokio::test]
    async fn test_read_response_for_returns_matching_idx() {
        let mut reader = MockReader::new(vec![r#"{"idx": 5, "pos": 10000}"#.to_string()]);
        let response = SerialManager::read_response_for(&mut reader, 5)
            .await
            .unwrap();
        assert!(response.contains("10000"));
    }

    #[tokio::test]
    async fn test_read_response_for_discards_stale() {
        let mut reader = MockReader::new(vec![
            r#"{"idx": 6}"#.to_string(),               // stale — wrong idx
            r#"{"idx": 5, "pos": 12345}"#.to_string(), // correct
        ]);
        let response = SerialManager::read_response_for(&mut reader, 5)
            .await
            .unwrap();
        assert!(response.contains("12345"));
    }

    #[tokio::test]
    async fn test_read_response_for_retries_exhausted() {
        let stale_responses: Vec<String> = (0..5).map(|_| r#"{"idx": 6}"#.to_string()).collect();
        let mut reader = MockReader::new(stale_responses);

        let err = SerialManager::read_response_for(&mut reader, 5)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("No response with idx 5 after 5 reads"),
            "Expected retry-exhaustion error, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_read_response_for_connection_closed() {
        let mut reader = MockReader::new(vec![]);

        let err = SerialManager::read_response_for(&mut reader, 5)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Connection closed"), "got: {}", msg);
    }
}
