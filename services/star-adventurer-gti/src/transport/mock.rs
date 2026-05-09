//! Feature-gated in-memory mock transport.
//!
//! Simulates the motor controller as a small state machine: accepts
//! `:cmd<axis><payload>\r` frames, maintains per-axis state (position,
//! motion mode, running flag, initialised flag, tracking), and emits
//! well-formed `=...\r` / `!XX\r` replies. Used by:
//!
//! * BDD tests (via [`crate::ServerBuilder::with_transport`])
//! * `tests/test_lib.rs` server-startup tests
//! * the `conformu` integration target
//!
//! The mock is deliberately not exposed unless the `mock` feature is on so a
//! production build cannot accidentally pick it up.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::error::Result;
use crate::transport::Transport;

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

#[async_trait]
impl Transport for MockTransport {
    async fn round_trip(&self, _request: &[u8], _timeout: Duration) -> Result<Vec<u8>> {
        unimplemented!("Phase 3: parse :cmd<axis><payload>\\r, mutate state, emit =/! reply")
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}
