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

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::config::SerialConfig;
use crate::error::{FalconRotatorError, Result};
use crate::io::{SerialPortFactory, SerialReader, SerialWriter};
use crate::protocol::{
    parse_firmware_version, parse_full_status, parse_voltage_raw, validate_echo,
    validate_ping_response, Command, FalconStatus,
};

/// Normalise a degree value into `[0.0, 360.0)`.
///
/// Handles negative deltas (e.g. from `Move(delta < 0)`) by adding a full
/// turn before the second modulo so the result is always non-negative.
fn normalise_deg(deg: f64) -> f64 {
    ((deg % 360.0) + 360.0) % 360.0
}

/// Quantise a degree value to the `MD:nn.nn` wire precision (1/100°) by
/// rounding to two decimal places.
///
/// Without this step, `format!("{:.2}", 359.999)` rounds up to `"360.00"`,
/// which violates the documented `[0, 360)` wire range. Quantising first
/// produces `360.00` as an `f64`, which the subsequent `normalise_deg`
/// call wraps back to `0.0` before formatting — keeping the wire output
/// inside the documented range.
fn quantise_to_wire(deg: f64) -> f64 {
    (deg * 100.0).round() / 100.0
}

/// Shared serial port manager.
pub struct SerialManager {
    config: SerialConfig,
    connection_count: Arc<AtomicU32>,
    serial_available: Arc<AtomicBool>,
    reader: Arc<Mutex<Option<Box<dyn SerialReader>>>>,
    writer: Arc<Mutex<Option<Box<dyn SerialWriter>>>>,
    command_lock: Arc<Mutex<()>>,
    /// Serialises every `connect` / `disconnect` invocation so the
    /// open + handshake pair is atomic against concurrent device
    /// connects / disconnects. Without it, a second `connect` could
    /// race past `fetch_add` and return `Ok` while the first caller's
    /// handshake is still running (and may yet fail).
    connect_lock: Arc<Mutex<()>>,
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
            connect_lock: Arc::new(Mutex::new(())),
            sync_offset: Arc::new(Mutex::new(0.0)),
            target_position: Arc::new(Mutex::new(None)),
            last_limit_detected: Arc::new(Mutex::new(None)),
            serial_factory,
        }
    }

    /// Connect to the serial port (ref-counted; first connect runs the handshake).
    ///
    /// `connect_lock` is held for the entire body so a concurrent
    /// `connect` from the second device cannot return `Ok` before the
    /// first caller's handshake has either succeeded (and flipped
    /// `serial_available` to `true`) or failed (and rolled the refcount
    /// back to 0).
    pub async fn connect(&self) -> Result<()> {
        let _conn_guard = self.connect_lock.lock().await;
        let count = self.connection_count.load(Ordering::SeqCst);

        if count > 0 {
            self.connection_count.store(count + 1, Ordering::SeqCst);
            debug!(
                "Additional device connecting (connection count: {})",
                count + 1
            );
            return Ok(());
        }

        debug!("First device connecting, opening serial port");

        let pair = self
            .serial_factory
            .open(
                &self.config.port,
                self.config.baud_rate,
                self.config.timeout,
            )
            .await
            .map_err(|e| {
                FalconRotatorError::ConnectionFailed(format!("open {}: {e}", self.config.port))
            })?;

        *self.reader.lock().await = Some(pair.reader);
        *self.writer.lock().await = Some(pair.writer);

        if let Err(e) = self.run_handshake().await {
            // Handshake failed: tear the connection down so the next
            // `connect()` retries the open + handshake cleanly instead of
            // short-circuiting on the elevated refcount.
            *self.reader.lock().await = None;
            *self.writer.lock().await = None;
            return Err(e);
        }

        self.connection_count.store(1, Ordering::SeqCst);
        self.serial_available.store(true, Ordering::SeqCst);
        info!(
            "Serial port opened on {} (connection count: 1)",
            self.config.port
        );
        Ok(())
    }

    /// Disconnect from the serial port (ref-counted; last disconnect closes).
    ///
    /// `connect_lock` is held for the entire body so disconnect cannot
    /// race with a concurrent first-connect's handshake. On the final
    /// decrement we additionally take `command_lock` before dropping
    /// the reader/writer so any in-flight `send_command_internal` has
    /// completed its write+read pair — otherwise a racing close could
    /// leave a half-issued command and a stranded response on the wire.
    pub async fn disconnect(&self) {
        let _conn_guard = self.connect_lock.lock().await;
        let count = self.connection_count.load(Ordering::SeqCst);

        if count == 0 {
            debug!("disconnect() called with connection count already at 0");
            return;
        }

        let new_count = count - 1;
        self.connection_count.store(new_count, Ordering::SeqCst);

        if new_count == 0 {
            debug!("Last device disconnecting, closing serial port");
            self.serial_available.store(false, Ordering::SeqCst);
            // Drain any in-flight command before dropping the serial
            // halves. New commands route through `send_command` (public)
            // which has already seen `serial_available = false` and will
            // refuse with `NotConnected`, so this lock is only contended
            // by a command that started before the store above.
            let _cmd_guard = self.command_lock.lock().await;
            *self.reader.lock().await = None;
            *self.writer.lock().await = None;
            *self.sync_offset.lock().await = 0.0;
            *self.target_position.lock().await = None;
            *self.last_limit_detected.lock().await = None;
            info!("Serial port closed (connection count: 0)");
        } else {
            debug!("Device disconnecting (connection count: {})", new_count);
        }
    }

    /// Whether the serial port is currently open.
    pub fn is_available(&self) -> bool {
        self.serial_available.load(Ordering::SeqCst)
    }

    /// Issue a command on the device under the command lock, returning the raw response.
    ///
    /// Public surface for callers that need to address a protocol command
    /// not covered by the higher-level helpers below. Refuses with
    /// `NotConnected` if the port isn't open.
    pub async fn send_command(&self, command: Command) -> Result<String> {
        if !self.is_available() {
            return Err(FalconRotatorError::NotConnected);
        }
        self.send_command_internal(&command).await
    }

    /// Internal command send + read used by the handshake (which runs before
    /// `serial_available` is `true`) and by the higher-level helpers.
    async fn send_command_internal(&self, command: &Command) -> Result<String> {
        let _cmd_guard = self.command_lock.lock().await;
        let command_str = command.to_command_string();
        debug!("Sending command: {}", command_str);

        {
            let mut writer_guard = self.writer.lock().await;
            let writer = writer_guard
                .as_mut()
                .ok_or(FalconRotatorError::NotConnected)?;
            writer.write_message(&command_str).await?;
        }

        let response = {
            let mut reader_guard = self.reader.lock().await;
            let reader = reader_guard
                .as_mut()
                .ok_or(FalconRotatorError::NotConnected)?;
            reader
                .read_line()
                .await?
                .ok_or_else(|| FalconRotatorError::Communication("Connection closed".to_string()))?
        };

        debug!("Received response: {}", response);
        Ok(response)
    }

    /// Connect-time handshake: F# → FV → DR:0 → FA → VS.
    ///
    /// The FA and VS reads are smoke tests — we just want to confirm the
    /// wire format is honoured and that the device responds, so the parsed
    /// results are discarded.
    async fn run_handshake(&self) -> Result<()> {
        // F# — ping
        let resp = self
            .send_command_internal(&Command::Ping)
            .await
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("ping: {e}")))?;
        validate_ping_response(&resp)
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("ping: {e}")))?;

        // FV — firmware version (surfaced at info!)
        let resp = self
            .send_command_internal(&Command::FirmwareVersion)
            .await
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("firmware: {e}")))?;
        let version = parse_firmware_version(&resp)
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("firmware: {e}")))?;
        info!("Falcon firmware v{}", version);

        // DR:0 — force de-rotation off
        let resp = self
            .send_command_internal(&Command::DerotationOff)
            .await
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("derotation: {e}")))?;
        validate_echo(&Command::DerotationOff, &resp)
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("derotation: {e}")))?;

        // FA — smoke test full status (parsed result discarded; no-cache design)
        let resp = self
            .send_command_internal(&Command::FullStatus)
            .await
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("full status: {e}")))?;
        let _ = parse_full_status(&resp)
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("full status: {e}")))?;

        // VS — smoke test voltage
        let resp = self
            .send_command_internal(&Command::Voltage)
            .await
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("voltage: {e}")))?;
        let _ = parse_voltage_raw(&resp)
            .map_err(|e| FalconRotatorError::ConnectionFailed(format!("voltage: {e}")))?;

        Ok(())
    }

    /// Issue `FA` and parse the response; also performs the `limit_detect` edge log.
    ///
    /// Logs `warn!` exactly once on the `None → true` or `Some(false) → true`
    /// transition of `FA.limit_detect`. The state initialises to `None` on
    /// connect, so a fresh connection that reports `limit_detect = 1` on its
    /// very first observation through `read_status` still surfaces the warning.
    pub async fn read_status(&self) -> Result<FalconStatus> {
        let response = self.send_command_internal(&Command::FullStatus).await?;
        let status = parse_full_status(&response)?;

        let target = *self.target_position.lock().await;
        let mut last = self.last_limit_detected.lock().await;
        let rising_edge = match *last {
            None => status.limit_detect,
            Some(prev) => !prev && status.limit_detect,
        };
        *last = Some(status.limit_detect);
        drop(last);

        if rising_edge {
            warn!(
                "Falcon reported limit_detect after move toward {:?}",
                target
            );
        }

        Ok(status)
    }

    /// Issue `VS` and return the raw ADC count.
    pub async fn read_voltage_raw(&self) -> Result<u32> {
        let response = self.send_command_internal(&Command::Voltage).await?;
        parse_voltage_raw(&response)
    }

    /// Move to a mechanical angle. The caller has already applied the sync
    /// offset; this method validates finiteness, quantises to the `MD:nn.nn`
    /// wire precision, normalises into `[0, 360)`, and emits the command.
    ///
    /// The quantise-before-normalise step matters because `format!("{:.2}",
    /// 359.999)` rounds up to `"360.00"`, which violates the documented
    /// `[0, 360)` wire range. Rounding first lets `normalise_deg` wrap that
    /// `360.0` back to `0.0` so the wire output stays in range.
    pub async fn move_mechanical(&self, target_mech_deg: f64) -> Result<()> {
        if !self.is_available() {
            return Err(FalconRotatorError::NotConnected);
        }
        if !target_mech_deg.is_finite() {
            return Err(FalconRotatorError::InvalidValue(format!(
                "move target must be finite, got {target_mech_deg}"
            )));
        }
        let wire_deg = normalise_deg(quantise_to_wire(target_mech_deg));
        let cmd = Command::MoveDeg(wire_deg);
        let response = self.send_command_internal(&cmd).await?;
        validate_echo(&cmd, &response)?;
        Ok(())
    }

    /// Issue `FH`, validate the `FH:1` echo, and clear the stored target.
    pub async fn halt(&self) -> Result<()> {
        if !self.is_available() {
            return Err(FalconRotatorError::NotConnected);
        }
        let response = self.send_command_internal(&Command::Halt).await?;
        validate_echo(&Command::Halt, &response)?;
        *self.target_position.lock().await = None;
        Ok(())
    }

    /// Read `FA` then write `FN:b` iff the device's `motor_reverse` differs.
    ///
    /// EEPROM-wear protection (design doc Reverse semantics): the Falcon
    /// persists `FN:b` to EEPROM on every write, so we read first and skip
    /// the write when the device already reports the requested value.
    pub async fn set_reverse(&self, want: bool) -> Result<()> {
        if !self.is_available() {
            return Err(FalconRotatorError::NotConnected);
        }
        let current = self.read_status().await?.motor_reverse;
        if current == want {
            debug!(
                "set_reverse({}): device already matches, skipping FN write",
                want
            );
            return Ok(());
        }
        let cmd = Command::SetReverse(want);
        let response = self.send_command_internal(&cmd).await?;
        validate_echo(&cmd, &response)?;
        Ok(())
    }

    /// Driver-side sync: store `(sky_deg - mech) mod 360` in `sync_offset`.
    ///
    /// Per the design doc Sync semantics, ASCOM `Sync` must leave
    /// `MechanicalPosition` unchanged, so the offset lives in driver memory
    /// and the Falcon's `SD` command is never issued.
    pub async fn sync(&self, sky_deg: f64) -> Result<()> {
        if !self.is_available() {
            return Err(FalconRotatorError::NotConnected);
        }
        if !sky_deg.is_finite() {
            return Err(FalconRotatorError::InvalidValue(format!(
                "sync target must be finite, got {sky_deg}"
            )));
        }
        let mech = self.read_status().await?.position_deg;
        let offset = normalise_deg(sky_deg - mech);
        *self.sync_offset.lock().await = offset;
        debug!(
            "sync: sky={:.4} mech={:.4} → sync_offset={:.4}",
            sky_deg, mech, offset
        );
        Ok(())
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    // ---- normalise_deg ---------------------------------------------------

    #[test]
    fn normalise_deg_zero_is_zero() {
        assert!((normalise_deg(0.0)).abs() < 1e-9);
    }

    #[test]
    fn normalise_deg_under_360_passthrough() {
        assert!((normalise_deg(180.0) - 180.0).abs() < 1e-9);
    }

    #[test]
    fn normalise_deg_wraps_positive_overflow() {
        assert!((normalise_deg(370.0) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn normalise_deg_wraps_negative_into_positive() {
        assert!((normalise_deg(-10.0) - 350.0).abs() < 1e-9);
    }

    #[test]
    fn normalise_deg_handles_two_turn_overflow() {
        assert!((normalise_deg(720.0)).abs() < 1e-9);
    }

    // ---- quantise_to_wire ------------------------------------------------

    #[test]
    fn quantise_to_wire_rounds_up_to_360_from_just_below() {
        assert!((quantise_to_wire(359.999) - 360.0).abs() < 1e-9);
    }

    #[test]
    fn quantise_to_wire_preserves_two_decimal_values() {
        assert!((quantise_to_wire(123.45) - 123.45).abs() < 1e-9);
    }

    #[test]
    fn quantise_to_wire_then_normalise_keeps_wire_in_range() {
        // Composition is what `move_mechanical` actually uses.
        let v = normalise_deg(quantise_to_wire(359.999));
        assert!((v).abs() < 1e-9, "expected 0.0, got {v}");
        let formatted = format!("{v:.2}");
        assert_eq!(formatted, "0.00");
    }
}

/// Mock-backed integration tests for the SerialManager. These exercise the
/// connect handshake, ref-counting, command dispatch, and driver-side state
/// against the deterministic `MockSerialPortFactory`. Gated on `feature =
/// "mock"` per the qhy-focuser precedent.
#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod mock_tests {
    use super::*;
    use crate::config::Config;
    use crate::mock::MockSerialPortFactory;
    use std::sync::Mutex as StdMutex;
    use tracing::Subscriber;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::Layer;

    fn test_config() -> Config {
        Config {
            serial: SerialConfig {
                port: "/dev/mock".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn manager_with(factory: Arc<MockSerialPortFactory>) -> Arc<SerialManager> {
        Arc::new(SerialManager::new(
            test_config(),
            factory as Arc<dyn SerialPortFactory>,
        ))
    }

    // ---- Connect / disconnect -------------------------------------------

    #[tokio::test]
    async fn test_connect_makes_available() {
        let manager = manager_with(Arc::new(MockSerialPortFactory::default()));
        assert!(!manager.is_available());
        manager.connect().await.unwrap();
        assert!(manager.is_available());
        manager.disconnect().await;
        assert!(!manager.is_available());
    }

    #[tokio::test]
    async fn test_connect_increments_refcount_handshake_runs_once() {
        // First connect runs the handshake (F#, FV, DR:0, FA, VS = 5 commands).
        // A second concurrent connect just bumps the refcount; no extra
        // commands should hit the wire.
        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = manager_with(Arc::clone(&factory));

        manager.connect().await.unwrap();
        let after_first = factory.command_log().await;
        // Pin the exact handshake sequence (per design doc Connection
        // Lifecycle): F# → FV → DR:0 → FA → VS. A future reorder or
        // dropped smoke-test command should fail here loudly, not at
        // the once-unskipped BDD scenario.
        assert_eq!(
            after_first,
            vec![
                "F#".to_string(),
                "FV".to_string(),
                "DR:0".to_string(),
                "FA".to_string(),
                "VS".to_string(),
            ],
            "handshake order must match design doc Connection Lifecycle"
        );

        manager.connect().await.unwrap();
        let after_second = factory.command_log().await;
        assert_eq!(
            after_second, after_first,
            "second connect must not issue any new commands"
        );

        // First disconnect just decrements; port stays open.
        manager.disconnect().await;
        assert!(manager.is_available());

        // Last disconnect closes.
        manager.disconnect().await;
        assert!(!manager.is_available());
    }

    #[tokio::test]
    async fn test_disconnect_underflow_protection() {
        let manager = manager_with(Arc::new(MockSerialPortFactory::default()));
        // Two extra disconnects before any connect should not panic.
        manager.disconnect().await;
        manager.disconnect().await;
        assert!(!manager.is_available());
    }

    #[tokio::test]
    async fn test_disconnect_resets_driver_state() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = manager_with(Arc::clone(&factory));

        manager.connect().await.unwrap();
        manager.set_target_position(123.45).await;
        // Drive the limit_detect tracker so it holds Some(_)
        factory.set_limit_detect(true).await;
        let _ = manager.read_status().await.unwrap();
        // Set a non-zero sync offset.
        factory.set_mech_position_deg(45.0).await;
        manager.sync(90.0).await.unwrap();
        assert!((manager.sync_offset().await - 45.0).abs() < 1e-9);

        manager.disconnect().await;

        // All driver-side state should be reset on full disconnect.
        assert_eq!(manager.target_position().await, None);
        assert!((manager.sync_offset().await).abs() < 1e-9);
        assert_eq!(*manager.last_limit_detected.lock().await, None);
    }

    // ---- Handshake failure paths ---------------------------------------

    /// Factory that always returns a synthetic SerialPort error so we can
    /// exercise the `connect → ConnectionFailed → refcount rollback` path.
    #[derive(Default)]
    struct FailingFactory;

    #[async_trait::async_trait]
    impl SerialPortFactory for FailingFactory {
        async fn open(
            &self,
            _port: &str,
            _baud_rate: u32,
            _timeout: std::time::Duration,
        ) -> Result<crate::io::SerialPair> {
            Err(FalconRotatorError::SerialPort("synthetic".into()))
        }
        async fn port_exists(&self, _port: &str) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn test_connect_open_failure_returns_connection_failed_and_resets_refcount() {
        let manager = Arc::new(SerialManager::new(
            test_config(),
            Arc::new(FailingFactory) as Arc<dyn SerialPortFactory>,
        ));
        let err = manager.connect().await.unwrap_err();
        assert!(
            matches!(err, FalconRotatorError::ConnectionFailed(_)),
            "got {err:?}"
        );
        assert!(!manager.is_available());
        // Second attempt must re-enter the first-time path (refcount was
        // rolled back), not short-circuit on the elevated count.
        let err2 = manager.connect().await.unwrap_err();
        assert!(matches!(err2, FalconRotatorError::ConnectionFailed(_)));
    }

    #[tokio::test]
    async fn test_connect_handshake_ping_failure_rolls_back() {
        // Reader that always returns garbage for the very first read so the
        // ping validator fails. We hand-build the factory so we can control
        // the first response without going through the rich mock.
        #[derive(Default)]
        struct GarbageReader {
            sent: bool,
        }
        #[async_trait::async_trait]
        impl SerialReader for GarbageReader {
            async fn read_line(&mut self) -> Result<Option<String>> {
                self.sent = true;
                Ok(Some("BAD_PREFIX".to_string()))
            }
        }
        struct SinkWriter;
        #[async_trait::async_trait]
        impl SerialWriter for SinkWriter {
            async fn write_message(&mut self, _message: &str) -> Result<()> {
                Ok(())
            }
        }
        struct GarbageFactory;
        #[async_trait::async_trait]
        impl SerialPortFactory for GarbageFactory {
            async fn open(
                &self,
                _port: &str,
                _baud_rate: u32,
                _timeout: std::time::Duration,
            ) -> Result<crate::io::SerialPair> {
                Ok(crate::io::SerialPair {
                    reader: Box::new(GarbageReader::default()),
                    writer: Box::new(SinkWriter),
                })
            }
            async fn port_exists(&self, _port: &str) -> bool {
                true
            }
        }

        let manager = Arc::new(SerialManager::new(
            test_config(),
            Arc::new(GarbageFactory) as Arc<dyn SerialPortFactory>,
        ));
        let err = manager.connect().await.unwrap_err();
        assert!(
            matches!(err, FalconRotatorError::ConnectionFailed(ref msg) if msg.contains("ping")),
            "got {err:?}"
        );
        assert!(!manager.is_available());
        assert!(manager.reader.lock().await.is_none());
    }

    // ---- Command dispatch ----------------------------------------------

    #[tokio::test]
    async fn test_send_command_requires_connection() {
        let manager = manager_with(Arc::new(MockSerialPortFactory::default()));
        let err = manager.send_command(Command::Ping).await.unwrap_err();
        assert!(matches!(err, FalconRotatorError::NotConnected));
    }

    #[tokio::test]
    async fn test_read_status_returns_parsed_status() {
        let factory = Arc::new(MockSerialPortFactory::default());
        factory.set_mech_position_deg(50.0).await;
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();

        let status = manager.read_status().await.unwrap();
        assert!((status.position_deg - 50.0).abs() < 1e-9);
        assert!(!status.is_moving);
    }

    #[tokio::test]
    async fn test_read_voltage_raw_returns_default() {
        let factory = Arc::new(MockSerialPortFactory::default());
        factory.set_voltage_raw(812).await;
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();
        let v = manager.read_voltage_raw().await.unwrap();
        assert_eq!(v, 812);
    }

    // ---- Move / halt ----------------------------------------------------

    #[tokio::test]
    async fn test_move_mechanical_sends_md_with_normalised_angle() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();
        factory.clear_command_log().await;

        manager.move_mechanical(-30.0).await.unwrap(); // → 330°
        let log = factory.command_log().await;
        assert_eq!(log, vec!["MD:330.00".to_string()]);
    }

    #[tokio::test]
    async fn test_move_mechanical_requires_connection() {
        let manager = manager_with(Arc::new(MockSerialPortFactory::default()));
        let err = manager.move_mechanical(45.0).await.unwrap_err();
        assert!(matches!(err, FalconRotatorError::NotConnected));
    }

    #[tokio::test]
    async fn test_move_mechanical_rejects_non_finite() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();
        factory.clear_command_log().await;

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = manager.move_mechanical(bad).await.unwrap_err();
            assert!(
                matches!(err, FalconRotatorError::InvalidValue(_)),
                "expected InvalidValue for {bad}, got {err:?}"
            );
        }
        assert!(
            factory.command_log().await.is_empty(),
            "no command should reach the wire for non-finite targets"
        );
    }

    #[tokio::test]
    async fn test_move_mechanical_wraps_just_under_360_to_zero() {
        // `format!("{:.2}", 359.999)` rounds to "360.00", which would
        // violate the documented [0, 360) wire range. After quantise +
        // normalise the wire should carry "MD:0.00".
        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();
        factory.clear_command_log().await;

        manager.move_mechanical(359.999).await.unwrap();
        assert_eq!(factory.command_log().await, vec!["MD:0.00".to_string()]);
    }

    #[tokio::test]
    async fn test_move_mechanical_exact_360_wraps_to_zero() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();
        factory.clear_command_log().await;

        manager.move_mechanical(360.0).await.unwrap();
        assert_eq!(factory.command_log().await, vec!["MD:0.00".to_string()]);
    }

    #[tokio::test]
    async fn test_halt_sends_fh_and_clears_target() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();
        manager.set_target_position(180.0).await;
        factory.clear_command_log().await;

        manager.halt().await.unwrap();
        assert_eq!(factory.command_log().await, vec!["FH".to_string()]);
        assert_eq!(manager.target_position().await, None);
    }

    #[tokio::test]
    async fn test_halt_requires_connection() {
        let manager = manager_with(Arc::new(MockSerialPortFactory::default()));
        let err = manager.halt().await.unwrap_err();
        assert!(matches!(err, FalconRotatorError::NotConnected));
    }

    // ---- set_reverse (EEPROM-wear protection) --------------------------

    #[tokio::test]
    async fn test_set_reverse_skips_write_when_equal() {
        let factory = Arc::new(MockSerialPortFactory::default());
        factory.set_motor_reverse(true).await;
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();
        factory.clear_command_log().await;

        manager.set_reverse(true).await.unwrap();
        // Only the FA read; no FN write.
        assert_eq!(factory.command_log().await, vec!["FA".to_string()]);
    }

    #[tokio::test]
    async fn test_set_reverse_writes_when_different() {
        let factory = Arc::new(MockSerialPortFactory::default());
        // Mock starts with reverse=false.
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();
        factory.clear_command_log().await;

        manager.set_reverse(true).await.unwrap();
        // Reads FA first, then writes FN:1.
        assert_eq!(
            factory.command_log().await,
            vec!["FA".to_string(), "FN:1".to_string()]
        );
    }

    #[tokio::test]
    async fn test_set_reverse_requires_connection() {
        let manager = manager_with(Arc::new(MockSerialPortFactory::default()));
        let err = manager.set_reverse(true).await.unwrap_err();
        assert!(matches!(err, FalconRotatorError::NotConnected));
    }

    // ---- sync (driver-side offset) -------------------------------------

    #[tokio::test]
    async fn test_sync_offset_arithmetic() {
        let factory = Arc::new(MockSerialPortFactory::default());
        // mech = 120°, sync to 30° → offset = (30 - 120) mod 360 = 270.
        factory.set_mech_position_deg(120.0).await;
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();

        manager.sync(30.0).await.unwrap();
        let offset = manager.sync_offset().await;
        assert!(
            (offset - 270.0).abs() < 1e-9,
            "expected offset 270.0, got {offset}"
        );
    }

    #[tokio::test]
    async fn test_sync_requires_connection() {
        let manager = manager_with(Arc::new(MockSerialPortFactory::default()));
        let err = manager.sync(0.0).await.unwrap_err();
        assert!(matches!(err, FalconRotatorError::NotConnected));
    }

    #[tokio::test]
    async fn test_sync_rejects_non_finite() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();
        let starting_offset = manager.sync_offset().await;

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = manager.sync(bad).await.unwrap_err();
            assert!(
                matches!(err, FalconRotatorError::InvalidValue(_)),
                "expected InvalidValue for {bad}, got {err:?}"
            );
        }
        // Offset must not have been mutated by any of the rejections.
        assert!((manager.sync_offset().await - starting_offset).abs() < 1e-9);
    }

    // ---- limit_detect edge log -----------------------------------------

    /// Tracing layer that counts events at WARN level. We use a counter
    /// rather than capturing the full event text so the assertion stays
    /// resilient to changes in the warning format.
    #[derive(Clone, Default)]
    struct WarnCounter(Arc<StdMutex<u32>>);

    impl<S: Subscriber> Layer<S> for WarnCounter {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            if event.metadata().level() == &tracing::Level::WARN {
                *self.0.lock().unwrap() += 1;
            }
        }
    }

    impl WarnCounter {
        fn count(&self) -> u32 {
            *self.0.lock().unwrap()
        }
    }

    #[tokio::test]
    async fn test_limit_detect_edge_log_fires_once_on_rising_edge() {
        let counter = WarnCounter::default();
        let subscriber = tracing_subscriber::registry().with(counter.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let factory = Arc::new(MockSerialPortFactory::default());
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();

        // First read: limit_detect=false. State: None → Some(false). No warn.
        let _ = manager.read_status().await.unwrap();
        assert_eq!(counter.count(), 0);

        // Flip the device's flag and read again. State: Some(false) →
        // Some(true). One warn fires.
        factory.set_limit_detect(true).await;
        let _ = manager.read_status().await.unwrap();
        assert_eq!(
            counter.count(),
            1,
            "expected exactly one warn on rising edge"
        );

        // Same flag again: Some(true) → Some(true). No new warn.
        let _ = manager.read_status().await.unwrap();
        assert_eq!(counter.count(), 1, "no new warn while flag stays high");
    }

    #[tokio::test]
    async fn test_limit_detect_edge_log_fires_on_first_observation_when_high() {
        let counter = WarnCounter::default();
        let subscriber = tracing_subscriber::registry().with(counter.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let factory = Arc::new(MockSerialPortFactory::default());
        factory.set_limit_detect(true).await;
        let manager = manager_with(Arc::clone(&factory));
        manager.connect().await.unwrap();

        // The handshake's FA doesn't pass through read_status, so the edge
        // tracker is still None when read_status fires for real. None →
        // Some(true) should warn.
        let _ = manager.read_status().await.unwrap();
        assert_eq!(counter.count(), 1);
    }

    // ---- Debug ----------------------------------------------------------

    #[tokio::test]
    async fn test_debug_representation_contains_serial_manager() {
        let manager = manager_with(Arc::new(MockSerialPortFactory::default()));
        let debug_str = format!("{manager:?}");
        assert!(debug_str.contains("SerialManager"));
    }
}
