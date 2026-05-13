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
    /// Sky-Watcher spec §5 (Response E nibble-1 bit-1): the firmware
    /// reports `Blocked` when the motor is stepping but the encoder
    /// isn't advancing — typically because the axis is against a
    /// mechanical stop or stalled. The slew watcher uses this to
    /// abort a runaway goto before the gearbox is damaged.
    pub blocked: bool,
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
    /// Depth counter for active [`PollPauseGuard`]s. The background
    /// polling task skips its tick whenever this is > 0: no `:j` /
    /// `:f` round-trips, no `command_lock` contention. The slew/park
    /// watchers acquire a guard for the duration of a slew so
    /// pickup-loop wire commands run without contention and the
    /// watcher's own targeted polls drive the snapshot's freshness.
    /// Ref-counted so overlapping guards (e.g. AbortSlew racing the
    /// post-slew tracking restart, or a future nested-watcher case)
    /// don't prematurely resume polling while another guard is
    /// still alive. Auto-decremented on guard drop, even if the
    /// watcher task is aborted mid-flight.
    poll_pause_depth: Arc<AtomicU32>,
    /// Background polling task; populated on the 0→1 connect transition,
    /// aborted on the 1→0 disconnect.
    poll_handle: Mutex<Option<JoinHandle<()>>>,
    /// Shutdown channel for the polling task. The task watches the
    /// receiver and exits when the value flips to `true`.
    shutdown_tx: watch::Sender<bool>,
}

/// RAII guard that pauses background polling while held. Drop
/// decrements a depth counter; the polling task resumes only when
/// the counter reaches zero. Returned by
/// [`TransportManager::pause_background_polling`].
///
/// Ref-counted (not a plain bool) so overlapping guards from
/// different paths (e.g. an AbortSlew racing a still-running slew
/// watcher) can't prematurely resume polling while another guard
/// is still active.
///
/// While paused, the watcher is the *only* writer on the wire
/// (modulo concurrent Alpaca-client driven `MountDevice` ops, which
/// are rare during a slew). The watcher must poll `:f` / `:j`
/// directly via [`TransportManager::poll_axes_now`] to keep the
/// cached snapshot fresh for any ASCOM reader that happens to fire
/// during the slew.
pub struct PollPauseGuard {
    depth: Arc<AtomicU32>,
}

impl Drop for PollPauseGuard {
    fn drop(&mut self) {
        // saturating_sub semantics via fetch_sub is fine: we only
        // ever increment-and-then-decrement-on-drop, so the counter
        // can't go negative under normal control flow. If a future
        // refactor somehow drops a guard twice, fetch_sub at 0
        // would underflow to MAX_U32 — switch to a CAS-based
        // saturating decrement here if that risk becomes real.
        self.depth.fetch_sub(1, Ordering::SeqCst);
    }
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
            poll_pause_depth: Arc::new(AtomicU32::new(0)),
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
        let command_timeout = self.command_timeout();
        let task = spawn_poll_task(
            Arc::clone(&transport),
            Arc::clone(&self.command_lock),
            Arc::clone(&self.snapshot),
            Arc::clone(&self.poll_pause_depth),
            self.shutdown_tx.subscribe(),
            polling_interval,
            command_timeout,
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

        // Best-effort: halt any in-progress motion before closing.
        // Clone the Arc out of the mutex first and drop the guard
        // immediately — holding the async mutex guard across the
        // `send_through` awaits would block any concurrent `send()`
        // call from making progress (the protocol's per-frame lock
        // would have nothing to do with it; this is the
        // `Mutex<Option<...>>` slot itself).
        let transport_for_halt = self.transport.lock().await.clone();
        if let Some(t) = transport_for_halt {
            // Issue :L on both axes (instant stop, aborts goto /
            // tracking alike) plus :K1 to be safe. Order matters —
            // :L is the hammer; :K is graceful.
            let _ = self
                .send_through(&t, Command::InstantStop(Axis::Ra))
                .await
                .inspect_err(|e| warn!("disconnect: :L1 failed: {e}"));
            let _ = self
                .send_through(&t, Command::InstantStop(Axis::Dec))
                .await
                .inspect_err(|e| warn!("disconnect: :L2 failed: {e}"));
            let _ = self
                .send_through(&t, Command::StopMotion(Axis::Ra))
                .await
                .inspect_err(|e| warn!("disconnect: :K1 failed: {e}"));
        }

        // Drop the transport Arc from the manager's slot — combined
        // with the local `transport_for_halt` going out of scope this
        // releases the last refs and triggers `Transport::close`.
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

    /// Pause the background polling task and return a guard that
    /// resumes it on drop. Used by the slew/park watchers to free the
    /// wire during a slew — pickup-loop commands run without
    /// contending with `:j` / `:f` polls, and the watcher's own
    /// [`poll_axes_now`](Self::poll_axes_now) drives snapshot
    /// freshness while paused.
    ///
    /// Ref-counted: each call increments a depth counter; polling
    /// resumes only when the last guard drops (counter back to 0).
    /// Safe to nest or overlap across paths (e.g. an AbortSlew that
    /// happens to fire while the slew watcher is still inside its
    /// post-pickup tracking restart). The depth counter underflows
    /// to `u32::MAX` if a guard is double-dropped — which can't
    /// happen under normal control flow because `PollPauseGuard`
    /// doesn't implement `Clone` — but if a future refactor allows
    /// it, switch the `Drop` impl to a CAS-based saturating
    /// decrement.
    pub fn pause_background_polling(&self) -> PollPauseGuard {
        self.poll_pause_depth.fetch_add(1, Ordering::SeqCst);
        PollPauseGuard {
            depth: Arc::clone(&self.poll_pause_depth),
        }
    }

    /// Synchronously round-trip `:j` + `:f` on both axes, update the
    /// cached snapshot, and return the fresh snapshot. Used by the
    /// slew/park watcher loops *instead of* reading the background
    /// polling task's cached snapshot — at the cost of 4 wire ops
    /// per call (~50 ms USB, ~100 ms UDP), the watcher knows the
    /// motor's actual state within one round-trip of it changing,
    /// rather than within `polling_interval` of the background task's
    /// last tick (up to 200 ms stale).
    ///
    /// The caller is responsible for ensuring the background polling
    /// task is paused via [`pause_background_polling`](Self::pause_background_polling)
    /// during a sequence of `poll_axes_now` calls — otherwise the two
    /// tasks contend for `command_lock` and the wire load doubles.
    pub async fn poll_axes_now(&self) -> Result<MountSnapshot> {
        let transport = self
            .transport
            .lock()
            .await
            .clone()
            .ok_or(StarAdvError::NotConnected)?;
        let _guard = self.command_lock.lock().await;
        let timeout = self.command_timeout();
        let mut snap = MountSnapshot::default();
        poll_axis(&transport, Axis::Ra, &mut snap.ra, timeout).await?;
        poll_axis(&transport, Axis::Dec, &mut snap.dec, timeout).await?;
        drop(_guard);
        *self.snapshot.write().await = snap;
        Ok(snap)
    }

    /// Update the cached snapshot's RA position.
    ///
    /// Used by `SyncToCoordinates` to publish the just-written encoder
    /// position immediately rather than waiting up to
    /// `polling_interval` for the background task to refresh. Without
    /// this, callers that read `RightAscension` within one poll
    /// interval of `Sync` see the pre-sync position — ConformU reads
    /// ~2 ms after the sync call returns and flags the stale value as
    /// a 3675-arc-second sync error.
    ///
    /// Per-axis methods (instead of taking an `Axis`) eliminate the
    /// `Axis::Both` case at the type level — `:E3` (both axes) isn't
    /// part of the MVP wire surface, sync writes per-axis values that
    /// can differ, and there's no sensible single-tick interpretation
    /// of "seed both". Previously this lived in one method that
    /// rejected `Axis::Both` via `debug_assert!`, which is a
    /// production no-op silently leaving the cache stale.
    pub async fn seed_ra_position(&self, ticks: i32) {
        self.snapshot.write().await.ra.position_ticks = ticks;
    }

    /// Update the cached snapshot's Dec position. See
    /// [`seed_ra_position`](Self::seed_ra_position) for rationale.
    pub async fn seed_dec_position(&self, ticks: i32) {
        self.snapshot.write().await.dec.position_ticks = ticks;
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
    debug!(tx = ?String::from_utf8_lossy(&bytes), "wire TX");
    let reply = transport.round_trip(&bytes, timeout).await?;
    debug!(rx = ?String::from_utf8_lossy(&reply), "wire RX");
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
/// `true`. Skips the poll cycle entirely while the pause depth-counter
/// is > 0 — the slew/park watchers hold one or more
/// [`PollPauseGuard`]s during a slew to free the wire for their own
/// targeted polls.
fn spawn_poll_task(
    transport: Arc<dyn Transport>,
    command_lock: Arc<Mutex<()>>,
    snapshot: Arc<RwLock<MountSnapshot>>,
    poll_pause_depth: Arc<AtomicU32>,
    mut shutdown: watch::Receiver<bool>,
    interval: Duration,
    command_timeout: Duration,
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
                    if poll_pause_depth.load(Ordering::SeqCst) > 0 {
                        // Watcher owns the wire — skip this tick. The
                        // watcher's own `poll_axes_now` updates the
                        // snapshot during the slew so ASCOM readers
                        // still see fresh data.
                        continue;
                    }
                    let _guard = command_lock.lock().await;
                    let mut snap = MountSnapshot::default();
                    if let Err(e) = poll_axis(&transport, Axis::Ra, &mut snap.ra, command_timeout).await {
                        debug!("polling RA failed: {e}");
                        continue;
                    }
                    if let Err(e) = poll_axis(&transport, Axis::Dec, &mut snap.dec, command_timeout).await {
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
    timeout: Duration,
) -> Result<()> {
    use skywatcher_motor_protocol::AxisStatus;
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
    out.blocked = s.blocked;
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

    /// Factory whose `open` always errors. Used to verify that the
    /// 0->1 connect transition rolls the count back when the underlying
    /// transport fails to open, leaving the manager re-tryable.
    struct FailingFactory;

    #[async_trait::async_trait]
    impl TransportFactory for FailingFactory {
        async fn open(&self, _config: &Config) -> Result<Arc<dyn Transport>> {
            Err(StarAdvError::ConnectionFailed("mock open failure".into()))
        }
    }

    #[tokio::test]
    async fn connect_rolls_back_count_when_factory_open_fails() {
        let m = TransportManager::new(Config::default(), Arc::new(FailingFactory));
        let err = m.connect().await.unwrap_err();
        assert!(matches!(err, StarAdvError::ConnectionFailed(_)));
        // Count must have rolled back so a later retry isn't blocked
        // by stale state.
        assert_eq!(m.connection_count.load(Ordering::SeqCst), 0);
        assert!(!m.is_available());
    }

    /// Transport that hands back a non-`=` reply on every round trip.
    /// Used to drive `run_handshake` into its expect_*-failure rollback
    /// branch.
    struct BadReplyTransport;

    #[async_trait::async_trait]
    impl Transport for BadReplyTransport {
        async fn round_trip(&self, _request: &[u8], _timeout: Duration) -> Result<Vec<u8>> {
            // `!00\r` = mount UnknownCommand error -> Response::decode returns
            // ProtocolError::MountError -> bubbles up as StarAdvError::Protocol.
            Ok(b"!00\r".to_vec())
        }
        async fn close(&self) -> Result<()> {
            Ok(())
        }
    }

    /// Factory whose `open()` initially hands back the bad-reply
    /// transport (so the handshake fails) and, after `set_healthy(true)`,
    /// hands back the real mock instead. Used to verify that the *same*
    /// `TransportManager` instance is re-connectable after a handshake
    /// rollback — not just that a brand-new manager would work.
    struct ToggleFactory {
        healthy: std::sync::atomic::AtomicBool,
    }

    impl ToggleFactory {
        fn new() -> Self {
            Self {
                healthy: std::sync::atomic::AtomicBool::new(false),
            }
        }
        fn set_healthy(&self, on: bool) {
            self.healthy.store(on, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl TransportFactory for ToggleFactory {
        async fn open(&self, config: &Config) -> Result<Arc<dyn Transport>> {
            if self.healthy.load(Ordering::SeqCst) {
                MockTransportFactory.open(config).await
            } else {
                Ok(Arc::new(BadReplyTransport))
            }
        }
    }

    #[tokio::test]
    async fn connect_rolls_back_when_handshake_fails_and_is_retryable() {
        let factory = Arc::new(ToggleFactory::new());
        let m = TransportManager::new(Config::default(), Arc::clone(&factory) as _);
        let err = m.connect().await.unwrap_err();
        // Some kind of protocol/transport error — the exact variant is
        // not the test's concern, just that connect surfaces the failure
        // and rolls the manager state back.
        assert!(!matches!(err, StarAdvError::ConnectionFailed(_)));
        assert_eq!(m.connection_count.load(Ordering::SeqCst), 0);
        assert!(!m.is_available());
        // No transport stuck in the slot.
        assert!(m.transport.lock().await.is_none());
        // Now flip the SAME factory to healthy and retry on the SAME
        // manager — proves the rollback left m re-connectable.
        factory.set_healthy(true);
        m.connect().await.unwrap();
        assert!(m.is_available());
        assert_eq!(m.connection_count.load(Ordering::SeqCst), 1);
        assert!(m.parameters().await.is_some());
    }

    #[tokio::test]
    async fn disconnect_at_zero_count_is_a_noop() {
        // Defensive: never decrement past zero. Calling disconnect on a
        // freshly-constructed manager must not wrap-around the counter
        // or panic.
        let m = manager();
        m.disconnect().await.unwrap();
        assert_eq!(m.connection_count.load(Ordering::SeqCst), 0);
        assert!(!m.is_available());
    }

    #[tokio::test]
    async fn polling_interval_for_watcher_matches_usb_config() {
        // The watcher reads its cadence from the same place as the
        // background poller — verify they don't drift.
        let mut cfg = Config::default();
        if let TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(123);
        }
        let m = TransportManager::new(cfg, Arc::new(MockTransportFactory));
        assert_eq!(m.polling_interval_for_watcher(), Duration::from_millis(123));
    }

    #[tokio::test]
    async fn polling_interval_for_watcher_matches_udp_config() {
        let cfg = Config {
            transport: TransportConfig::Udp(crate::config::UdpConfig {
                polling_interval: Duration::from_millis(77),
                ..crate::config::UdpConfig::default()
            }),
            ..Config::default()
        };
        let m = TransportManager::new(cfg, Arc::new(MockTransportFactory));
        assert_eq!(m.polling_interval_for_watcher(), Duration::from_millis(77));
    }

    #[tokio::test]
    async fn snapshot_is_populated_after_polling() {
        let m = manager();
        m.connect().await.unwrap();
        // Give the polling task at least one tick to run. Default
        // polling interval is much longer than this; the test is OK
        // tolerating one missed snapshot since handshake also seeds it.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snap = m.snapshot().await;
        // Handshake seeded position from the mock's :j reply (== 0
        // initially). Just confirm the read returns without panicking
        // and the running flags are sensible.
        let _ = snap.ra.position_ticks;
        assert!(!snap.ra.running);
        assert!(!snap.dec.running);
    }

    #[tokio::test]
    async fn pause_background_polling_stops_wire_traffic_and_resumes_on_drop() {
        // Direct observable contract: while a pause guard is held,
        // the background polling task issues *no* wire round-trips;
        // after the guard drops, polling resumes on the next tick.
        // Verified by counting `:j` and `:f` requests in the
        // CapturingMockFactory's command_log, which is the most
        // unambiguous evidence that the task actually skipped its
        // ticks (rather than the snapshot just happening to stay
        // constant, which a constant-value mock would do anyway).
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let mut cfg = Config::default();
        if let TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        let m = TransportManager::new(cfg, Arc::new(factory));
        m.connect().await.unwrap();

        // Helper: count poll-flavoured wire frames in the log. The
        // background poll task emits exactly `:j1 :f1 :j2 :f2`
        // per tick; ad-hoc `send()` calls and the handshake hit
        // different command letters.
        let poll_count = |log: &[Vec<u8>]| -> usize {
            log.iter()
                .filter(|f| {
                    f.len() >= 3
                        && f[0] == b':'
                        && matches!(f[1], b'j' | b'f')
                        && matches!(f[2], b'1' | b'2')
                })
                .count()
        };

        // Let the polling task tick a few times so we know it's
        // actively producing traffic.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let baseline_polls = poll_count(&mock.state.lock().await.command_log);
        assert!(
            baseline_polls >= 4,
            "expected background polling to have issued ≥4 :j/:f frames in 80ms (interval=20ms), got {baseline_polls}"
        );

        let guard = m.pause_background_polling();
        let count_at_pause = poll_count(&mock.state.lock().await.command_log);
        // Wait long enough for ~5 would-be polling ticks (~20 :j/:f
        // frames) to elapse. With the guard held, the task should
        // emit none.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let count_during_pause = poll_count(&mock.state.lock().await.command_log);
        assert_eq!(
            count_during_pause,
            count_at_pause,
            "polling task issued {} new :j/:f frames while a pause guard was held; expected 0",
            count_during_pause - count_at_pause
        );

        drop(guard);
        // Wait for the polling task to tick again post-resume.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let count_after_resume = poll_count(&mock.state.lock().await.command_log);
        assert!(
            count_after_resume > count_during_pause,
            "polling did not resume after guard drop: {} → {} (expected increase)",
            count_during_pause,
            count_after_resume
        );
    }

    #[tokio::test]
    async fn pause_background_polling_is_refcounted_across_overlapping_guards() {
        // Two overlapping guards: dropping the *first* must NOT
        // resume polling while the second is still alive. Only when
        // the depth counter hits 0 should the polling task tick
        // again. Without ref-counting, a single AtomicBool flag
        // would have flipped back to false on the first drop and
        // polling would have resumed while the second guard
        // claimed to still own the wire.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let mut cfg = Config::default();
        if let TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        let m = TransportManager::new(cfg, Arc::new(factory));
        m.connect().await.unwrap();

        let poll_count = |log: &[Vec<u8>]| -> usize {
            log.iter()
                .filter(|f| {
                    f.len() >= 3
                        && f[0] == b':'
                        && matches!(f[1], b'j' | b'f')
                        && matches!(f[2], b'1' | b'2')
                })
                .count()
        };

        // Acquire two guards.
        let outer = m.pause_background_polling();
        let inner = m.pause_background_polling();
        let count_at_pause = poll_count(&mock.state.lock().await.command_log);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            poll_count(&mock.state.lock().await.command_log),
            count_at_pause,
            "depth=2: polling must stay paused"
        );

        // Drop the inner guard; depth → 1, still paused.
        drop(inner);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            poll_count(&mock.state.lock().await.command_log),
            count_at_pause,
            "depth=1 after inner drop: polling must remain paused while outer is alive"
        );

        // Drop the outer guard; depth → 0, polling resumes.
        drop(outer);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let count_after_full_release = poll_count(&mock.state.lock().await.command_log);
        assert!(
            count_after_full_release > count_at_pause,
            "polling must resume after depth returns to 0: {} → {}",
            count_at_pause,
            count_after_full_release
        );
    }

    #[tokio::test]
    async fn poll_axes_now_returns_fresh_snapshot_and_updates_cache() {
        // `poll_axes_now` must do its own :j + :f round-trip
        // (synchronous round-trips, no waiting on the polling task)
        // and update the cached snapshot so ASCOM readers via
        // `snapshot()` see the same data.
        let m = manager();
        m.connect().await.unwrap();
        let polled = m.poll_axes_now().await.unwrap();
        let cached = m.snapshot().await;
        assert_eq!(polled.ra.position_ticks, cached.ra.position_ticks);
        assert_eq!(polled.dec.position_ticks, cached.dec.position_ticks);
        assert_eq!(polled.ra.running, cached.ra.running);
        assert_eq!(polled.dec.running, cached.dec.running);
    }

    #[tokio::test]
    async fn poll_axes_now_returns_not_connected_when_disconnected() {
        // Calling poll_axes_now before connect (no transport open)
        // must propagate the NotConnected error rather than panicking.
        let m = manager();
        let err = m.poll_axes_now().await.unwrap_err();
        assert!(matches!(err, StarAdvError::NotConnected));
    }

    #[test]
    fn expect_ack_rejects_non_ack_responses() {
        assert!(expect_ack(Response::U24(0)).is_err());
        assert!(expect_ack(Response::Position(0)).is_err());
        assert!(expect_ack(Response::Ack).is_ok());
    }

    #[test]
    fn expect_u24_rejects_non_u24_responses() {
        assert!(expect_u24(Response::Ack).is_err());
        assert!(expect_u24(Response::Position(0)).is_err());
        assert_eq!(expect_u24(Response::U24(42)).unwrap(), 42);
    }

    #[test]
    fn expect_position_rejects_non_position_responses() {
        assert!(expect_position(Response::Ack).is_err());
        assert!(expect_position(Response::U24(0)).is_err());
        assert_eq!(expect_position(Response::Position(-7)).unwrap(), -7);
    }
}
