//! Thin manager wrapping `SharedTransport<PpbaCodec>` plus the cached
//! protocol state both ASCOM devices read from.
//!
//! The refcount, slot, open/close transitions, command-lock arbitration,
//! and poll-task lifetime all live in
//! [`rusty_photon_shared_transport::SharedTransport`]. What stays here:
//!
//! * The PPBA handshake (ping → PA → PS, seed the cache).
//! * The 5s poll loop body that refreshes PA + PS into the cache.
//! * The cached state both devices share (status, power stats, USB hub
//!   tracking, sensor sliding-window means).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rusty_photon_shared_transport::{
    Connection, Hooks, Session, SessionError, SharedTransport, TransportFactory, WhileOpen,
};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, warn};

use crate::codec::{PpbaCodec, PpbaCodecError, PpbaResponse};
use crate::config::Config;
use crate::error::{PpbaError, Result};
use crate::mean::SensorMean;
use crate::protocol::{PpbaCommand, PpbaPowerStats, PpbaStatus};

/// Cached device state shared between the switch device and the
/// observing-conditions device.
#[derive(Debug, Clone, Default)]
pub struct CachedState {
    pub status: Option<PpbaStatus>,
    pub power_stats: Option<PpbaPowerStats>,
    /// USB hub state — not part of the PA reply, tracked separately.
    pub usb_hub_enabled: bool,
    pub last_update: Option<SystemTime>,
    pub temp_mean: SensorMean,
    pub humidity_mean: SensorMean,
    pub dewpoint_mean: SensorMean,
}

/// Manager that wraps the shared transport plus PPBA-specific cached
/// state. One instance per process; both devices hold `Arc<PpbaManager>`.
pub struct PpbaManager {
    transport: Arc<SharedTransport<PpbaCodec>>,
    cached_state: Arc<RwLock<CachedState>>,
}

impl PpbaManager {
    pub fn new(config: Config, factory: Arc<dyn TransportFactory>) -> Arc<Self> {
        // Seed sensor windows from config.
        let mut state = CachedState::default();
        let window = config.observingconditions.averaging_period;
        state.temp_mean.set_window(window);
        state.humidity_mean.set_window(window);
        state.dewpoint_mean.set_window(window);
        let cached_state = Arc::new(RwLock::new(state));

        let poll_interval = config.serial.polling_interval;
        let hooks = build_hooks(Arc::clone(&cached_state), poll_interval);
        let transport = SharedTransport::new(factory, PpbaCodec, hooks);

        Arc::new(Self {
            transport,
            cached_state,
        })
    }

    /// Access the shared transport so devices can acquire sessions.
    pub fn transport(&self) -> &Arc<SharedTransport<PpbaCodec>> {
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

    /// Reconfigure the sliding-window length on all three sensor means.
    pub async fn set_averaging_period(&self, period: Duration) {
        let mut state = self.cached_state.write().await;
        state.temp_mean.set_window(period);
        state.humidity_mean.set_window(period);
        state.dewpoint_mean.set_window(period);
        debug!(?period, "sensor averaging period updated");
    }

    /// Update the cached USB hub flag — the PPBA's PA reply doesn't
    /// include it, so the driver tracks it locally after every PU: write.
    pub async fn set_usb_hub_state(&self, enabled: bool) {
        let mut state = self.cached_state.write().await;
        state.usb_hub_enabled = enabled;
        debug!(enabled, "usb hub state updated");
    }

    /// Issue a protocol command on the device's session and return the
    /// decoded response. Used by both devices for set-commands and the
    /// PPBA-specific routing they do around them.
    pub async fn send_command(
        &self,
        session: &Session<PpbaCodec>,
        cmd: PpbaCommand,
    ) -> Result<PpbaResponse> {
        session.request(cmd).await.map_err(PpbaError::from)
    }

    /// Refresh the status (PA) cache via the caller's session.
    ///
    /// Devices use this on-demand (e.g. before validating an auto-dew
    /// write, or after a successful set command). The poll loop does the
    /// same thing internally while a session is alive.
    pub async fn refresh_status(&self, session: &Session<PpbaCodec>) -> Result<()> {
        let resp = session
            .request(PpbaCommand::Status)
            .await
            .map_err(PpbaError::from)?;
        let PpbaResponse::Status(status) = resp else {
            return Err(PpbaError::InvalidResponse(
                "PA command returned non-status frame".to_string(),
            ));
        };
        let mut state = self.cached_state.write().await;
        apply_status(&mut state, &status);
        Ok(())
    }

    /// Refresh the power-statistics (PS) cache via the caller's session.
    pub async fn refresh_power_stats(&self, session: &Session<PpbaCodec>) -> Result<()> {
        let resp = session
            .request(PpbaCommand::PowerStats)
            .await
            .map_err(PpbaError::from)?;
        let PpbaResponse::PowerStats(stats) = resp else {
            return Err(PpbaError::InvalidResponse(
                "PS command returned non-power-stats frame".to_string(),
            ));
        };
        let mut state = self.cached_state.write().await;
        state.power_stats = Some(stats);
        Ok(())
    }
}

fn build_hooks(
    cached_state: Arc<RwLock<CachedState>>,
    poll_interval: Duration,
) -> Hooks<PpbaCodec> {
    let cs_handshake = Arc::clone(&cached_state);
    let cs_poll = Arc::clone(&cached_state);
    Hooks {
        handshake: Box::new(move |conn| {
            let cs = Arc::clone(&cs_handshake);
            Box::pin(handshake(conn, cs))
        }),
        teardown: Box::new(|_| Box::pin(async {})),
        while_open: Some(Box::new(move |ctx| {
            let cs = Arc::clone(&cs_poll);
            Box::pin(poll_loop(ctx, cs, poll_interval))
        })),
    }
}

async fn handshake(
    conn: &Connection<PpbaCodec>,
    cached_state: Arc<RwLock<CachedState>>,
) -> std::result::Result<(), PpbaCodecError> {
    // Ping first — fails fast on a wrong-protocol peer.
    let ping = conn.request(PpbaCommand::Ping).await?;
    if !matches!(ping, PpbaResponse::PingOk) {
        return Err(PpbaCodecError::InvalidResponse(
            "expected PPBA_OK from ping".to_string(),
        ));
    }

    let status_resp = conn.request(PpbaCommand::Status).await?;
    let status = match status_resp {
        PpbaResponse::Status(s) => s,
        _ => {
            return Err(PpbaCodecError::InvalidResponse(
                "handshake PA returned non-status frame".to_string(),
            ));
        }
    };

    let power_resp = conn.request(PpbaCommand::PowerStats).await?;
    let power_stats = match power_resp {
        PpbaResponse::PowerStats(p) => p,
        _ => {
            return Err(PpbaCodecError::InvalidResponse(
                "handshake PS returned non-power-stats frame".to_string(),
            ));
        }
    };

    let mut state = cached_state.write().await;
    apply_status(&mut state, &status);
    state.power_stats = Some(power_stats);
    Ok(())
}

async fn poll_loop(
    ctx: WhileOpen<PpbaCodec>,
    cached_state: Arc<RwLock<CachedState>>,
    poll_interval: Duration,
) {
    let mut ticker = interval(poll_interval);
    // Skip the immediate first tick — the handshake just populated the
    // cache; first poll should wait one interval.
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = ctx.cancelled() => {
                debug!("ppba poll loop received cancellation");
                return;
            }
        }

        match ctx.request(PpbaCommand::Status).await {
            Ok(PpbaResponse::Status(status)) => {
                let mut state = cached_state.write().await;
                apply_status(&mut state, &status);
            }
            Ok(other) => warn!("ppba poll: PA returned unexpected frame variant: {other:?}"),
            Err(e) => session_err_to_warn("PA", e),
        }

        match ctx.request(PpbaCommand::PowerStats).await {
            Ok(PpbaResponse::PowerStats(stats)) => {
                let mut state = cached_state.write().await;
                state.power_stats = Some(stats);
            }
            Ok(other) => warn!("ppba poll: PS returned unexpected frame variant: {other:?}"),
            Err(e) => session_err_to_warn("PS", e),
        }
    }
}

fn session_err_to_warn(op: &str, err: SessionError<PpbaCodecError>) {
    warn!(op, error = %err, "ppba poll request failed");
}

fn apply_status(state: &mut CachedState, status: &PpbaStatus) {
    state.status = Some(status.clone());
    state.last_update = Some(SystemTime::now());
    state.temp_mean.add_sample(status.temperature);
    state.humidity_mean.add_sample(status.humidity);
    state.dewpoint_mean.add_sample(status.dewpoint);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    //! Behaviour-level tests for the manager, driven through the mock
    //! transport factory. Race / refcount / rollback invariants are
    //! tested once for everyone in `rusty-photon-shared-transport` —
    //! they don't get re-tested here per the migration plan.

    use super::*;
    use crate::mock::MockPpbaTransportFactory;

    fn make_manager() -> Arc<PpbaManager> {
        let factory = Arc::new(MockPpbaTransportFactory::default());
        PpbaManager::new(Config::default(), factory)
    }

    #[tokio::test]
    async fn acquire_runs_handshake_and_seeds_cache() {
        let manager = make_manager();
        let session = manager.transport().acquire().await.unwrap();
        assert!(manager.is_available());

        let state = manager.get_cached_state().await;
        let status = state.status.expect("status seeded by handshake");
        assert!((status.temperature - 25.0).abs() < f64::EPSILON);
        let stats = state.power_stats.expect("power stats seeded by handshake");
        assert!((stats.average_amps - 2.5).abs() < f64::EPSILON);

        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn refresh_status_updates_cache() {
        let manager = make_manager();
        let session = manager.transport().acquire().await.unwrap();

        // Mutate device state via a set command, then refresh and observe.
        manager
            .send_command(&session, PpbaCommand::SetQuad12V(false))
            .await
            .unwrap();
        manager.refresh_status(&session).await.unwrap();

        let state = manager.get_cached_state().await;
        assert!(!state.status.unwrap().quad_12v);

        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn set_averaging_period_resizes_means() {
        let manager = make_manager();
        let new_window = Duration::from_secs(120);
        manager.set_averaging_period(new_window).await;
        let state = manager.get_cached_state().await;
        assert_eq!(state.temp_mean.window(), new_window);
        assert_eq!(state.humidity_mean.window(), new_window);
        assert_eq!(state.dewpoint_mean.window(), new_window);
    }

    #[tokio::test]
    async fn set_usb_hub_state_tracks_locally() {
        let manager = make_manager();
        manager.set_usb_hub_state(true).await;
        assert!(manager.get_cached_state().await.usb_hub_enabled);
        manager.set_usb_hub_state(false).await;
        assert!(!manager.get_cached_state().await.usb_hub_enabled);
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
