//! Ref-counted transport with background polling and parameter cache.
//!
//! The first `Connected = true` opens the transport, runs the init handshake,
//! seeds the parameter cache (CPR per axis, TMR_Freq, high-speed ratio,
//! motor-board version), and starts a background task polling `:f<axis>`
//! and `:j<axis>` at `polling_interval`. Subsequent connects bump the
//! reference count without re-opening; the last disconnect tears everything
//! down.
//!
//! Same pattern as `qhy-focuser::SerialManager` and
//! `ppba-driver::SerialManager`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::config::Config;
use crate::error::Result;
use crate::transport::Transport;

/// Snapshot of the values the mount reports during the init handshake. All
/// 24-bit unsigned wire values; meaningful units are in the design doc.
#[derive(Debug, Clone, Copy, Default)]
pub struct MountParameters {
    pub cpr_ra: u32,
    pub cpr_dec: u32,
    pub tmr_freq: u32,
    pub high_speed_ratio_ra: u32,
    pub high_speed_ratio_dec: u32,
    pub motor_board_version: u32,
}

/// Latest poll-loop snapshot. Updated by the background task at
/// `polling_interval`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AxisSnapshot {
    pub position_ticks: i32,
    pub running: bool,
    pub goto: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MountSnapshot {
    pub ra: AxisSnapshot,
    pub dec: AxisSnapshot,
}

/// Shared, ref-counted transport handle.
pub struct TransportManager {
    _config: Config,
    transport: Arc<dyn Transport>,
    connection_count: AtomicU32,
    parameters: RwLock<Option<MountParameters>>,
    snapshot: RwLock<MountSnapshot>,
}

impl TransportManager {
    pub fn new(config: Config, transport: Arc<dyn Transport>) -> Self {
        Self {
            _config: config,
            transport,
            connection_count: AtomicU32::new(0),
            parameters: RwLock::new(None),
            snapshot: RwLock::new(MountSnapshot::default()),
        }
    }

    /// Reference-counted connect. Returns immediately on success; first
    /// caller pays the init-handshake latency.
    pub async fn connect(&self) -> Result<()> {
        let _ = self.connection_count.fetch_add(1, Ordering::SeqCst);
        unimplemented!(
            "Phase 3: open transport once, run :F/:a/:b/:g/:e/:j handshake, spawn poll task"
        )
    }

    /// Reference-counted disconnect. Last caller out triggers teardown.
    pub async fn disconnect(&self) -> Result<()> {
        unimplemented!(
            "Phase 3: decrement count, on zero stop poll task, abort motion, close transport"
        )
    }

    /// `true` when the transport is currently open (handshake completed and
    /// not yet torn down).
    pub fn is_available(&self) -> bool {
        self.connection_count.load(Ordering::SeqCst) > 0
    }

    /// Latest cached parameters. `None` until handshake completes.
    pub async fn parameters(&self) -> Option<MountParameters> {
        *self.parameters.read().await
    }

    /// Latest poll-loop snapshot.
    pub async fn snapshot(&self) -> MountSnapshot {
        *self.snapshot.read().await
    }

    /// Send one command, return one reply. Does *not* update the snapshot —
    /// the background poller owns that responsibility.
    pub async fn send(
        &self,
        _command: skywatcher_motor_protocol::Command,
    ) -> Result<skywatcher_motor_protocol::Response> {
        let _ = &self.transport; // silence unused
        unimplemented!("Phase 3: encode -> transport.round_trip -> Response::decode")
    }
}
