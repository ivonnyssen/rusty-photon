//! Shared serial port manager
//!
//! This module manages a shared serial port connection that can be used by multiple
//! ASCOM devices simultaneously. It implements reference counting to ensure the port
//! is opened when the first device connects and closed when the last device disconnects.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::config::{Config, SerialConfig};
use crate::error::{PpbaError, Result};
use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use crate::mean::SensorMean;
use crate::protocol::{
    parse_power_stats_response, parse_status_response, validate_ping_response, PpbaCommand,
    PpbaPowerStats, PpbaStatus,
};

/// Cached state from the PPBA device including sensor means
#[derive(Debug, Clone, Default)]
pub struct CachedState {
    /// Last known device status (from PA command)
    pub status: Option<PpbaStatus>,
    /// Last known power statistics (from PS command)
    pub power_stats: Option<PpbaPowerStats>,
    /// USB hub state (tracked separately)
    pub usb_hub_enabled: bool,
    /// Last update timestamp
    pub last_update: Option<SystemTime>,
    /// Temperature sensor mean
    pub temp_mean: SensorMean,
    /// Humidity sensor mean
    pub humidity_mean: SensorMean,
    /// Dewpoint sensor mean
    pub dewpoint_mean: SensorMean,
}

/// Shared serial port manager
///
/// Manages a single serial port connection that can be shared between multiple
/// ASCOM devices. Uses reference counting to track how many devices are connected.
pub struct SerialManager {
    config: SerialConfig,
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
        // Initialize sensor means with configured averaging period
        let mut cached_state = CachedState::default();
        let window = Duration::from_millis(config.serial.polling_interval_ms * 60); // Default window
        cached_state.temp_mean.set_window(window);
        cached_state.humidity_mean.set_window(window);
        cached_state.dewpoint_mean.set_window(window);

        let (shutdown_tx, _) = watch::channel(false);

        Self {
            config: config.serial,
            connection_count: Arc::new(AtomicU32::new(0)),
            serial_available: Arc::new(AtomicBool::new(false)),
            cached_state: Arc::new(RwLock::new(cached_state)),
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
    /// opens the serial port and starts polling.
    pub async fn connect(&self) -> Result<()> {
        let count = self.connection_count.fetch_add(1, Ordering::SeqCst);

        if count == 0 {
            // First device connecting - open the port
            debug!("First device connecting, opening serial port");

            let timeout = Duration::from_secs(self.config.timeout_seconds);

            let pair: SerialPair = self
                .serial_factory
                .open(&self.config.port, self.config.baud_rate, timeout)
                .await?;

            // Store reader and writer
            *self.reader.lock().await = Some(pair.reader);
            *self.writer.lock().await = Some(pair.writer);

            // Verify connection with ping
            self.send_command_internal(PpbaCommand::Ping).await?;

            // Get initial status
            self.refresh_status().await?;
            self.refresh_power_stats().await?;

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
    ///
    /// Decrements the connection reference count. If this is the last connection,
    /// closes the serial port and stops polling.
    pub async fn disconnect(&self) {
        let count = self.connection_count.fetch_sub(1, Ordering::SeqCst);

        if count == 1 {
            // Last device disconnecting - close the port
            debug!("Last device disconnecting, closing serial port");

            // Signal polling loop to stop before waiting for it to finish.
            // This must happen first so the loop sees the flag and exits
            // cleanly, releasing any held locks.
            self.serial_available.store(false, Ordering::SeqCst);
            let _ = self.shutdown_tx.send(true);

            self.stop_polling().await;

            *self.reader.lock().await = None;
            *self.writer.lock().await = None;

            info!("Serial port closed (connection count: 0)");
        } else {
            debug!(
                "Device disconnecting (connection count: {})",
                count.saturating_sub(1)
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

    /// Send a command to the device
    pub async fn send_command(&self, command: PpbaCommand) -> Result<String> {
        if !self.is_available() {
            return Err(PpbaError::NotConnected);
        }

        self.send_command_internal(command).await
    }

    /// Internal command sending (doesn't check connection state)
    async fn send_command_internal(&self, command: PpbaCommand) -> Result<String> {
        let _cmd_guard = self.command_lock.lock().await;
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
            _ => {
                // For set commands, just validate that response matches command
                if !response.starts_with(&command_str) {
                    return Err(PpbaError::Communication(format!(
                        "Expected response starting with '{}', got: {}",
                        command_str, response
                    )));
                }
            }
        }

        Ok(response)
    }

    /// Refresh the device status (PA command)
    pub async fn refresh_status(&self) -> Result<()> {
        let response = self.send_command_internal(PpbaCommand::Status).await?;
        let status = parse_status_response(&response)?;

        let mut cached = self.cached_state.write().await;
        cached.status = Some(status.clone());
        cached.last_update = Some(SystemTime::now());

        // Update sensor means
        cached.temp_mean.add_sample(status.temperature);
        cached.humidity_mean.add_sample(status.humidity);
        cached.dewpoint_mean.add_sample(status.dewpoint);

        Ok(())
    }

    /// Refresh power statistics (PS command)
    pub async fn refresh_power_stats(&self) -> Result<()> {
        let response = self.send_command_internal(PpbaCommand::PowerStats).await?;
        let stats = parse_power_stats_response(&response)?;

        let mut cached = self.cached_state.write().await;
        cached.power_stats = Some(stats);

        Ok(())
    }

    /// Start background polling for status updates
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
                // Wait for the next tick or a shutdown signal, whichever comes first
                tokio::select! {
                    _ = poll_interval.tick() => {}
                    _ = shutdown_rx.changed() => {
                        debug!("Polling stopped: shutdown signal received");
                        break;
                    }
                }

                // Check if still connected
                if !serial_available.load(Ordering::SeqCst) {
                    debug!("Polling stopped: serial port closed");
                    break;
                }

                // Refresh status
                if let Err(e) =
                    Self::poll_status(&command_lock, &reader, &writer, &cached_state).await
                {
                    warn!("Failed to poll PPBA status: {}", e);
                }

                // Refresh power stats
                if let Err(e) =
                    Self::poll_power_stats(&command_lock, &reader, &writer, &cached_state).await
                {
                    warn!("Failed to poll PPBA power stats: {}", e);
                }
            }
        });

        let mut polling_handle = self.polling_handle.lock().await;
        *polling_handle = Some(handle);
    }

    /// Poll status (for use in background task)
    async fn poll_status(
        command_lock: &Arc<Mutex<()>>,
        reader: &Arc<Mutex<Option<Box<dyn SerialReader>>>>,
        writer: &Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
        cached_state: &Arc<RwLock<CachedState>>,
    ) -> Result<()> {
        let _cmd_guard = command_lock.lock().await;
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
        cached.status = Some(status.clone());
        cached.last_update = Some(SystemTime::now());

        // Update sensor means
        cached.temp_mean.add_sample(status.temperature);
        cached.humidity_mean.add_sample(status.humidity);
        cached.dewpoint_mean.add_sample(status.dewpoint);

        Ok(())
    }

    /// Poll power stats (for use in background task)
    async fn poll_power_stats(
        command_lock: &Arc<Mutex<()>>,
        reader: &Arc<Mutex<Option<Box<dyn SerialReader>>>>,
        writer: &Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
        cached_state: &Arc<RwLock<CachedState>>,
    ) -> Result<()> {
        let _cmd_guard = command_lock.lock().await;
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
    ///
    /// Waits for the polling task to exit gracefully with a timeout.
    /// Falls back to aborting the task if it doesn't exit within 5 seconds.
    /// The caller must set `serial_available` to false before calling this
    /// so the polling loop sees the flag and breaks out.
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

    /// Update the averaging period for sensor means
    pub async fn set_averaging_period(&self, period: Duration) {
        let mut cached = self.cached_state.write().await;
        cached.temp_mean.set_window(period);
        cached.humidity_mean.set_window(period);
        cached.dewpoint_mean.set_window(period);
        debug!("Sensor averaging period updated to {:?}", period);
    }

    /// Update USB hub state in cache
    ///
    /// USB hub state is not included in PA status response, so we track it separately
    pub async fn set_usb_hub_state(&self, enabled: bool) {
        let mut cached = self.cached_state.write().await;
        cached.usb_hub_enabled = enabled;
        debug!("USB hub state updated to {}", enabled);
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
