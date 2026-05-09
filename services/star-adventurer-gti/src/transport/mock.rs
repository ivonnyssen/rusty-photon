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
#[derive(Debug, Default)]
pub struct MockMountState {
    pub ra: AxisSimState,
    pub dec: AxisSimState,
    /// `0x375F00` (3,628,800) is the GTi default; tests can override.
    pub cpr_ra: u32,
    pub cpr_dec: u32,
    /// `0xF42400` (≈ 16 MHz) is the GTi default.
    pub tmr_freq: u32,
    pub high_speed_ratio_ra: u32,
    pub high_speed_ratio_dec: u32,
    /// `0x03300C` matches the GTi probe table in the design doc.
    pub motor_board_version: u32,
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
