//! Thin manager wrapping `SharedTransport<SkywatcherCodec>` plus the
//! mount-specific cached state (parameters from the handshake, the
//! background poll snapshot).
//!
//! The refcount, slot, open/close transitions, command-lock arbitration,
//! and poll-task lifetime all live in
//! [`rusty_photon_shared_transport::SharedTransport`]. What stays here:
//!
//! * The Sky-Watcher handshake (`:F` × 2, `:a` × 2, `:b`, `:g` × 2,
//!   `:e`, `:j` × 2) and the parameter cache it populates.
//! * The background poll loop body that refreshes `:f` + `:j` on both
//!   axes into the snapshot.
//! * The [`PollPauseGuard`] mechanism the slew/park watchers use to
//!   pause the polling task while they own the wire.
//! * `seed_*_position` mutators that publish `:E`-written encoder
//!   values into the snapshot immediately (so reads landing right
//!   after `Sync` don't see the pre-sync position).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rusty_photon_shared_transport::{
    Connection, Hooks, Session, SessionError, SharedTransport, TransportFactory, WhileOpen,
};
use skywatcher_motor_protocol::{Axis, AxisStatus, Command, Response};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, warn};

use crate::codec::{decode_frame_for, SkywatcherCodec, SkywatcherCodecError};
use crate::config::{Config, TransportConfig};
use crate::error::{Result, StarAdvError};

/// Snapshot of the values the mount reports during the init handshake.
/// Meaningful units are in the design doc.
#[derive(Debug, Clone, Copy, Default)]
pub struct MountParameters {
    pub cpr_ra: u32,
    pub cpr_dec: u32,
    pub tmr_freq: u32,
    pub high_speed_ratio_ra: u32,
    pub high_speed_ratio_dec: u32,
    pub motor_board_version: u32,
    pub ra_at_handshake_ticks: i32,
    pub dec_at_handshake_ticks: i32,
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

/// RAII guard that pauses background polling while held. Drop
/// decrements a depth counter; the polling task resumes only when
/// the counter reaches zero. Returned by
/// [`MountManager::pause_background_polling`].
///
/// Ref-counted (not a plain bool) so overlapping guards from
/// different paths (e.g. an `AbortSlew` racing a still-running slew
/// watcher) can't prematurely resume polling while another guard
/// is still active.
///
/// While paused, the watcher is the *only* writer on the wire
/// (modulo concurrent Alpaca-client driven `MountDevice` ops, which
/// are rare during a slew). The watcher must poll `:f` / `:j`
/// directly via the watcher's own session to keep the cached snapshot
/// fresh for any ASCOM reader that happens to fire during the slew.
pub struct PollPauseGuard {
    depth: Arc<AtomicU32>,
}

impl Drop for PollPauseGuard {
    fn drop(&mut self) {
        // We only ever increment-and-then-decrement-on-drop, so the
        // counter can't go negative under normal control flow. If a
        // future refactor somehow drops a guard twice, fetch_sub at 0
        // would underflow to `u32::MAX` — switch to a CAS-based
        // saturating decrement here if that risk becomes real.
        self.depth.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Manager that wraps the shared transport plus mount-specific
/// state. One instance per process; the `MountDevice` and every
/// spawned watcher hold `Arc<MountManager>`.
pub struct MountManager {
    transport: Arc<SharedTransport<SkywatcherCodec>>,
    parameters: Arc<RwLock<Option<MountParameters>>>,
    snapshot: Arc<RwLock<MountSnapshot>>,
    poll_pause_depth: Arc<AtomicU32>,
    polling_interval: Duration,
    command_timeout: Duration,
}

impl MountManager {
    pub fn new(config: Config, factory: Arc<dyn TransportFactory>) -> Arc<Self> {
        let parameters = Arc::new(RwLock::new(None));
        let snapshot = Arc::new(RwLock::new(MountSnapshot::default()));
        let poll_pause_depth = Arc::new(AtomicU32::new(0));
        let (polling_interval, command_timeout) = match &config.transport {
            TransportConfig::Usb(usb) => (usb.polling_interval, usb.command_timeout),
            TransportConfig::Udp(udp) => (udp.polling_interval, udp.command_timeout),
        };

        let hooks = build_hooks(
            Arc::clone(&parameters),
            Arc::clone(&snapshot),
            Arc::clone(&poll_pause_depth),
            polling_interval,
        );
        let transport = SharedTransport::new(factory, SkywatcherCodec, hooks);

        Arc::new(Self {
            transport,
            parameters,
            snapshot,
            poll_pause_depth,
            polling_interval,
            command_timeout,
        })
    }

    /// Access the shared transport so devices can acquire sessions.
    pub fn transport(&self) -> &Arc<SharedTransport<SkywatcherCodec>> {
        &self.transport
    }

    /// Cheap, non-blocking snapshot — true between handshake completion
    /// and the start of teardown.
    pub fn is_available(&self) -> bool {
        self.transport.is_available()
    }

    /// Latest cached parameters. `None` until handshake completes.
    pub async fn parameters(&self) -> Option<MountParameters> {
        *self.parameters.read().await
    }

    /// Latest poll-loop snapshot.
    pub async fn snapshot(&self) -> MountSnapshot {
        *self.snapshot.read().await
    }

    /// Wire-protocol polling interval taken from the config block. Exposed
    /// so the slew-completion watcher can match the background poller's
    /// cadence.
    pub fn polling_interval_for_watcher(&self) -> Duration {
        self.polling_interval
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
    /// Safe to nest or overlap across paths.
    pub fn pause_background_polling(&self) -> PollPauseGuard {
        self.poll_pause_depth.fetch_add(1, Ordering::SeqCst);
        PollPauseGuard {
            depth: Arc::clone(&self.poll_pause_depth),
        }
    }

    /// Send one command on the caller's session, return one typed
    /// response. Does *not* update the snapshot — the background
    /// poller (and `poll_axes_now`) own that responsibility.
    pub async fn send(
        &self,
        session: &Session<SkywatcherCodec>,
        command: Command,
    ) -> Result<Response> {
        let bytes = session
            .request(command.clone())
            .await
            .map_err(StarAdvError::from)?;
        decode_frame_for(&command, &bytes)
            .map_err(|SkywatcherCodecError::Protocol(pe)| StarAdvError::Protocol(pe))
    }

    /// Synchronously round-trip `:f` + `:j` on both axes via the
    /// caller's session, update the cached snapshot, and return the
    /// fresh snapshot.
    ///
    /// Used by the slew/park watcher loops *instead of* reading the
    /// background polling task's cached snapshot. The caller is
    /// responsible for ensuring the background polling task is paused
    /// via [`pause_background_polling`](Self::pause_background_polling)
    /// during a sequence of `poll_axes_now` calls — otherwise the two
    /// paths contend for the connection's command lock and the wire
    /// load doubles.
    pub async fn poll_axes_now(&self, session: &Session<SkywatcherCodec>) -> Result<MountSnapshot> {
        let mut snap = MountSnapshot::default();
        poll_axis_via_session(self, session, Axis::Ra, &mut snap.ra).await?;
        poll_axis_via_session(self, session, Axis::Dec, &mut snap.dec).await?;
        *self.snapshot.write().await = snap;
        Ok(snap)
    }

    /// Update the cached snapshot's RA position.
    ///
    /// Used by `SyncToCoordinates` to publish the just-written encoder
    /// position immediately rather than waiting up to
    /// `polling_interval` for the background task to refresh.
    ///
    /// Per-axis methods (instead of taking an `Axis`) eliminate the
    /// `Axis::Both` case at the type level — `:E3` (both axes) isn't
    /// part of the MVP wire surface, sync writes per-axis values that
    /// can differ, and there's no sensible single-tick interpretation
    /// of "seed both".
    pub async fn seed_ra_position(&self, ticks: i32) {
        self.snapshot.write().await.ra.position_ticks = ticks;
    }

    /// Update the cached snapshot's Dec position. See
    /// [`seed_ra_position`](Self::seed_ra_position) for rationale.
    pub async fn seed_dec_position(&self, ticks: i32) {
        self.snapshot.write().await.dec.position_ticks = ticks;
    }

    /// Per-call command timeout from the active transport config block.
    /// Exposed so handshake and poll loops can match the configured
    /// expectation; the shared-transport `FrameTransport`s already
    /// enforce the same value at the wire layer.
    pub fn command_timeout(&self) -> Duration {
        self.command_timeout
    }
}

fn build_hooks(
    parameters: Arc<RwLock<Option<MountParameters>>>,
    snapshot: Arc<RwLock<MountSnapshot>>,
    poll_pause_depth: Arc<AtomicU32>,
    polling_interval: Duration,
) -> Hooks<SkywatcherCodec> {
    let p_hs = Arc::clone(&parameters);
    let s_hs = Arc::clone(&snapshot);
    let s_poll = Arc::clone(&snapshot);
    let depth_poll = Arc::clone(&poll_pause_depth);
    let p_td = Arc::clone(&parameters);
    Hooks {
        handshake: Box::new(move |conn| {
            let parameters = Arc::clone(&p_hs);
            let snapshot = Arc::clone(&s_hs);
            Box::pin(handshake(conn, parameters, snapshot))
        }),
        teardown: Box::new(move |conn| {
            let parameters = Arc::clone(&p_td);
            Box::pin(teardown(conn, parameters))
        }),
        while_open: Some(Box::new(move |ctx| {
            let snapshot = Arc::clone(&s_poll);
            let depth = Arc::clone(&depth_poll);
            Box::pin(poll_loop(ctx, snapshot, depth, polling_interval))
        })),
    }
}

/// `0→1` handshake: initialise both axes, query parameters, seed the
/// snapshot with the initial encoder positions.
async fn handshake(
    conn: &Connection<SkywatcherCodec>,
    parameters: Arc<RwLock<Option<MountParameters>>>,
    snapshot: Arc<RwLock<MountSnapshot>>,
) -> std::result::Result<(), SkywatcherCodecError> {
    // Step 1–2: initialise both axes.
    for axis in [Axis::Ra, Axis::Dec] {
        expect_ack(request_typed(conn, Command::Initialize(axis)).await?)?;
    }

    // Step 3–4: per-axis CPR.
    let cpr_ra = expect_u24(request_typed(conn, Command::InquireCpr(Axis::Ra)).await?)?;
    let cpr_dec = expect_u24(request_typed(conn, Command::InquireCpr(Axis::Dec)).await?)?;
    // Step 5: TMR_Freq.
    let tmr_freq = expect_u24(request_typed(conn, Command::InquireTmrFreq).await?)?;
    // Step 6–7: high-speed ratio per axis.
    let hsr_ra = expect_u24(request_typed(conn, Command::InquireHighSpeedRatio(Axis::Ra)).await?)?;
    let hsr_dec =
        expect_u24(request_typed(conn, Command::InquireHighSpeedRatio(Axis::Dec)).await?)?;
    // Step 8: motor-board version (logged only).
    let board =
        expect_u24(request_typed(conn, Command::InquireMotorBoardVersion(Axis::Ra)).await?)?;
    debug!(motor_board = format!("{board:#08X}"), "motor-board version");

    // Step 9–10: initial encoder positions seed the snapshot.
    let pos_ra = expect_position(request_typed(conn, Command::InquirePosition(Axis::Ra)).await?)?;
    let pos_dec = expect_position(request_typed(conn, Command::InquirePosition(Axis::Dec)).await?)?;

    *parameters.write().await = Some(MountParameters {
        cpr_ra,
        cpr_dec,
        tmr_freq,
        high_speed_ratio_ra: hsr_ra,
        high_speed_ratio_dec: hsr_dec,
        motor_board_version: board,
        ra_at_handshake_ticks: pos_ra,
        dec_at_handshake_ticks: pos_dec,
    });
    let mut snap = snapshot.write().await;
    snap.ra.position_ticks = pos_ra;
    snap.dec.position_ticks = pos_dec;
    Ok(())
}

/// `1→0` teardown: best-effort halt on both axes before the transport
/// closes, then clear the parameter cache so the next acquire starts
/// from a clean slate. All errors are log-and-continue per the
/// `Hooks::teardown` infallible contract.
async fn teardown(
    conn: &Connection<SkywatcherCodec>,
    parameters: Arc<RwLock<Option<MountParameters>>>,
) {
    // Order matters: `:L` is the hammer (instant stop), `:K` is
    // graceful — issue the hammer first to guarantee motion stops
    // even if the graceful stop fails.
    for cmd in [
        Command::InstantStop(Axis::Ra),
        Command::InstantStop(Axis::Dec),
        Command::StopMotion(Axis::Ra),
    ] {
        if let Err(e) = request_typed(conn, cmd.clone()).await {
            warn!(
                command = ?cmd,
                error = %e,
                "teardown wire command failed (continuing)"
            );
        }
    }
    *parameters.write().await = None;
}

/// Background poll loop. Refreshes `:j` + `:f` on both axes at
/// `polling_interval` while not paused via [`PollPauseGuard`].
async fn poll_loop(
    ctx: WhileOpen<SkywatcherCodec>,
    snapshot: Arc<RwLock<MountSnapshot>>,
    poll_pause_depth: Arc<AtomicU32>,
    polling_interval: Duration,
) {
    let mut ticker = interval(polling_interval);
    // Skip the immediate first tick — the handshake just populated the
    // snapshot; first poll should wait one interval.
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = ctx.cancelled() => {
                debug!("sky-watcher poll loop received cancellation");
                return;
            }
        }
        if poll_pause_depth.load(Ordering::SeqCst) > 0 {
            // Watcher owns the wire — skip this tick. The watcher's
            // own `poll_axes_now` updates the snapshot during the
            // slew so ASCOM readers still see fresh data.
            continue;
        }
        let mut snap = MountSnapshot::default();
        if let Err(e) = poll_axis_via_ctx(&ctx, Axis::Ra, &mut snap.ra).await {
            debug!("polling RA failed: {e}");
            continue;
        }
        if let Err(e) = poll_axis_via_ctx(&ctx, Axis::Dec, &mut snap.dec).await {
            debug!("polling Dec failed: {e}");
            continue;
        }
        *snapshot.write().await = snap;
    }
}

/// Request a command via the handshake-time `Connection` and decode it
/// to the protocol crate's typed [`Response`]. Used inside `handshake`
/// and `teardown` (which receive `&Connection<C>` per the `Hooks`
/// contract, not a `Session`).
async fn request_typed(
    conn: &Connection<SkywatcherCodec>,
    cmd: Command,
) -> std::result::Result<Response, SkywatcherCodecError> {
    let bytes = conn.request(cmd.clone()).await.map_err(|e| match e {
        SessionError::Codec(c) => c,
        SessionError::Transport(t) => SkywatcherCodecError::Protocol(
            skywatcher_motor_protocol::ProtocolError::FrameError(t.to_string()),
        ),
        SessionError::SkipExhausted(n) => {
            SkywatcherCodecError::Protocol(skywatcher_motor_protocol::ProtocolError::FrameError(
                format!("skipped {n} non-matching frame(s)"),
            ))
        }
    })?;
    decode_frame_for(&cmd, &bytes)
}

async fn poll_axis_via_ctx(
    ctx: &WhileOpen<SkywatcherCodec>,
    axis: Axis,
    out: &mut AxisSnapshot,
) -> Result<()> {
    let pos_bytes = ctx
        .request(Command::InquirePosition(axis))
        .await
        .map_err(StarAdvError::from)?;
    let pos = decode_frame_for(&Command::InquirePosition(axis), &pos_bytes)
        .map_err(|SkywatcherCodecError::Protocol(pe)| StarAdvError::Protocol(pe))?;
    out.position_ticks = expect_position_runtime(pos)?;
    let status_bytes = ctx
        .request(Command::InquireStatus(axis))
        .await
        .map_err(StarAdvError::from)?;
    let status = decode_frame_for(&Command::InquireStatus(axis), &status_bytes)
        .map_err(|SkywatcherCodecError::Protocol(pe)| StarAdvError::Protocol(pe))?;
    let s = expect_status_runtime(status)?;
    out.running = s.running;
    out.goto = s.goto;
    out.blocked = s.blocked;
    Ok(())
}

async fn poll_axis_via_session(
    manager: &MountManager,
    session: &Session<SkywatcherCodec>,
    axis: Axis,
    out: &mut AxisSnapshot,
) -> Result<()> {
    let pos = manager
        .send(session, Command::InquirePosition(axis))
        .await?;
    out.position_ticks = expect_position_runtime(pos)?;
    let status = manager.send(session, Command::InquireStatus(axis)).await?;
    let s = expect_status_runtime(status)?;
    out.running = s.running;
    out.goto = s.goto;
    out.blocked = s.blocked;
    Ok(())
}

fn expect_ack(r: Response) -> std::result::Result<(), SkywatcherCodecError> {
    match r {
        Response::Ack => Ok(()),
        other => Err(SkywatcherCodecError::Protocol(
            skywatcher_motor_protocol::ProtocolError::FrameError(format!(
                "expected Ack, got {other:?}"
            )),
        )),
    }
}

fn expect_u24(r: Response) -> std::result::Result<u32, SkywatcherCodecError> {
    match r {
        Response::U24(v) => Ok(v),
        other => Err(SkywatcherCodecError::Protocol(
            skywatcher_motor_protocol::ProtocolError::FrameError(format!(
                "expected U24, got {other:?}"
            )),
        )),
    }
}

fn expect_position(r: Response) -> std::result::Result<i32, SkywatcherCodecError> {
    match r {
        Response::Position(v) => Ok(v),
        other => Err(SkywatcherCodecError::Protocol(
            skywatcher_motor_protocol::ProtocolError::FrameError(format!(
                "expected Position, got {other:?}"
            )),
        )),
    }
}

fn expect_position_runtime(r: Response) -> Result<i32> {
    match r {
        Response::Position(v) => Ok(v),
        other => Err(StarAdvError::Transport(format!(
            "expected Position, got {other:?}"
        ))),
    }
}

fn expect_status_runtime(r: Response) -> Result<AxisStatus> {
    match r {
        Response::Status(s) => Ok(s),
        other => Err(StarAdvError::Transport(format!(
            "expected Status, got {other:?}"
        ))),
    }
}

#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    //! Behaviour-level tests for the manager, driven through the mock
    //! transport factory. Race / refcount / rollback invariants are
    //! tested once for everyone in `rusty-photon-shared-transport` —
    //! they don't get re-tested here per the migration plan.

    use super::*;
    use crate::transport::mock::{CapturingMockFactory, MockTransportFactory};

    fn manager() -> Arc<MountManager> {
        MountManager::new(Config::default(), Arc::new(MockTransportFactory))
    }

    #[test]
    fn new_starts_unavailable() {
        let m = manager();
        assert!(!m.is_available());
    }

    #[tokio::test]
    async fn parameters_are_none_before_handshake() {
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
    async fn acquire_runs_handshake_and_seeds_parameter_cache() {
        let m = manager();
        let session = m.transport().acquire().await.unwrap();
        assert!(m.is_available());
        let params = m.parameters().await.expect("handshake populates cache");
        assert_eq!(params.cpr_ra, 0x0037_5F00);
        assert_eq!(params.cpr_dec, 0x0037_5F00);
        assert_eq!(params.tmr_freq, 0x00F4_2400);
        assert_eq!(params.motor_board_version, 0x0003_300C);
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn close_clears_parameters_and_marks_unavailable() {
        let m = manager();
        let session = m.transport().acquire().await.unwrap();
        assert!(m.is_available());
        session.close().await.unwrap();
        assert!(!m.is_available());
        assert!(m.parameters().await.is_none());
    }

    #[tokio::test]
    async fn send_round_trips_typed_response() {
        let m = manager();
        let session = m.transport().acquire().await.unwrap();
        let r = m
            .send(&session, Command::InquireCpr(Axis::Ra))
            .await
            .unwrap();
        assert_eq!(r, Response::U24(0x0037_5F00));
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn poll_axes_now_returns_fresh_snapshot_and_updates_cache() {
        let m = manager();
        let session = m.transport().acquire().await.unwrap();
        let polled = m.poll_axes_now(&session).await.unwrap();
        let cached = m.snapshot().await;
        assert_eq!(polled.ra.position_ticks, cached.ra.position_ticks);
        assert_eq!(polled.dec.position_ticks, cached.dec.position_ticks);
        assert_eq!(polled.ra.running, cached.ra.running);
        assert_eq!(polled.dec.running, cached.dec.running);
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn pause_background_polling_stops_wire_traffic_and_resumes_on_drop() {
        // Hold a guard, count zero `:j`/`:f` traffic during the hold,
        // resume after drop. Mirrors the legacy
        // `transport_manager.rs` test.
        let factory = CapturingMockFactory::new();
        let state = Arc::clone(&factory.state);
        let mut cfg = Config::default();
        if let TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        let m = MountManager::new(cfg, Arc::new(factory));
        let session = m.transport().acquire().await.unwrap();

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

        tokio::time::sleep(Duration::from_millis(80)).await;
        let baseline_polls = poll_count(&state.lock().await.command_log);
        assert!(
            baseline_polls >= 4,
            "expected background polling to have issued ≥4 :j/:f frames in 80ms (interval=20ms), got {baseline_polls}"
        );

        let guard = m.pause_background_polling();
        let count_at_pause = poll_count(&state.lock().await.command_log);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let count_during_pause = poll_count(&state.lock().await.command_log);
        assert_eq!(
            count_during_pause,
            count_at_pause,
            "polling task issued {} new :j/:f frames while pause guard was held",
            count_during_pause - count_at_pause
        );

        drop(guard);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let count_after_resume = poll_count(&state.lock().await.command_log);
        assert!(
            count_after_resume > count_during_pause,
            "polling did not resume after guard drop"
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn pause_background_polling_is_refcounted_across_overlapping_guards() {
        let factory = CapturingMockFactory::new();
        let state = Arc::clone(&factory.state);
        let mut cfg = Config::default();
        if let TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        let m = MountManager::new(cfg, Arc::new(factory));
        let session = m.transport().acquire().await.unwrap();

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

        let outer = m.pause_background_polling();
        let inner = m.pause_background_polling();
        let count_at_pause = poll_count(&state.lock().await.command_log);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            poll_count(&state.lock().await.command_log),
            count_at_pause,
            "depth=2: polling must stay paused"
        );

        drop(inner);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            poll_count(&state.lock().await.command_log),
            count_at_pause,
            "depth=1 after inner drop: polling must remain paused while outer is alive"
        );

        drop(outer);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let count_after_full_release = poll_count(&state.lock().await.command_log);
        assert!(
            count_after_full_release > count_at_pause,
            "polling must resume after depth returns to 0"
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn polling_interval_for_watcher_matches_usb_config() {
        let mut cfg = Config::default();
        if let TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(123);
        }
        let m = MountManager::new(cfg, Arc::new(MockTransportFactory));
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
        let m = MountManager::new(cfg, Arc::new(MockTransportFactory));
        assert_eq!(m.polling_interval_for_watcher(), Duration::from_millis(77));
    }

    #[tokio::test]
    async fn seed_positions_update_snapshot() {
        let m = manager();
        m.seed_ra_position(12_345).await;
        m.seed_dec_position(-6_789).await;
        let snap = m.snapshot().await;
        assert_eq!(snap.ra.position_ticks, 12_345);
        assert_eq!(snap.dec.position_ticks, -6_789);
    }

    #[tokio::test]
    async fn teardown_sends_halt_sequence() {
        // After session.close, the teardown hook should have issued
        // :L1, :L2, :K1 in order. Use CapturingMockFactory to inspect.
        let factory = CapturingMockFactory::new();
        let state = Arc::clone(&factory.state);
        let m = MountManager::new(Config::default(), Arc::new(factory));
        let session = m.transport().acquire().await.unwrap();
        session.close().await.unwrap();
        let log = state.lock().await.command_log.clone();
        // Find the teardown commands (they come last).
        let teardown_frames: Vec<&[u8]> = log
            .iter()
            .rev()
            .take_while(|f| f.starts_with(b":L") || f.starts_with(b":K"))
            .map(|v| v.as_slice())
            .collect();
        // We pushed in reverse: rev again to get forward order.
        let mut forward: Vec<&[u8]> = teardown_frames.into_iter().collect();
        forward.reverse();
        assert!(
            forward.iter().any(|f| f.starts_with(b":L1")),
            "teardown should issue :L1; got {forward:?}"
        );
        assert!(
            forward.iter().any(|f| f.starts_with(b":L2")),
            "teardown should issue :L2; got {forward:?}"
        );
        assert!(
            forward.iter().any(|f| f.starts_with(b":K1")),
            "teardown should issue :K1; got {forward:?}"
        );
    }
}
