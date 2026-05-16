//! Shared serial port manager for the Falcon Rotator
//!
//! Ref-counted connect/disconnect, per-command serialisation lock, and the
//! three pieces of driver-side state the design doc pins:
//!
//! - `sync_offset` — sky-vs-mechanical correction (set by `Sync`, reset on
//!   reconnect; ASCOM Sync is driver-side only, never sent to the device).
//! - `target_position` — sky-coordinate target of the last `Move*` request,
//!   surfaced by `Rotator::TargetPosition`.
//! - `last_limit_detected` — used by `read_status` to log a `warn!` on the
//!   rising edge of `FA.limit_detect`.

use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::config::SerialConfig;
use crate::error::Result;
use crate::io::{SerialPortFactory, SerialReader, SerialWriter};
use crate::protocol::{Command, FalconStatus};

/// Shared serial port manager.
pub struct SerialManager {
    config: SerialConfig,
    connection_count: Arc<AtomicU32>,
    serial_available: Arc<AtomicBool>,
    reader: Arc<Mutex<Option<Box<dyn SerialReader>>>>,
    writer: Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
    command_lock: Arc<Mutex<()>>,
    sync_offset: Arc<Mutex<f64>>,
    target_position: Arc<Mutex<Option<f64>>>,
    last_limit_detected: Arc<Mutex<Option<bool>>>,
    serial_factory: Arc<dyn SerialPortFactory>,
}

impl SerialManager {
    /// Construct a new manager bound to the given serial config and factory.
    pub fn new(config: crate::config::Config, serial_factory: Arc<dyn SerialPortFactory>) -> Self {
        Self {
            config: config.serial,
            connection_count: Arc::new(AtomicU32::new(0)),
            serial_available: Arc::new(AtomicBool::new(false)),
            reader: Arc::new(Mutex::new(None)),
            writer: Arc::new(Mutex::new(None)),
            command_lock: Arc::new(Mutex::new(())),
            sync_offset: Arc::new(Mutex::new(0.0)),
            target_position: Arc::new(Mutex::new(None)),
            last_limit_detected: Arc::new(Mutex::new(None)),
            serial_factory,
        }
    }

    /// Connect to the serial port (ref-counted; first connect runs the handshake).
    pub async fn connect(&self) -> Result<()> {
        let _ = (&self.config, &self.serial_factory);
        unimplemented!("SerialManager::connect is implemented in Phase 3c")
    }

    /// Disconnect from the serial port (ref-counted; last disconnect closes).
    pub async fn disconnect(&self) {
        unimplemented!("SerialManager::disconnect is implemented in Phase 3c")
    }

    /// Whether the serial port is currently open.
    pub fn is_available(&self) -> bool {
        self.serial_available
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Issue a command on the device under the command lock, returning the raw response.
    pub async fn send_command(&self, _command: Command) -> Result<String> {
        let _ = (&self.reader, &self.writer, &self.command_lock);
        unimplemented!("SerialManager::send_command is implemented in Phase 3c")
    }

    /// Issue `FA` and parse the response; also performs the `limit_detect` edge log.
    pub async fn read_status(&self) -> Result<FalconStatus> {
        let _ = &self.last_limit_detected;
        unimplemented!("SerialManager::read_status is implemented in Phase 3c")
    }

    /// Issue `VS` and return the raw ADC count.
    pub async fn read_voltage_raw(&self) -> Result<u32> {
        unimplemented!("SerialManager::read_voltage_raw is implemented in Phase 3c")
    }

    /// Move to a mechanical angle (caller has already normalised + applied offset).
    pub async fn move_mechanical(&self, _target_mech_deg: f64) -> Result<()> {
        unimplemented!("SerialManager::move_mechanical is implemented in Phase 3c")
    }

    /// Issue `FH`, validate the echo, and clear `target_position`.
    pub async fn halt(&self) -> Result<()> {
        unimplemented!("SerialManager::halt is implemented in Phase 3c")
    }

    /// Read `FA` then write `FN:b` iff the device's `motor_reverse` differs.
    pub async fn set_reverse(&self, _want: bool) -> Result<()> {
        unimplemented!("SerialManager::set_reverse is implemented in Phase 3c")
    }

    /// Driver-side sync: store `(sky_deg - mech) mod 360` in `sync_offset`.
    pub async fn sync(&self, _sky_deg: f64) -> Result<()> {
        let _ = &self.sync_offset;
        unimplemented!("SerialManager::sync is implemented in Phase 3c")
    }

    /// Read the current driver-side sync offset.
    pub async fn sync_offset(&self) -> f64 {
        *self.sync_offset.lock().await
    }

    /// Store the last-requested sky-coordinate target.
    pub async fn set_target_position(&self, sky_deg: f64) {
        *self.target_position.lock().await = Some(sky_deg);
    }

    /// Clear the stored target (used by `Halt`).
    pub async fn clear_target_position(&self) {
        *self.target_position.lock().await = None;
    }

    /// Read the last-requested sky-coordinate target.
    pub async fn target_position(&self) -> Option<f64> {
        *self.target_position.lock().await
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
