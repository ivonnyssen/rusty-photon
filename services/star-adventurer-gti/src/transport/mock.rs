//! Feature-gated in-memory mock transport.
//!
//! Simulates the motor controller as a small state machine: accepts
//! `:cmd<axis><payload>\r` frames, maintains per-axis state (position,
//! motion mode, running flag, initialised flag, tracking), and emits
//! well-formed `=...\r` / `!XX\r` replies. Phase 2 wires it through
//! [`crate::ServerBuilder::with_transport`] for the BDD `tests/bdd.rs`
//! harness. Phase 3 will additionally use it from a server-startup
//! integration test (`tests/test_lib.rs`) and the ConformU integration
//! target — neither file exists yet.
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

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
}
