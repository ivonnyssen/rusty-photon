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

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use crate::config::Config;
use crate::error::Result;
use crate::transport::{Transport, TransportFactory};

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
///
/// Owns a [`TransportFactory`] (not a pre-built [`Transport`]) so the
/// 0→1 connect transition can call `factory.open(&config)` and the 1→0
/// disconnect transition can drop the resulting `Arc<dyn Transport>` to
/// trigger its [`Transport::close`]. This is the qhy-focuser pattern,
/// adapted for the Sky-Watcher protocol.
pub struct TransportManager {
    config: Config,
    factory: Arc<dyn TransportFactory>,
    /// Active transport handle. `None` when no client is connected; set
    /// to `Some` by the 0→1 connect transition and cleared by the 1→0
    /// disconnect transition.
    transport: Mutex<Option<Arc<dyn Transport>>>,
    /// Number of clients that currently believe themselves connected.
    /// Incremented on the way into [`connect`] and decremented on the way
    /// out of [`disconnect`]; the actual transport open/close is gated on
    /// the count crossing 0.
    connection_count: AtomicU32,
    /// Set to `true` only after [`connect`] finishes the init handshake;
    /// cleared on [`disconnect`] before the transport is closed. Read by
    /// [`is_available`] so a connect-in-flight (handshake still running)
    /// or a connect that failed mid-handshake does not falsely advertise
    /// the transport as ready. Same pattern as
    /// `qhy-focuser::SerialManager::serial_available`.
    available: AtomicBool,
    parameters: RwLock<Option<MountParameters>>,
    snapshot: RwLock<MountSnapshot>,
}

impl TransportManager {
    pub fn new(config: Config, factory: Arc<dyn TransportFactory>) -> Self {
        Self {
            config,
            factory,
            transport: Mutex::new(None),
            connection_count: AtomicU32::new(0),
            available: AtomicBool::new(false),
            parameters: RwLock::new(None),
            snapshot: RwLock::new(MountSnapshot::default()),
        }
    }

    /// Reference-counted connect. Returns immediately on success; first
    /// caller pays the init-handshake latency. Sets [`is_available`] to
    /// true only after the handshake completes.
    ///
    /// On the 0→1 transition, calls `factory.open(&config)` to get a
    /// fresh [`Transport`], then runs the init handshake. Phase 3 fills
    /// the handshake body in.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn connect(&self) -> Result<()> {
        let prior = self.connection_count.fetch_add(1, Ordering::SeqCst);
        if prior == 0 {
            // 0→1: actually open the transport.
            let transport = self.factory.open(&self.config).await?;
            *self.transport.lock().await = Some(transport);
        }
        unimplemented!(
            "Phase 3: run :F/:a/:b/:g/:e/:j handshake against the open transport, \
             then self.available.store(true, Ordering::SeqCst), spawn poll task"
        )
    }

    /// Reference-counted disconnect. Last caller out triggers teardown:
    /// clears [`is_available`] before stopping the poll task and closing
    /// the transport.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn disconnect(&self) -> Result<()> {
        unimplemented!(
            "Phase 3: decrement count; on zero, self.available.store(false, ..), \
             stop poll task, abort motion, *self.transport.lock().await = None \
             (drops the Arc, triggering Transport::close)"
        )
    }

    /// `true` only when the underlying transport is open AND the init
    /// handshake has succeeded — i.e. the manager is ready to round-trip
    /// commands. Returns `false` while a connect is mid-handshake or after
    /// a handshake-failure rollback.
    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
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
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn send(
        &self,
        _command: skywatcher_motor_protocol::Command,
    ) -> Result<skywatcher_motor_protocol::Response> {
        unimplemented!("Phase 3: encode -> transport.round_trip -> Response::decode")
    }
}

#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::transport::mock::MockTransportFactory;

    fn manager() -> TransportManager {
        TransportManager::new(Config::default(), Arc::new(MockTransportFactory))
    }

    #[test]
    fn new_starts_unavailable_and_idle() {
        // A fresh manager must report unavailable: nothing has connected
        // and the handshake has not run, so callers must treat the
        // transport as not-yet-ready (and ASCOM `Connected` returns false).
        let m = manager();
        assert!(!m.is_available());
        assert_eq!(m.connection_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn parameters_are_none_before_handshake() {
        // Handshake populates the parameter cache; before that, every
        // caller must see None so coordinate math does not run with bogus
        // CPRs.
        let m = manager();
        assert!(m.parameters().await.is_none());
    }

    #[tokio::test]
    async fn snapshot_is_default_before_polling() {
        let m = manager();
        let snap = m.snapshot().await;
        assert_eq!(snap.ra.position_ticks, 0);
        assert_eq!(snap.dec.position_ticks, 0);
        assert!(!snap.ra.running);
        assert!(!snap.dec.running);
    }
}
