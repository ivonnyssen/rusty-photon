//! Mock serial port for testing without real hardware.
//!
//! Feature-gated under `mock`. The mock implements a deterministic state
//! machine that responds to every Falcon command from the design-doc
//! [Command Table](../../../docs/services/falcon-rotator.md#command-table)
//! and tracks `is_moving` / position / reverse / derotation state across
//! commands. Tests inspect `command_log` to assert wire traffic.
//!
//! `FF` (firmware reload) is deliberately rejected with an error sentinel —
//! the design doc forbids the driver from ever issuing it, and the mock
//! refuses to silently accept it so a regression in protocol routing fails
//! loudly rather than passing because the mock was permissive.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::debug;

use crate::error::Result;
use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};

/// Steps per degree (vendor product page).
const STEPS_PER_DEGREE: f64 = 86.6;

/// Default raw ADC voltage reading returned by `VS`.
const DEFAULT_VOLTAGE_RAW: u32 = 800;

/// Default firmware version reported by `FV`.
const DEFAULT_FIRMWARE_VERSION: &str = "1.3";

/// Sentinel returned for commands the driver should never issue (e.g. `FF`)
/// or that the mock does not understand. The driver itself doesn't parse this
/// — it is a "loud" response that surfaces as `InvalidResponse` and trips a
/// failing test.
const UNKNOWN_COMMAND_RESPONSE: &str = "ERR:UNKNOWN";

/// Simulated Falcon Rotator device state.
#[derive(Debug, Clone)]
struct MockDeviceState {
    /// Current mechanical position in degrees (normalised to `[0, 360)`).
    mech_position_deg: f64,
    /// `true` until the next `FA` clears the flag (best-effort BDD model).
    is_moving: bool,
    /// Mirrors the `FN:b` setting; persists across commands.
    motor_reverse: bool,
    /// `true` after `DR:<ms>` where `ms > 0`; cleared by `DR:0`.
    do_derotation: bool,
    /// Mirrors `FA.limit_detect`. Test hooks can set this directly.
    limit_detect: bool,
    /// Raw ADC count returned by `VS`.
    voltage_raw: u32,
    /// String returned by `FV`.
    firmware_version: String,
}

impl Default for MockDeviceState {
    fn default() -> Self {
        Self {
            mech_position_deg: 0.0,
            is_moving: false,
            motor_reverse: false,
            do_derotation: false,
            limit_detect: false,
            voltage_raw: DEFAULT_VOLTAGE_RAW,
            firmware_version: DEFAULT_FIRMWARE_VERSION.to_string(),
        }
    }
}

impl MockDeviceState {
    fn position_steps(&self) -> u32 {
        (self.mech_position_deg * STEPS_PER_DEGREE).round() as u32
    }

    fn full_status_response(&self) -> String {
        format!(
            "FR_OK:{}:{:.2}:{}:{}:{}:{}",
            self.position_steps(),
            self.mech_position_deg,
            bit(self.is_moving),
            bit(self.limit_detect),
            bit(self.do_derotation),
            bit(self.motor_reverse),
        )
    }
}

fn bit(b: bool) -> u8 {
    if b {
        1
    } else {
        0
    }
}

fn normalise_deg(deg: f64) -> f64 {
    ((deg % 360.0) + 360.0) % 360.0
}

/// Shared state between mock reader and writer for one open `SerialPair`.
#[derive(Debug, Default)]
struct MockState {
    response_queue: Vec<String>,
    device_state: MockDeviceState,
    command_log: Vec<String>,
}

impl MockState {
    fn process_command(&mut self, raw: &str) {
        let command = raw.trim_end_matches(['\r', '\n']).trim();
        if command.is_empty() {
            return;
        }
        debug!("Mock processing command: '{}'", command);
        self.command_log.push(command.to_string());

        let response = match command {
            "F#" => "FR_OK".to_string(),
            "FA" => {
                let resp = self.device_state.full_status_response();
                // Plan §3b: is_moving "best-effort — flipped by MD:, cleared
                // on next FA after each move". Clearing here makes the
                // sequence MD → FA(moving=1) → FA(moving=0) the BDD path.
                self.device_state.is_moving = false;
                resp
            }
            "FV" => format!("FV:{}", self.device_state.firmware_version),
            "FD" => format!("FD:{:.2}", self.device_state.mech_position_deg),
            "FP" => format!("FP:{}", self.device_state.position_steps()),
            "VS" => format!("VS:{}", self.device_state.voltage_raw),
            "FH" => {
                self.device_state.is_moving = false;
                "FH:1".to_string()
            }
            "FR" => format!("FR:{}", bit(self.device_state.is_moving)),
            "FF" => UNKNOWN_COMMAND_RESPONSE.to_string(),
            other if other.starts_with("DR:") => self.process_derotation(other),
            other if other.starts_with("MD:") => self.process_move_deg(other),
            other if other.starts_with("MS:") => self.process_move_steps(other),
            other if other.starts_with("FN:") => self.process_reverse(other),
            _ => UNKNOWN_COMMAND_RESPONSE.to_string(),
        };

        debug!("Mock queuing response: '{}'", response);
        self.response_queue.push(response);
    }

    fn process_derotation(&mut self, command: &str) -> String {
        let value = command.strip_prefix("DR:").unwrap_or("");
        match value.parse::<u32>() {
            Ok(0) => {
                self.device_state.do_derotation = false;
                "DR:0".to_string()
            }
            Ok(ms) => {
                self.device_state.do_derotation = true;
                format!("DR:{ms}")
            }
            Err(_) => UNKNOWN_COMMAND_RESPONSE.to_string(),
        }
    }

    fn process_move_deg(&mut self, command: &str) -> String {
        let value = command.strip_prefix("MD:").unwrap_or("");
        match value.parse::<f64>() {
            Ok(deg) if deg.is_finite() => {
                self.device_state.mech_position_deg = normalise_deg(deg);
                self.device_state.is_moving = true;
                // Echo the command literally so the driver's `validate_echo`
                // sees what it sent regardless of the mock's precision quirks.
                command.to_string()
            }
            _ => UNKNOWN_COMMAND_RESPONSE.to_string(),
        }
    }

    fn process_move_steps(&mut self, command: &str) -> String {
        let value = command.strip_prefix("MS:").unwrap_or("");
        match value.parse::<u32>() {
            Ok(steps) => {
                let deg = f64::from(steps) / STEPS_PER_DEGREE;
                self.device_state.mech_position_deg = normalise_deg(deg);
                self.device_state.is_moving = true;
                format!("MS:{steps}")
            }
            Err(_) => UNKNOWN_COMMAND_RESPONSE.to_string(),
        }
    }

    fn process_reverse(&mut self, command: &str) -> String {
        let value = command.strip_prefix("FN:").unwrap_or("");
        match value {
            "0" => {
                self.device_state.motor_reverse = false;
                "FN:0".to_string()
            }
            "1" => {
                self.device_state.motor_reverse = true;
                "FN:1".to_string()
            }
            _ => UNKNOWN_COMMAND_RESPONSE.to_string(),
        }
    }

    fn next_response(&mut self) -> Option<String> {
        if self.response_queue.is_empty() {
            None
        } else {
            Some(self.response_queue.remove(0))
        }
    }
}

/// Mock serial reader that pops queued responses produced by the writer.
pub struct MockSerialReader {
    state: Arc<Mutex<MockState>>,
}

impl MockSerialReader {
    fn new(state: Arc<Mutex<MockState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl SerialReader for MockSerialReader {
    async fn read_line(&mut self) -> Result<Option<String>> {
        let mut state = self.state.lock().await;
        match state.next_response() {
            Some(resp) => {
                debug!("Mock serial read: '{}'", resp);
                Ok(Some(resp))
            }
            None => {
                debug!("Mock serial read: NO RESPONSE QUEUED");
                Ok(None)
            }
        }
    }
}

/// Mock serial writer that drives the state machine and queues responses.
pub struct MockSerialWriter {
    state: Arc<Mutex<MockState>>,
}

impl MockSerialWriter {
    fn new(state: Arc<Mutex<MockState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl SerialWriter for MockSerialWriter {
    async fn write_message(&mut self, message: &str) -> Result<()> {
        debug!("Mock serial write: {}", message);
        let mut state = self.state.lock().await;
        state.process_command(message);
        Ok(())
    }
}

/// Mock serial port factory.
///
/// Maintains persistent state across multiple `open` cycles so reconnect
/// scenarios start from where the previous session left off — mirroring real
/// hardware where the Falcon's EEPROM keeps its `motor_reverse` setting and
/// the mechanical position survives a power cycle.
#[derive(Clone, Debug, Default)]
pub struct MockSerialPortFactory {
    state: Arc<Mutex<MockState>>,
}

#[async_trait]
impl SerialPortFactory for MockSerialPortFactory {
    async fn open(&self, port: &str, baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        debug!("Mock serial port opened: {} at {} baud", port, baud_rate);
        let state = Arc::clone(&self.state);
        Ok(SerialPair {
            reader: Box::new(MockSerialReader::new(Arc::clone(&state))),
            writer: Box::new(MockSerialWriter::new(state)),
        })
    }

    async fn port_exists(&self, _port: &str) -> bool {
        true
    }
}

impl MockSerialPortFactory {
    /// Seed the mock's mechanical position before opening a connection.
    pub async fn set_mech_position_deg(&self, value: f64) {
        self.state.lock().await.device_state.mech_position_deg = normalise_deg(value);
    }

    /// Read the mock's current mechanical position. Used by tests that
    /// verify a code path *did not* mutate the device counter (e.g. ASCOM
    /// Sync, which must leave MechanicalPosition untouched).
    pub async fn mech_position_deg(&self) -> f64 {
        self.state.lock().await.device_state.mech_position_deg
    }

    /// Seed the mock's `motor_reverse` flag (mirrors EEPROM persistence).
    pub async fn set_motor_reverse(&self, value: bool) {
        self.state.lock().await.device_state.motor_reverse = value;
    }

    /// Seed the mock's `limit_detect` flag visible to the next `FA`.
    pub async fn set_limit_detect(&self, value: bool) {
        self.state.lock().await.device_state.limit_detect = value;
    }

    /// Seed the raw ADC count returned by `VS`.
    pub async fn set_voltage_raw(&self, value: u32) {
        self.state.lock().await.device_state.voltage_raw = value;
    }

    /// Snapshot every command the writer has seen, in order received.
    pub async fn command_log(&self) -> Vec<String> {
        self.state.lock().await.command_log.clone()
    }

    /// Clear the recorded command log. Useful between handshake and the
    /// command-under-test in unit tests so assertions only see the latter.
    pub async fn clear_command_log(&self) {
        self.state.lock().await.command_log.clear();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    async fn fresh_pair() -> (MockSerialWriter, MockSerialReader) {
        let state = Arc::new(Mutex::new(MockState::default()));
        (
            MockSerialWriter::new(Arc::clone(&state)),
            MockSerialReader::new(state),
        )
    }

    async fn round_trip(
        writer: &mut MockSerialWriter,
        reader: &mut MockSerialReader,
        cmd: &str,
    ) -> String {
        writer.write_message(cmd).await.unwrap();
        reader.read_line().await.unwrap().unwrap()
    }

    // ---- Helpers ---------------------------------------------------------

    #[test]
    fn normalise_deg_wraps_positive_overflow() {
        assert!((normalise_deg(370.0) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn normalise_deg_wraps_negative_into_positive() {
        assert!((normalise_deg(-10.0) - 350.0).abs() < 1e-9);
    }

    #[test]
    fn bit_maps_true_and_false() {
        assert_eq!(bit(true), 1);
        assert_eq!(bit(false), 0);
    }

    // ---- Per-command tests ----------------------------------------------

    #[tokio::test]
    async fn ping_returns_fr_ok() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(round_trip(&mut writer, &mut reader, "F#").await, "FR_OK");
    }

    #[tokio::test]
    async fn firmware_version_returns_fv_one_three() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(round_trip(&mut writer, &mut reader, "FV").await, "FV:1.3");
    }

    #[tokio::test]
    async fn full_status_default_shape() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(
            round_trip(&mut writer, &mut reader, "FA").await,
            "FR_OK:0:0.00:0:0:0:0"
        );
    }

    #[tokio::test]
    async fn move_deg_updates_position_and_flags_is_moving() {
        let (mut writer, mut reader) = fresh_pair().await;
        let echo = round_trip(&mut writer, &mut reader, "MD:50.00").await;
        assert_eq!(echo, "MD:50.00");

        // First FA after the move sees is_moving=1
        let first = round_trip(&mut writer, &mut reader, "FA").await;
        assert!(
            first.contains(":1:0:0:0"),
            "expected moving flag set: {first}"
        );
        assert!(
            first.contains(":50.00:"),
            "expected position 50.00: {first}"
        );

        // Second FA returns is_moving=0
        let second = round_trip(&mut writer, &mut reader, "FA").await;
        assert!(
            second.contains(":0:0:0:0"),
            "expected moving flag cleared on second poll: {second}"
        );
    }

    #[tokio::test]
    async fn move_steps_converts_via_steps_per_degree() {
        let (mut writer, mut reader) = fresh_pair().await;
        // 8660 steps / 86.6 = 100°
        let echo = round_trip(&mut writer, &mut reader, "MS:8660").await;
        assert_eq!(echo, "MS:8660");

        // Drain the is_moving=1 read first.
        let _ = round_trip(&mut writer, &mut reader, "FA").await;

        let pos = round_trip(&mut writer, &mut reader, "FD").await;
        assert_eq!(pos, "FD:100.00");
    }

    #[tokio::test]
    async fn halt_clears_is_moving_and_echoes_fh_one() {
        let (mut writer, mut reader) = fresh_pair().await;
        // Kick a move so is_moving is set.
        let _ = round_trip(&mut writer, &mut reader, "MD:90.00").await;

        let echo = round_trip(&mut writer, &mut reader, "FH").await;
        assert_eq!(echo, "FH:1");

        let status = round_trip(&mut writer, &mut reader, "FA").await;
        assert!(
            status.contains(":0:0:0:0"),
            "expected is_moving cleared after halt: {status}"
        );
    }

    #[tokio::test]
    async fn is_running_reports_state() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(round_trip(&mut writer, &mut reader, "FR").await, "FR:0");

        let _ = round_trip(&mut writer, &mut reader, "MD:10.00").await;
        assert_eq!(round_trip(&mut writer, &mut reader, "FR").await, "FR:1");
    }

    #[tokio::test]
    async fn set_reverse_persists_state() {
        let (mut writer, mut reader) = fresh_pair().await;
        let echo = round_trip(&mut writer, &mut reader, "FN:1").await;
        assert_eq!(echo, "FN:1");

        let status = round_trip(&mut writer, &mut reader, "FA").await;
        assert!(status.ends_with(":1"), "expected motor_reverse=1: {status}");

        let echo = round_trip(&mut writer, &mut reader, "FN:0").await;
        assert_eq!(echo, "FN:0");
        let status = round_trip(&mut writer, &mut reader, "FA").await;
        assert!(status.ends_with(":0"), "expected motor_reverse=0: {status}");
    }

    #[tokio::test]
    async fn derotation_off_then_on_toggles_flag() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(round_trip(&mut writer, &mut reader, "DR:0").await, "DR:0");
        let status = round_trip(&mut writer, &mut reader, "FA").await;
        assert!(
            status.contains(":0:0:0"),
            "expected derotation cleared: {status}"
        );

        assert_eq!(round_trip(&mut writer, &mut reader, "DR:25").await, "DR:25");
        let status = round_trip(&mut writer, &mut reader, "FA").await;
        // Field order: is_moving:limit:derot:reverse → the derot bit is third.
        assert!(
            status.ends_with(":0:0:1:0"),
            "expected derotation set: {status}"
        );
    }

    #[tokio::test]
    async fn voltage_returns_default_raw() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(round_trip(&mut writer, &mut reader, "VS").await, "VS:800");
    }

    #[tokio::test]
    async fn position_deg_and_steps_track_each_other() {
        let (mut writer, mut reader) = fresh_pair().await;
        let _ = round_trip(&mut writer, &mut reader, "MD:50.00").await;
        // Drain is_moving=1.
        let _ = round_trip(&mut writer, &mut reader, "FA").await;

        let deg = round_trip(&mut writer, &mut reader, "FD").await;
        let steps = round_trip(&mut writer, &mut reader, "FP").await;
        assert_eq!(deg, "FD:50.00");
        // 50 * 86.6 = 4330
        assert_eq!(steps, "FP:4330");
    }

    #[tokio::test]
    async fn limit_detect_visible_when_set() {
        let state = Arc::new(Mutex::new(MockState::default()));
        state.lock().await.device_state.limit_detect = true;
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(state);

        let status = round_trip(&mut writer, &mut reader, "FA").await;
        // Field order after FR_OK: steps:deg:moving:limit:derot:reverse
        assert!(
            status.contains(":0:1:0:0"),
            "expected limit bit set: {status}"
        );
    }

    // ---- Defensive paths -------------------------------------------------

    #[tokio::test]
    async fn ff_returns_unknown_sentinel() {
        let (mut writer, mut reader) = fresh_pair().await;
        // The design doc bans the driver from ever issuing FF. The mock
        // refuses to silently accept it so a routing regression fails loudly.
        assert_eq!(
            round_trip(&mut writer, &mut reader, "FF").await,
            UNKNOWN_COMMAND_RESPONSE
        );
    }

    #[tokio::test]
    async fn unknown_command_returns_sentinel() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(
            round_trip(&mut writer, &mut reader, "XX").await,
            UNKNOWN_COMMAND_RESPONSE
        );
    }

    #[tokio::test]
    async fn malformed_move_deg_returns_sentinel() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(
            round_trip(&mut writer, &mut reader, "MD:not_a_float").await,
            UNKNOWN_COMMAND_RESPONSE
        );
    }

    #[tokio::test]
    async fn malformed_derotation_rate_returns_sentinel() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(
            round_trip(&mut writer, &mut reader, "DR:not_a_number").await,
            UNKNOWN_COMMAND_RESPONSE
        );
    }

    #[tokio::test]
    async fn malformed_move_steps_returns_sentinel() {
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(
            round_trip(&mut writer, &mut reader, "MS:not_a_number").await,
            UNKNOWN_COMMAND_RESPONSE
        );
    }

    #[tokio::test]
    async fn malformed_reverse_value_returns_sentinel() {
        // `FN:` accepts only "0" or "1" — anything else falls through to the
        // unknown-command sentinel so a routing regression fails loudly.
        let (mut writer, mut reader) = fresh_pair().await;
        assert_eq!(
            round_trip(&mut writer, &mut reader, "FN:maybe").await,
            UNKNOWN_COMMAND_RESPONSE
        );
    }

    #[tokio::test]
    async fn writer_trims_trailing_newline_from_caller() {
        // The real `TokioSerialWriter` appends `\n`. The mock writer should
        // recognise the command regardless of trailing LF/CR so tests that
        // bypass the protocol layer and write raw bytes still work.
        let (mut writer, mut reader) = fresh_pair().await;
        writer.write_message("F#\r\n").await.unwrap();
        assert_eq!(reader.read_line().await.unwrap().unwrap(), "FR_OK");
    }

    #[tokio::test]
    async fn command_log_records_every_command_in_order() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut writer = MockSerialWriter::new(Arc::clone(&state));
        let mut reader = MockSerialReader::new(Arc::clone(&state));

        let _ = round_trip(&mut writer, &mut reader, "F#").await;
        let _ = round_trip(&mut writer, &mut reader, "FA").await;
        let _ = round_trip(&mut writer, &mut reader, "MD:45.00").await;

        let log = state.lock().await.command_log.clone();
        assert_eq!(
            log,
            vec!["F#".to_string(), "FA".to_string(), "MD:45.00".to_string()]
        );
    }

    #[tokio::test]
    async fn factory_open_returns_working_pair() {
        let factory = MockSerialPortFactory::default();
        let mut pair = factory
            .open("/dev/mock", 9600, Duration::from_secs(1))
            .await
            .unwrap();

        pair.writer.write_message("F#").await.unwrap();
        let resp = pair.reader.read_line().await.unwrap().unwrap();
        assert_eq!(resp, "FR_OK");
    }

    #[tokio::test]
    async fn factory_state_persists_across_reopens() {
        let factory = MockSerialPortFactory::default();

        // First "session": set reverse=1
        let mut pair = factory
            .open("/dev/mock", 9600, Duration::from_secs(1))
            .await
            .unwrap();
        pair.writer.write_message("FN:1").await.unwrap();
        let _ = pair.reader.read_line().await.unwrap();
        drop(pair);

        // Second "session": FA should still report reverse=1
        let mut pair = factory
            .open("/dev/mock", 9600, Duration::from_secs(1))
            .await
            .unwrap();
        pair.writer.write_message("FA").await.unwrap();
        let resp = pair.reader.read_line().await.unwrap().unwrap();
        assert!(
            resp.ends_with(":1"),
            "expected motor_reverse to persist across reopen: {resp}"
        );
    }

    #[tokio::test]
    async fn factory_port_exists_always_true() {
        let factory = MockSerialPortFactory::default();
        assert!(factory.port_exists("/dev/whatever").await);
    }

    #[tokio::test]
    async fn empty_command_is_ignored() {
        let (mut writer, mut reader) = fresh_pair().await;
        writer.write_message("\n").await.unwrap();
        // No response queued for an empty command.
        assert!(reader.read_line().await.unwrap().is_none());
    }
}
