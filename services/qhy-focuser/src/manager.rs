//! Thin manager wrapping `SharedTransport<QhyCodec>` plus the cached
//! focuser state the ASCOM device reads from.
//!
//! Refcount, slot, open/close transitions, command-lock arbitration, and
//! poll-task lifetime all live in
//! [`rusty_photon_shared_transport::SharedTransport`]. What stays here:
//!
//! * The Q-Focuser handshake (GetVersion → SetSpeed → GetPosition →
//!   ReadTemperature, seed the cache).
//! * The polling-interval loop body that refreshes position + temperature
//!   and detects move completion.
//! * The cached state the ASCOM `Focuser` properties read (`position`,
//!   `is_moving`, `temperature`, …).

use std::sync::Arc;
use std::time::Duration;

use rusty_photon_shared_transport::{
    Connection, Hooks, Session, SessionError, SharedTransport, TransportFactory, WhileOpen,
};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, warn};

use crate::codec::{QhyCodec, QhyCodecError, QhyResponse};
use crate::config::Config;
use crate::error::{QhyFocuserError, Result};
use crate::protocol::Command;

/// Cached state from the QHY Q-Focuser device.
#[derive(Debug, Clone, Default)]
pub struct CachedState {
    /// Current focuser position.
    pub position: Option<i64>,
    /// Target position for the in-flight move.
    pub target_position: Option<i64>,
    /// True between move start and the poll that observes target reached
    /// (or a successful Abort).
    pub is_moving: bool,
    /// Outer temperature (°C).
    pub outer_temp: Option<f64>,
    /// Chip temperature (°C).
    pub chip_temp: Option<f64>,
    /// Input voltage (V).
    pub voltage: Option<f64>,
    /// Firmware version reported by the handshake.
    pub firmware_version: Option<String>,
    /// Board version reported by the handshake.
    pub board_version: Option<String>,
}

/// Manager that wraps the shared transport plus Q-Focuser-specific
/// cached state. One instance per process; the ASCOM device holds
/// `Arc<FocuserManager>`.
pub struct FocuserManager {
    transport: Arc<SharedTransport<QhyCodec>>,
    cached_state: Arc<RwLock<CachedState>>,
}

impl FocuserManager {
    pub fn new(config: Config, factory: Arc<dyn TransportFactory>) -> Arc<Self> {
        let cached_state = Arc::new(RwLock::new(CachedState::default()));
        let hooks = build_hooks(
            Arc::clone(&cached_state),
            config.focuser.speed,
            config.serial.polling_interval,
        );
        let transport = SharedTransport::new(factory, QhyCodec, hooks);
        Arc::new(Self {
            transport,
            cached_state,
        })
    }

    /// Access the shared transport so the device can acquire sessions.
    pub fn transport(&self) -> &Arc<SharedTransport<QhyCodec>> {
        &self.transport
    }

    /// Cheap, non-blocking snapshot — true between handshake completion
    /// and the start of teardown.
    pub fn is_available(&self) -> bool {
        self.transport.is_available()
    }

    /// Clone the current cached state for read-only consumers.
    pub async fn get_cached_state(&self) -> CachedState {
        self.cached_state.read().await.clone()
    }

    /// Issue an absolute-move command over the device's session and
    /// update the cached `is_moving` / `target_position`. The poll loop
    /// clears these once the device reports the target reached.
    pub async fn move_absolute(&self, session: &Session<QhyCodec>, position: i64) -> Result<()> {
        // Set cache state before sending so a racing `is_moving` read
        // can't observe `is_moving == false` while the move is in flight.
        {
            let mut state = self.cached_state.write().await;
            state.target_position = Some(position);
            state.is_moving = true;
        }
        session
            .request(Command::AbsoluteMove { position })
            .await
            .map_err(QhyFocuserError::from)?;
        debug!(position, "absolute-move command sent");
        Ok(())
    }

    /// Abort the in-flight move and clear `is_moving` / `target_position`.
    pub async fn abort(&self, session: &Session<QhyCodec>) -> Result<()> {
        session
            .request(Command::Abort)
            .await
            .map_err(QhyFocuserError::from)?;
        let mut state = self.cached_state.write().await;
        state.is_moving = false;
        state.target_position = None;
        debug!("abort command sent");
        Ok(())
    }

    /// Force-refresh the cached position by issuing a `GetPosition` over
    /// the device's session — used by `is_moving` to avoid waiting up to
    /// one polling interval for move-completion detection.
    pub async fn refresh_position(&self, session: &Session<QhyCodec>) -> Result<()> {
        let resp = session
            .request(Command::GetPosition)
            .await
            .map_err(QhyFocuserError::from)?;
        let position = match resp {
            QhyResponse::Position(p) => p.position,
            other => {
                return Err(QhyFocuserError::InvalidResponse(format!(
                    "GetPosition returned non-Position frame: {other:?}"
                )));
            }
        };
        let mut state = self.cached_state.write().await;
        apply_position(&mut state, position);
        Ok(())
    }
}

fn build_hooks(
    cached_state: Arc<RwLock<CachedState>>,
    speed: u8,
    poll_interval: Duration,
) -> Hooks<QhyCodec> {
    let cs_handshake = Arc::clone(&cached_state);
    let cs_poll = Arc::clone(&cached_state);
    Hooks {
        handshake: Box::new(move |conn| {
            let cs = Arc::clone(&cs_handshake);
            Box::pin(handshake(conn, cs, speed))
        }),
        teardown: Box::new(|_| Box::pin(async {})),
        while_open: Some(Box::new(move |ctx| {
            let cs = Arc::clone(&cs_poll);
            Box::pin(poll_loop(ctx, cs, poll_interval))
        })),
    }
}

async fn handshake(
    conn: &Connection<QhyCodec>,
    cached_state: Arc<RwLock<CachedState>>,
    speed: u8,
) -> std::result::Result<(), QhyCodecError> {
    let version = match conn.request(Command::GetVersion).await? {
        QhyResponse::Version(v) => v,
        other => {
            return Err(QhyCodecError::InvalidResponse(format!(
                "GetVersion returned non-Version frame: {other:?}"
            )));
        }
    };
    debug!(
        firmware = %version.firmware_version,
        board = %version.board_version,
        "Q-Focuser handshake: version"
    );

    conn.request(Command::SetSpeed { speed }).await?;
    debug!(speed, "Q-Focuser handshake: speed set");

    let position = match conn.request(Command::GetPosition).await? {
        QhyResponse::Position(p) => p,
        other => {
            return Err(QhyCodecError::InvalidResponse(format!(
                "GetPosition returned non-Position frame: {other:?}"
            )));
        }
    };
    debug!(
        position = position.position,
        "Q-Focuser handshake: position"
    );

    let temp = match conn.request(Command::ReadTemperature).await? {
        QhyResponse::Temperature(t) => t,
        other => {
            return Err(QhyCodecError::InvalidResponse(format!(
                "ReadTemperature returned non-Temperature frame: {other:?}"
            )));
        }
    };
    debug!(
        outer_temp = temp.outer_temp,
        voltage = temp.voltage,
        "Q-Focuser handshake: temperature"
    );

    let mut state = cached_state.write().await;
    state.firmware_version = Some(version.firmware_version);
    state.board_version = Some(version.board_version);
    state.position = Some(position.position);
    state.outer_temp = Some(temp.outer_temp);
    state.chip_temp = Some(temp.chip_temp);
    state.voltage = Some(temp.voltage);
    Ok(())
}

async fn poll_loop(
    ctx: WhileOpen<QhyCodec>,
    cached_state: Arc<RwLock<CachedState>>,
    poll_interval: Duration,
) {
    let mut ticker = interval(poll_interval);
    // Skip the immediate first tick — handshake just populated the cache.
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = ctx.cancelled() => {
                debug!("Q-Focuser poll loop received cancellation");
                return;
            }
        }

        match ctx.request(Command::GetPosition).await {
            Ok(QhyResponse::Position(p)) => {
                let mut state = cached_state.write().await;
                apply_position(&mut state, p.position);
            }
            Ok(other) => warn!("poll: GetPosition returned unexpected variant: {other:?}"),
            Err(e) => session_err_to_warn("GetPosition", e),
        }

        match ctx.request(Command::ReadTemperature).await {
            Ok(QhyResponse::Temperature(t)) => {
                let mut state = cached_state.write().await;
                state.outer_temp = Some(t.outer_temp);
                state.chip_temp = Some(t.chip_temp);
                state.voltage = Some(t.voltage);
            }
            Ok(other) => warn!("poll: ReadTemperature returned unexpected variant: {other:?}"),
            Err(e) => session_err_to_warn("ReadTemperature", e),
        }
    }
}

fn session_err_to_warn(op: &str, err: SessionError<QhyCodecError>) {
    warn!(op, error = %err, "Q-Focuser poll request failed");
}

/// Update the cached position and clear `is_moving` if the target was
/// reached. Kept in one place so `refresh_position` and the poll loop
/// stay in lockstep on move-completion semantics.
fn apply_position(state: &mut CachedState, position: i64) {
    state.position = Some(position);
    if state.is_moving {
        if let Some(target) = state.target_position {
            if position == target {
                debug!(position, target, "move complete: target reached");
                state.is_moving = false;
                state.target_position = None;
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    //! Behaviour-level tests for the manager, driven through the mock
    //! transport factory. Race / refcount / rollback invariants are
    //! tested once for everyone in `rusty-photon-shared-transport` —
    //! they don't get re-tested here per the migration plan.

    use super::*;
    use crate::mock::MockQhyTransportFactory;

    fn make_manager() -> Arc<FocuserManager> {
        let factory = Arc::new(MockQhyTransportFactory::default());
        FocuserManager::new(Config::default(), factory)
    }

    #[tokio::test]
    async fn acquire_runs_handshake_and_seeds_cache() {
        let manager = make_manager();
        let session = manager.transport().acquire().await.unwrap();
        assert!(manager.is_available());

        let state = manager.get_cached_state().await;
        assert_eq!(state.firmware_version.as_deref(), Some("2.1.0"));
        assert_eq!(state.board_version.as_deref(), Some("1.0"));
        assert_eq!(state.position, Some(0));
        assert!((state.outer_temp.unwrap() - 25.0).abs() < 1e-6);
        assert!((state.voltage.unwrap() - 12.5).abs() < 1e-6);
        assert!(!state.is_moving);

        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn move_absolute_sets_target_and_is_moving() {
        let manager = make_manager();
        let session = manager.transport().acquire().await.unwrap();

        manager.move_absolute(&session, 5000).await.unwrap();
        let state = manager.get_cached_state().await;
        assert_eq!(state.target_position, Some(5000));
        assert!(state.is_moving);

        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn refresh_position_detects_move_completion_when_target_reached() {
        let manager = make_manager();
        let session = manager.transport().acquire().await.unwrap();

        // Mock snaps to target when |target - position| <= 1000 on each
        // GetPosition; with a starting position of 0 a 500-step move
        // completes on the first refresh.
        manager.move_absolute(&session, 500).await.unwrap();
        manager.refresh_position(&session).await.unwrap();

        let state = manager.get_cached_state().await;
        assert_eq!(state.position, Some(500));
        assert!(!state.is_moving);
        assert_eq!(state.target_position, None);

        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn abort_clears_move_state() {
        let manager = make_manager();
        let session = manager.transport().acquire().await.unwrap();

        manager.move_absolute(&session, 50000).await.unwrap();
        manager.abort(&session).await.unwrap();

        let state = manager.get_cached_state().await;
        assert!(!state.is_moving);
        assert_eq!(state.target_position, None);

        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn close_releases_transport() {
        let manager = make_manager();
        let session = manager.transport().acquire().await.unwrap();
        assert!(manager.is_available());
        session.close().await.unwrap();
        assert!(!manager.is_available());
    }
}
