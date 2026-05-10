//! Feature-gated in-memory mock transport.
//!
//! Simulates the motor controller as a small state machine: accepts
//! `:cmd<axis><payload>\r` frames, maintains per-axis state (position,
//! motion mode, running flag, initialised flag, tracking), and emits
//! well-formed `=...\r` / `!XX\r` replies. Phase 2 wires it through
//! [`crate::ServerBuilder::with_transport_factory`] for the BDD `tests/bdd.rs`
//! harness. Phase 3 will additionally use it from a server-startup
//! integration test (`tests/test_lib.rs`) and the ConformU integration
//! target — neither file exists yet.
//!
//! The mock is deliberately not exposed unless the `mock` feature is on so a
//! production build cannot accidentally pick it up.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use skywatcher_motor_protocol::codec::{
    decode_position, decode_u24, decode_u8, encode_position, encode_u24,
};
use tokio::sync::Mutex;

use crate::config::Config;
use crate::error::{Result, StarAdvError};
use crate::transport::{Transport, TransportFactory};

/// Per-axis simulator state.
#[derive(Debug, Clone, Copy)]
pub struct AxisSimState {
    pub position_ticks: i32,
    pub initialized: bool,
    pub running: bool,
    pub goto: bool,
    pub forward: bool,
    pub fast: bool,
    pub goto_target_ticks: i32,
    pub step_period: u32,
}

impl Default for AxisSimState {
    fn default() -> Self {
        Self {
            position_ticks: 0,
            initialized: false,
            running: false,
            goto: false,
            forward: true,
            fast: false,
            goto_target_ticks: 0,
            step_period: 0,
        }
    }
}

impl AxisSimState {
    /// Pack the running / mode / direction / init flags into the three
    /// hex-digit payload that `:f<axis>` returns.
    ///
    /// Bit layout matches [`skywatcher_motor_protocol::AxisStatus::decode`]:
    /// first nibble carries goto / forward / fast, second carries running,
    /// third carries initialised.
    fn encode_status(self) -> [u8; 3] {
        let mut n0 = 0u8;
        if !self.goto {
            n0 |= 0x1; // tracking-mode flag (clear bit ⇒ goto)
        }
        if self.forward {
            n0 |= 0x2;
        }
        if self.fast {
            n0 |= 0x4;
        }
        let n1 = if self.running { 0x1 } else { 0 };
        let n2 = if self.initialized { 0x1 } else { 0 };
        [nibble_to_hex(n0), nibble_to_hex(n1), nibble_to_hex(n2)]
    }

    /// Advance position by one polling step toward `goto_target_ticks`.
    /// No-op when stopped; clears `running` when the target is reached.
    /// Used by the mock's `:f<axis>` and `:j<axis>` paths so BDD scenarios
    /// can assert "Slewing eventually false."
    fn advance_one_step(&mut self) {
        if !self.running {
            return;
        }
        // Move toward target in chunks proportional to mode. The exact size
        // does not matter; tests only care that motion eventually stops.
        let chunk: i32 = if self.fast { 100_000 } else { 100 };
        let delta = self.goto_target_ticks - self.position_ticks;
        if delta == 0 {
            self.running = false;
            return;
        }
        let step = chunk.min(delta.abs()) * delta.signum();
        self.position_ticks += step;
        if self.position_ticks == self.goto_target_ticks {
            self.running = false;
        }
    }
}

fn nibble_to_hex(n: u8) -> u8 {
    let n = n & 0x0F;
    match n {
        0..=9 => b'0' + n,
        10..=15 => b'A' + (n - 10),
        _ => unreachable!(),
    }
}

/// In-memory mock state machine.
#[derive(Debug)]
pub struct MockMountState {
    pub ra: AxisSimState,
    pub dec: AxisSimState,
    /// Counts per revolution on the RA axis. Defaults to the GTi value
    /// `0x375F00` (3,628,800); tests can override.
    pub cpr_ra: u32,
    /// Counts per revolution on the Dec axis. Defaults to the GTi value
    /// `0x375F00` (3,628,800); tests can override.
    pub cpr_dec: u32,
    /// Timer-interrupt frequency. Defaults to the GTi value `0xF42400`
    /// (≈ 16 MHz).
    pub tmr_freq: u32,
    pub high_speed_ratio_ra: u32,
    pub high_speed_ratio_dec: u32,
    /// Motor-board version. Defaults to `0x03300C` per the GTi probe table
    /// in the design doc (mount-type byte `0x03`, fw `0x30`/`0x0C`).
    pub motor_board_version: u32,
    /// Every command frame received, in arrival order. Tests assert against
    /// this to verify the driver issued the expected wire commands.
    pub command_log: Vec<Vec<u8>>,
}

impl Default for MockMountState {
    fn default() -> Self {
        // Matches the GTi probe table in
        // `docs/references/skywatcher-motor-controller-command-set.md`.
        Self {
            ra: AxisSimState::default(),
            dec: AxisSimState::default(),
            cpr_ra: 0x0037_5F00,
            cpr_dec: 0x0037_5F00,
            tmr_freq: 0x00F4_2400,
            // High-speed ratio is mount-specific and the design doc lists
            // example values (16/32/64) without naming a default. Pick a
            // common one; tests that care will override.
            high_speed_ratio_ra: 32,
            high_speed_ratio_dec: 32,
            motor_board_version: 0x0003_300C,
            command_log: Vec::new(),
        }
    }
}

impl MockMountState {
    fn axis_mut(&mut self, axis: u8) -> Option<&mut AxisSimState> {
        match axis {
            b'1' => Some(&mut self.ra),
            b'2' => Some(&mut self.dec),
            _ => None,
        }
    }

    fn cpr(&self, axis: u8) -> Option<u32> {
        match axis {
            b'1' => Some(self.cpr_ra),
            b'2' => Some(self.cpr_dec),
            _ => None,
        }
    }

    fn high_speed_ratio(&self, axis: u8) -> Option<u32> {
        match axis {
            b'1' => Some(self.high_speed_ratio_ra),
            b'2' => Some(self.high_speed_ratio_dec),
            _ => None,
        }
    }
}

/// Mock transport. Cheap to clone via the inner `Arc<Mutex<_>>`.
#[derive(Debug, Default, Clone)]
pub struct MockTransport {
    pub state: Arc<Mutex<MockMountState>>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Build an `=<payload>\r` success reply. Empty payload → `=\r`.
fn ack_with(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    out.push(b'=');
    out.extend_from_slice(payload);
    out.push(b'\r');
    out
}

/// Build an `!XX\r` mount-error reply.
fn err_reply(code: u8) -> Vec<u8> {
    use skywatcher_motor_protocol::codec::encode_u8;
    let bytes = encode_u8(code);
    vec![b'!', bytes[0], bytes[1], b'\r']
}

#[async_trait]
impl Transport for MockTransport {
    async fn round_trip(&self, request: &[u8], _timeout: Duration) -> Result<Vec<u8>> {
        // Frame validation: must be `:cmd<axis><payload?>\r`. Anything else
        // is the driver's bug — surface it as a transport error so tests
        // catch it.
        if request.len() < 3 || request[0] != b':' || request[request.len() - 1] != b'\r' {
            return Err(StarAdvError::Transport(format!(
                "mock received malformed frame: {request:?}"
            )));
        }
        let cmd = request[1];
        let axis = request[2];
        let payload = &request[3..request.len() - 1];

        let mut state = self.state.lock().await;
        state.command_log.push(request.to_vec());

        // Inquiries (lowercase letters)
        let reply = match cmd {
            b'a' => {
                // CPR per axis (24-bit unsigned)
                state
                    .cpr(axis)
                    .map(|cpr| ack_with(&encode_u24(cpr)))
                    .unwrap_or_else(|| err_reply(0))
            }
            b'b' => {
                // TMR_Freq, axis 1 only.
                if axis == b'1' {
                    ack_with(&encode_u24(state.tmr_freq))
                } else {
                    err_reply(0)
                }
            }
            b'g' => state
                .high_speed_ratio(axis)
                .map(|hsr| ack_with(&encode_u24(hsr)))
                .unwrap_or_else(|| err_reply(0)),
            b'e' => {
                // Motor board version, returned for either axis.
                if axis == b'1' || axis == b'2' {
                    ack_with(&encode_u24(state.motor_board_version))
                } else {
                    err_reply(0)
                }
            }
            b'j' => {
                if let Some(ax) = state.axis_mut(axis) {
                    // Polling-driven motion: every `:j` advances one step.
                    ax.advance_one_step();
                    let pos = ax.position_ticks;
                    ack_with(&encode_position(pos).expect("position in range"))
                } else {
                    err_reply(0)
                }
            }
            b'f' => {
                if let Some(ax) = state.axis_mut(axis) {
                    ax.advance_one_step();
                    let bytes = ax.encode_status();
                    ack_with(&bytes)
                } else {
                    err_reply(0)
                }
            }
            // Setters (uppercase letters)
            b'F' => {
                if let Some(ax) = state.axis_mut(axis) {
                    ax.initialized = true;
                    ack_with(&[])
                } else {
                    err_reply(0)
                }
            }
            b'G' => {
                // Set motion mode: payload is one byte (two hex digits).
                let bytes: [u8; 2] = match payload.try_into() {
                    Ok(b) => b,
                    Err(_) => return Ok(err_reply(1)), // CommandLengthError
                };
                let mode = match decode_u8(bytes) {
                    Ok(m) => m,
                    Err(_) => return Ok(err_reply(3)), // InvalidCharacter
                };
                if let Some(ax) = state.axis_mut(axis) {
                    ax.goto = (mode & 0x10) != 0;
                    ax.fast = (mode & 0x20) != 0;
                    ax.forward = (mode & 0x01) == 0;
                    ack_with(&[])
                } else {
                    err_reply(0)
                }
            }
            b'S' => {
                // Set goto target absolute: 6-byte signed/biased payload.
                let bytes: &[u8; 6] = match payload.try_into() {
                    Ok(b) => b,
                    Err(_) => return Ok(err_reply(1)),
                };
                let ticks = match decode_position(bytes) {
                    Ok(t) => t,
                    Err(_) => return Ok(err_reply(3)),
                };
                if let Some(ax) = state.axis_mut(axis) {
                    ax.goto_target_ticks = ticks;
                    ack_with(&[])
                } else {
                    err_reply(0)
                }
            }
            b'I' => {
                // Set step period: 6-byte u24 payload.
                let bytes: &[u8; 6] = match payload.try_into() {
                    Ok(b) => b,
                    Err(_) => return Ok(err_reply(1)),
                };
                let period = match decode_u24(bytes) {
                    Ok(p) => p,
                    Err(_) => return Ok(err_reply(3)),
                };
                if let Some(ax) = state.axis_mut(axis) {
                    ax.step_period = period;
                    ack_with(&[])
                } else {
                    err_reply(0)
                }
            }
            b'E' => {
                // Sync: write encoder position. 6-byte signed/biased payload.
                let bytes: &[u8; 6] = match payload.try_into() {
                    Ok(b) => b,
                    Err(_) => return Ok(err_reply(1)),
                };
                let ticks = match decode_position(bytes) {
                    Ok(t) => t,
                    Err(_) => return Ok(err_reply(3)),
                };
                if let Some(ax) = state.axis_mut(axis) {
                    ax.position_ticks = ticks;
                    ack_with(&[])
                } else {
                    err_reply(0)
                }
            }
            b'J' => {
                if let Some(ax) = state.axis_mut(axis) {
                    if !ax.initialized {
                        return Ok(err_reply(4)); // NotInitialized
                    }
                    ax.running = true;
                    ack_with(&[])
                } else {
                    err_reply(0)
                }
            }
            b'K' => {
                if let Some(ax) = state.axis_mut(axis) {
                    ax.running = false;
                    ack_with(&[])
                } else {
                    err_reply(0)
                }
            }
            b'L' => {
                if let Some(ax) = state.axis_mut(axis) {
                    ax.running = false;
                    ack_with(&[])
                } else {
                    err_reply(0)
                }
            }
            _ => err_reply(0), // UnknownCommand
        };

        Ok(reply)
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// [`TransportFactory`] that emits a fresh [`MockTransport`] on every open.
/// Phase 2's BDD harness, `tests/test_lib.rs`, and the `conformu`
/// integration target all use this so they never touch real I/O.
#[derive(Debug, Default)]
pub struct MockTransportFactory;

#[async_trait]
impl TransportFactory for MockTransportFactory {
    async fn open(&self, _config: &Config) -> Result<Arc<dyn Transport>> {
        Ok(Arc::new(MockTransport::new()))
    }
}

/// [`TransportFactory`] that returns a clone of a pre-built
/// [`MockTransport`] on every `open` call. The clones share the same
/// `Arc<Mutex<MockMountState>>`, so a test holding the original handle
/// can introspect the live `command_log` after the manager has issued
/// commands through its own clone.
///
/// Used by the unit tests that need to assert on the exact wire frames
/// the driver emitted (e.g. "tracking issues `:G1` then `:I1` then
/// `:J1` in that order").
#[derive(Debug, Clone)]
pub struct CapturingMockFactory {
    pub mock: MockTransport,
}

impl CapturingMockFactory {
    pub fn new() -> Self {
        Self {
            mock: MockTransport::new(),
        }
    }
}

impl Default for CapturingMockFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TransportFactory for CapturingMockFactory {
    async fn open(&self, _config: &Config) -> Result<Arc<dyn Transport>> {
        // Clone shares the inner Arc<Mutex<MockMountState>>, so the
        // outer handle held by the test sees every mutation the
        // manager makes through this returned Arc.
        Ok(Arc::new(self.mock.clone()))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn d() -> Duration {
        Duration::from_millis(100)
    }

    #[test]
    fn axis_sim_state_default_is_at_home_uninitialised_and_stopped() {
        let s = AxisSimState::default();
        assert_eq!(s.position_ticks, 0);
        assert!(!s.initialized);
        assert!(!s.running);
        assert!(!s.goto);
        assert!(s.forward);
        assert!(!s.fast);
        assert_eq!(s.goto_target_ticks, 0);
        assert_eq!(s.step_period, 0);
    }

    #[test]
    fn mock_mount_state_default_seeds_documented_gti_values() {
        // Anchored to the GTi probe table in
        // docs/references/skywatcher-motor-controller-command-set.md.
        // If the GTi firmware ever returns different values, the probe
        // table — and these constants — are what gets updated.
        let s = MockMountState::default();
        assert_eq!(s.cpr_ra, 0x0037_5F00);
        assert_eq!(s.cpr_dec, 0x0037_5F00);
        assert_eq!(s.tmr_freq, 0x00F4_2400);
        assert_eq!(s.motor_board_version, 0x0003_300C);
        assert_eq!(s.high_speed_ratio_ra, 32);
        assert_eq!(s.high_speed_ratio_dec, 32);
    }

    #[tokio::test]
    async fn mock_transport_close_is_a_noop() {
        // Idempotent close lets the ref-counted TransportManager call this
        // freely on every disconnect path.
        let t = MockTransport::new();
        t.close().await.expect("first close");
        t.close().await.expect("second close");
    }

    #[tokio::test]
    async fn round_trip_initialize_acks_and_marks_axis_initialized() {
        let t = MockTransport::new();
        let reply = t.round_trip(b":F1\r", d()).await.unwrap();
        assert_eq!(reply, b"=\r");
        assert!(t.state.lock().await.ra.initialized);
    }

    #[tokio::test]
    async fn round_trip_inquire_cpr_returns_seeded_value() {
        let t = MockTransport::new();
        let reply = t.round_trip(b":a1\r", d()).await.unwrap();
        // GTi default CPR 0x375F00 → encode_u24 → "005F37"
        assert_eq!(reply, b"=005F37\r");
    }

    #[tokio::test]
    async fn round_trip_inquire_position_returns_biased_value() {
        let t = MockTransport::new();
        // Initial position 0 → bias 0x800000 → "000080"
        let reply = t.round_trip(b":j1\r", d()).await.unwrap();
        assert_eq!(reply, b"=000080\r");
    }

    #[tokio::test]
    async fn round_trip_set_motion_mode_then_status_reflects_it() {
        let t = MockTransport::new();
        t.round_trip(b":F1\r", d()).await.unwrap();
        // Goto + fast + forward = 0x30 = "30"
        let reply = t.round_trip(b":G130\r", d()).await.unwrap();
        assert_eq!(reply, b"=\r");
        let state = t.state.lock().await;
        assert!(state.ra.goto);
        assert!(state.ra.fast);
        assert!(state.ra.forward);
        assert!(state.ra.initialized);
    }

    #[tokio::test]
    async fn start_motion_before_initialize_returns_not_initialized() {
        let t = MockTransport::new();
        let reply = t.round_trip(b":J1\r", d()).await.unwrap();
        assert_eq!(reply, b"!04\r");
    }

    #[tokio::test]
    async fn slew_lifecycle_advances_position_to_target_then_stops() {
        let t = MockTransport::new();
        t.round_trip(b":F1\r", d()).await.unwrap();
        // Goto + slow + forward = 0x10 = "10"
        t.round_trip(b":G110\r", d()).await.unwrap();
        // Target encoder ticks = 200 → bias 0x800000+200 = 0x8000C8 → "C80080"
        t.round_trip(b":S1C80080\r", d()).await.unwrap();
        t.round_trip(b":J1\r", d()).await.unwrap();
        // With slow chunk=100, two `:f` polls should reach 200.
        let _ = t.round_trip(b":f1\r", d()).await.unwrap();
        let r2 = t.round_trip(b":f1\r", d()).await.unwrap();
        // After second poll, position = 200, running = false.
        let state = t.state.lock().await;
        assert_eq!(state.ra.position_ticks, 200);
        assert!(!state.ra.running);
        // Status reply's middle nibble (running flag) is 0.
        assert_eq!(&r2[2..3], b"0");
    }

    #[tokio::test]
    async fn round_trip_logs_every_request() {
        let t = MockTransport::new();
        t.round_trip(b":F1\r", d()).await.unwrap();
        t.round_trip(b":F2\r", d()).await.unwrap();
        let log = &t.state.lock().await.command_log;
        assert_eq!(log.len(), 2);
        assert_eq!(log[0], b":F1\r");
        assert_eq!(log[1], b":F2\r");
    }

    #[tokio::test]
    async fn round_trip_rejects_malformed_frames() {
        let t = MockTransport::new();
        // No leading `:`
        assert!(t.round_trip(b"F1\r", d()).await.is_err());
        // No trailing `\r`
        assert!(t.round_trip(b":F1", d()).await.is_err());
        // Too short
        assert!(t.round_trip(b":\r", d()).await.is_err());
    }

    #[tokio::test]
    async fn unknown_command_letter_returns_unknown_command_error() {
        let t = MockTransport::new();
        let reply = t.round_trip(b":Z1\r", d()).await.unwrap();
        assert_eq!(reply, b"!00\r");
    }
}
