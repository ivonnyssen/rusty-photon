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
use std::time::Duration;

use skywatcher_motor_protocol::{Axis, Command, Response};
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::config::{Config, TransportConfig};
use crate::error::{Result, StarAdvError};
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
    /// Serialises every round-trip — both ad-hoc `send()` calls from
    /// `MountDevice` and the background polling task share the transport
    /// and the protocol has no per-request ID, so concurrent issues would
    /// race their replies.
    command_lock: Arc<Mutex<()>>,
    /// Number of clients that currently believe themselves connected.
    connection_count: AtomicU32,
    /// Set to `true` only after [`connect`] finishes the init handshake;
    /// cleared on [`disconnect`] before the transport is closed. Read by
    /// [`is_available`] so a connect-in-flight (handshake still running)
    /// or a connect that failed mid-handshake does not falsely advertise
    /// the transport as ready. Same pattern as
    /// `qhy-focuser::SerialManager::serial_available`.
    available: AtomicBool,
    parameters: RwLock<Option<MountParameters>>,
    snapshot: Arc<RwLock<MountSnapshot>>,
    /// Background polling task; populated on the 0→1 connect transition,
    /// aborted on the 1→0 disconnect.
    poll_handle: Mutex<Option<JoinHandle<()>>>,
    /// Shutdown channel for the polling task. The task watches the
    /// receiver and exits when the value flips to `true`.
    shutdown_tx: watch::Sender<bool>,
}

impl TransportManager {
    pub fn new(config: Config, factory: Arc<dyn TransportFactory>) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            config,
            factory,
            transport: Mutex::new(None),
            command_lock: Arc::new(Mutex::new(())),
            connection_count: AtomicU32::new(0),
            available: AtomicBool::new(false),
            parameters: RwLock::new(None),
            snapshot: Arc::new(RwLock::new(MountSnapshot::default())),
            poll_handle: Mutex::new(None),
            shutdown_tx,
        }
    }

    /// Reference-counted connect. First caller pays the init-handshake
    /// latency. Sets [`is_available`] to true only after the handshake
    /// completes.
    ///
    /// On the 0→1 transition: opens a transport via the factory, runs
    /// `:F`/`:a`/`:b`/`:g`/`:e`/`:j` per the design doc's initialisation
    /// sequence, populates the parameter cache, spawns the polling task.
    pub async fn connect(&self) -> Result<()> {
        let prior = self.connection_count.fetch_add(1, Ordering::SeqCst);
        if prior > 0 {
            // Already connected — just bump the count.
            return Ok(());
        }

        // 0→1: actually open the transport. On any error, roll the count
        // back so a later retry can still succeed.
        let transport = match self.factory.open(&self.config).await {
            Ok(t) => t,
            Err(e) => {
                self.connection_count.fetch_sub(1, Ordering::SeqCst);
                return Err(e);
            }
        };
        *self.transport.lock().await = Some(Arc::clone(&transport));

        // Run the init handshake. If any step fails, roll back: drop the
        // transport, decrement the count, surface the error.
        if let Err(e) = self.run_handshake(&transport).await {
            *self.transport.lock().await = None;
            self.connection_count.fetch_sub(1, Ordering::SeqCst);
            return Err(e);
        }

        // Spawn the polling task before flipping `available` so the first
        // snapshot read after connect already has fresh data (or at least
        // the handshake-time defaults).
        let polling_interval = self.polling_interval();
        let task = spawn_poll_task(
            Arc::clone(&transport),
            Arc::clone(&self.command_lock),
            Arc::clone(&self.snapshot),
            self.shutdown_tx.subscribe(),
            polling_interval,
        );
        *self.poll_handle.lock().await = Some(task);

        self.available.store(true, Ordering::SeqCst);
        debug!("transport manager connected and handshake complete");
        Ok(())
    }

    /// Reference-counted disconnect. Last caller out triggers teardown:
    /// clears [`is_available`], stops the poll task, sends `:K1` to halt
    /// tracking, drops the transport `Arc` (which calls
    /// `Transport::close`), and clears the parameter cache.
    pub async fn disconnect(&self) -> Result<()> {
        let prior = self.connection_count.fetch_sub(1, Ordering::SeqCst);
        if prior == 0 {
            // Already at zero — nothing to do; restore the count to avoid
            // wrap-around (shouldn't happen but defensive).
            self.connection_count.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        if prior > 1 {
            // Other clients still connected.
            return Ok(());
        }

        // 1→0 transition.
        self.available.store(false, Ordering::SeqCst);

        // Signal the poll task to exit, then await it.
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.poll_handle.lock().await.take() {
            // Best-effort: ignore JoinError (task already exited).
            let _ = handle.await;
        }

        // Best-effort tracking stop. The transport is about to close
        // either way, so a failure here is informational.
        if let Some(t) = self.transport.lock().await.as_ref() {
            let _ = self
                .send_through(t, Command::StopMotion(Axis::Ra))
                .await
                .inspect_err(|e| warn!("disconnect: stop tracking failed: {e}"));
        }

        // Drop the transport Arc — its `Drop` should call `close` on the
        // underlying connection.
        *self.transport.lock().await = None;
        *self.parameters.write().await = None;

        // Reset the shutdown channel so a subsequent connect starts fresh.
        let _ = self.shutdown_tx.send(false);

        debug!("transport manager disconnected");
        Ok(())
    }

    /// `true` only when the underlying transport is open AND the init
    /// handshake has succeeded — i.e. the manager is ready to round-trip
    /// commands. Returns `false` while a connect is mid-handshake or after
    /// a handshake-failure rollback.
    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    /// Wire-protocol polling interval taken from the config block. Exposed
    /// so [`crate::MountDevice`]'s slew-completion watcher can match the
    /// background poller's cadence.
    pub fn polling_interval_for_watcher(&self) -> Duration {
        self.polling_interval()
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
    pub async fn send(&self, command: Command) -> Result<Response> {
        let transport = self
            .transport
            .lock()
            .await
            .clone()
            .ok_or(StarAdvError::NotConnected)?;
        self.send_through(&transport, command).await
    }

    fn polling_interval(&self) -> Duration {
        match &self.config.transport {
            TransportConfig::Usb(usb) => usb.polling_interval,
            TransportConfig::Udp(udp) => udp.polling_interval,
        }
    }

    fn command_timeout(&self) -> Duration {
        match &self.config.transport {
            TransportConfig::Usb(usb) => usb.command_timeout,
            TransportConfig::Udp(udp) => udp.command_timeout,
        }
    }

    /// Round-trip a command through `transport`, taking the command lock
    /// first so concurrent send / poll calls serialise on the wire.
    async fn send_through(
        &self,
        transport: &Arc<dyn Transport>,
        command: Command,
    ) -> Result<Response> {
        let _guard = self.command_lock.lock().await;
        round_trip_one(transport, &command, self.command_timeout()).await
    }

    /// Run the init handshake against an opened transport. Populates the
    /// parameter cache and seeds the snapshot with the initial encoder
    /// positions. Mirrors the design doc's "Initialisation sequence".
    async fn run_handshake(&self, transport: &Arc<dyn Transport>) -> Result<()> {
        let timeout = self.command_timeout();
        let lock = self.command_lock.clone();
        let _guard = lock.lock().await;

        // Step 1–2: initialise both axes.
        for axis in [Axis::Ra, Axis::Dec] {
            expect_ack(round_trip_one(transport, &Command::Initialize(axis), timeout).await?)?;
        }

        // Step 3–4: per-axis CPR.
        let cpr_ra =
            expect_u24(round_trip_one(transport, &Command::InquireCpr(Axis::Ra), timeout).await?)?;
        let cpr_dec =
            expect_u24(round_trip_one(transport, &Command::InquireCpr(Axis::Dec), timeout).await?)?;
        // Step 5: TMR_Freq.
        let tmr_freq =
            expect_u24(round_trip_one(transport, &Command::InquireTmrFreq, timeout).await?)?;
        // Step 6–7: high-speed ratio per axis.
        let hsr_ra = expect_u24(
            round_trip_one(
                transport,
                &Command::InquireHighSpeedRatio(Axis::Ra),
                timeout,
            )
            .await?,
        )?;
        let hsr_dec = expect_u24(
            round_trip_one(
                transport,
                &Command::InquireHighSpeedRatio(Axis::Dec),
                timeout,
            )
            .await?,
        )?;
        // Step 8: motor-board version (logged only).
        let board = expect_u24(
            round_trip_one(
                transport,
                &Command::InquireMotorBoardVersion(Axis::Ra),
                timeout,
            )
            .await?,
        )?;
        debug!(motor_board = format!("{board:#08X}"), "motor-board version");

        // Step 9–10: initial encoder positions seed the snapshot.
        let pos_ra = expect_position(
            round_trip_one(transport, &Command::InquirePosition(Axis::Ra), timeout).await?,
        )?;
        let pos_dec = expect_position(
            round_trip_one(transport, &Command::InquirePosition(Axis::Dec), timeout).await?,
        )?;

        *self.parameters.write().await = Some(MountParameters {
            cpr_ra,
            cpr_dec,
            tmr_freq,
            high_speed_ratio_ra: hsr_ra,
            high_speed_ratio_dec: hsr_dec,
            motor_board_version: board,
        });
        let mut snap = self.snapshot.write().await;
        snap.ra.position_ticks = pos_ra;
        snap.dec.position_ticks = pos_dec;
        Ok(())
    }
}

/// Free-function send: encode → round_trip → decode. Used by both
/// [`TransportManager::send`] and the polling task.
async fn round_trip_one(
    transport: &Arc<dyn Transport>,
    command: &Command,
    timeout: Duration,
) -> Result<Response> {
    let bytes = command.encode()?;
    let reply = transport.round_trip(&bytes, timeout).await?;
    let response = Response::decode(&reply, command)?;
    Ok(response)
}

fn expect_ack(r: Response) -> Result<()> {
    match r {
        Response::Ack => Ok(()),
        other => Err(StarAdvError::Transport(format!(
            "expected Ack, got {other:?}"
        ))),
    }
}

fn expect_u24(r: Response) -> Result<u32> {
    match r {
        Response::U24(v) => Ok(v),
        other => Err(StarAdvError::Transport(format!(
            "expected U24, got {other:?}"
        ))),
    }
}

fn expect_position(r: Response) -> Result<i32> {
    match r {
        Response::Position(v) => Ok(v),
        other => Err(StarAdvError::Transport(format!(
            "expected Position, got {other:?}"
        ))),
    }
}

/// Spawn the polling task. Polls `:f<axis>` and `:j<axis>` for both axes
/// at `interval`, updates the snapshot, exits when `shutdown` flips to
/// `true`.
fn spawn_poll_task(
    transport: Arc<dyn Transport>,
    command_lock: Arc<Mutex<()>>,
    snapshot: Arc<RwLock<MountSnapshot>>,
    mut shutdown: watch::Receiver<bool>,
    interval: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        debug!("polling task: shutdown signal received");
                        return;
                    }
                }
                _ = tick.tick() => {
                    let _guard = command_lock.lock().await;
                    let mut snap = MountSnapshot::default();
                    if let Err(e) = poll_axis(&transport, Axis::Ra, &mut snap.ra).await {
                        debug!("polling RA failed: {e}");
                        continue;
                    }
                    if let Err(e) = poll_axis(&transport, Axis::Dec, &mut snap.dec).await {
                        debug!("polling Dec failed: {e}");
                        continue;
                    }
                    *snapshot.write().await = snap;
                }
            }
        }
    })
}

async fn poll_axis(
    transport: &Arc<dyn Transport>,
    axis: Axis,
    out: &mut AxisSnapshot,
) -> Result<()> {
    use skywatcher_motor_protocol::AxisStatus;
    // Use the manager's command timeout as a reasonable poll deadline. The
    // poll task does not have direct access to the manager's config, so
    // hard-code 1 s here — long enough for any single round-trip on either
    // transport, short enough that one stuck poll does not stall the loop.
    let timeout = Duration::from_secs(1);
    let pos = round_trip_one(transport, &Command::InquirePosition(axis), timeout).await?;
    out.position_ticks = match pos {
        Response::Position(p) => p,
        other => {
            return Err(StarAdvError::Transport(format!(
                "expected Position, got {other:?}"
            )))
        }
    };
    let status = round_trip_one(transport, &Command::InquireStatus(axis), timeout).await?;
    let s: AxisStatus = match status {
        Response::Status(s) => s,
        other => {
            return Err(StarAdvError::Transport(format!(
                "expected Status, got {other:?}"
            )))
        }
    };
    out.running = s.running;
    out.goto = s.goto;
    Ok(())
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

    #[tokio::test]
    async fn connect_runs_handshake_and_seeds_parameter_cache() {
        let m = manager();
        m.connect().await.unwrap();
        assert!(m.is_available());
        let params = m.parameters().await.expect("handshake populates cache");
        assert_eq!(params.cpr_ra, 0x0037_5F00);
        assert_eq!(params.cpr_dec, 0x0037_5F00);
        assert_eq!(params.tmr_freq, 0x00F4_2400);
        assert_eq!(params.motor_board_version, 0x0003_300C);
    }

    #[tokio::test]
    async fn connect_is_reference_counted() {
        let m = manager();
        m.connect().await.unwrap();
        m.connect().await.unwrap();
        assert_eq!(m.connection_count.load(Ordering::SeqCst), 2);
        assert!(m.is_available());
        m.disconnect().await.unwrap();
        // Still connected — one client remains.
        assert!(m.is_available());
        assert_eq!(m.connection_count.load(Ordering::SeqCst), 1);
        m.disconnect().await.unwrap();
        // Now teardown: no more clients.
        assert!(!m.is_available());
        assert_eq!(m.connection_count.load(Ordering::SeqCst), 0);
        // Parameter cache cleared on full disconnect.
        assert!(m.parameters().await.is_none());
    }

    #[tokio::test]
    async fn send_before_connect_returns_not_connected() {
        let m = manager();
        let r = m.send(Command::InquirePosition(Axis::Ra)).await;
        assert!(matches!(r, Err(StarAdvError::NotConnected)));
    }

    #[tokio::test]
    async fn send_after_connect_round_trips_through_mock() {
        let m = manager();
        m.connect().await.unwrap();
        let r = m.send(Command::InquireCpr(Axis::Ra)).await.unwrap();
        assert_eq!(r, Response::U24(0x0037_5F00));
    }
}
