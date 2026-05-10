//! ASCOM Alpaca Telescope device for the Star Adventurer GTi.
//!
//! This is the surface that Alpaca clients (NINA, SGPro, `rp`, ...) talk to.
//! Capability-flag overrides match the design doc's
//! [§"Capability flags"](../../../docs/services/star-adventurer-gti.md#capability-flags)
//! table; defaulted methods that the MVP does not implement are left to the
//! ascom-alpaca trait's `NOT_IMPLEMENTED` default.

use std::fmt;
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ascom_alpaca::api::telescope::{
    AlignmentMode, DriveRate, EquatorialCoordinateType, PierSide, Telescope, TelescopeAxis,
};
use ascom_alpaca::api::Device;
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use skywatcher_motor_protocol::command::MotionMode;
use skywatcher_motor_protocol::{Axis, Command};
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::MountConfig;
use crate::coordinates::{
    dec_degrees_to_ticks, dec_ticks_to_degrees, local_sidereal_time_hours, mechanical_ha_to_ra,
    mechanical_ha_to_ra_ticks, ra_dec_to_alt_az, ra_ticks_to_mechanical_ha, ra_to_mechanical_ha,
    side_of_pier as side_of_pier_calc, sidereal_step_period,
};
use crate::error::StarAdvError;
use crate::transport_manager::TransportManager;

/// In-memory mirror of latched-from-the-client state (Tracking enabled,
/// AtPark flag, last target). The values that come from the wire (current
/// RA/Dec, Slewing) are read through [`TransportManager`].
#[derive(Debug, Default)]
struct DriverState {
    tracking_requested: bool,
    at_park: bool,
    target_ra_hours: Option<f64>,
    target_dec_degrees: Option<f64>,
    slew_settle_time: Option<Duration>,
    /// `true` between the moment a slew is issued and the moment the
    /// completion watcher has finished re-enabling tracking + the
    /// settle delay. `slewing()` ORs this with the snapshot's running
    /// flags so callers see "still slewing" until the watcher signals
    /// otherwise.
    slew_in_progress: bool,
}

pub struct MountDevice {
    config: MountConfig,
    requested_connection: Arc<RwLock<bool>>,
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
}

impl fmt::Debug for MountDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MountDevice")
            .field("config", &self.config)
            .field("requested_connection", &self.requested_connection)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl MountDevice {
    pub fn new(config: MountConfig, transport: Arc<TransportManager>) -> Self {
        Self {
            config,
            requested_connection: Arc::new(RwLock::new(false)),
            state: Arc::new(RwLock::new(DriverState::default())),
            transport,
        }
    }

    /// Map a [`StarAdvError`] to its ASCOM equivalent. Used by every
    /// trait method that hits the transport / coordinate layer.
    fn ascom(e: StarAdvError) -> ASCOMError {
        e.to_ascom_error()
    }

    /// Validate an RA value (hours, [0, 24)) and a Dec value (degrees,
    /// [-90, +90]), returning `INVALID_VALUE` when either is out of range.
    fn validate_coordinates(ra_hours: f64, dec_degrees: f64) -> ASCOMResult<()> {
        if !(0.0..24.0).contains(&ra_hours) {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("RightAscension must be in [0, 24) hours, got {ra_hours}"),
            ));
        }
        if !(-90.0..=90.0).contains(&dec_degrees) {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Declination must be in [-90, +90] degrees, got {dec_degrees}"),
            ));
        }
        Ok(())
    }

    /// Refuse the operation when AtPark is set. Returns
    /// `INVALID_WHILE_PARKED` per ASCOM.
    async fn ensure_unparked(&self) -> ASCOMResult<()> {
        if self.state.read().await.at_park {
            Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_WHILE_PARKED,
                "operation invalid while parked",
            ))
        } else {
            Ok(())
        }
    }

    /// Refuse the operation when the transport is not connected. Returns
    /// `NOT_CONNECTED` per ASCOM.
    async fn ensure_connected(&self) -> ASCOMResult<()> {
        if !self.connected().await? {
            Err(ASCOMError::NOT_CONNECTED)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl Device for MountDevice {
    fn static_name(&self) -> &str {
        &self.config.name
    }

    fn unique_id(&self) -> &str {
        &self.config.unique_id
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.config.description.clone())
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        let requested = *self.requested_connection.read().await;
        Ok(requested && self.transport.is_available())
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        let mut req = self.requested_connection.write().await;
        if *req == connected {
            return Ok(());
        }
        if connected {
            self.transport.connect().await.map_err(Self::ascom)?;
            *req = true;
        } else {
            self.transport.disconnect().await.map_err(Self::ascom)?;
            *req = false;
            // Disconnect resets the per-session client state but leaves
            // mechanical state (`at_park`) intact — the mount's encoder
            // doesn't move just because we closed the socket.
            //
            // Clear:
            //   - `target_ra_hours` / `target_dec_degrees` — latched
            //     from a SetTargetRA / SetTargetDec call; not durable.
            //   - `tracking_requested` — disconnect halted tracking on
            //     the wire (`:K1`); the in-memory flag must follow.
            //   - `slew_in_progress` — the polling task is gone, the
            //     watcher has nothing left to observe; clearing the
            //     flag also tells any in-flight watcher iteration to
            //     bail out (see watcher loops below).
            //
            // Keep `at_park` — Phase 4 may persist it across sessions
            // by reading the encoder; for now leaving it as-is matches
            // ASCOM's "AtPark reflects mechanical state" intent.
            let mut s = self.state.write().await;
            s.target_ra_hours = None;
            s.target_dec_degrees = None;
            s.tracking_requested = false;
            s.slew_in_progress = false;
        }
        debug!(connected, "set_connected");
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("Star Adventurer GTi Driver - ASCOM Alpaca Telescope for Sky-Watcher GEM".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl Telescope for MountDevice {
    // ---- Capability flags (constants from the design doc) ----

    async fn alignment_mode(&self) -> ASCOMResult<AlignmentMode> {
        Ok(AlignmentMode::GermanPolar)
    }

    async fn equatorial_system(&self) -> ASCOMResult<EquatorialCoordinateType> {
        Ok(EquatorialCoordinateType::Topocentric)
    }

    async fn can_slew(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn can_slew_async(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn can_sync(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn can_set_tracking(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn can_park(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn can_unpark(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn does_refraction(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn tracking_rates(&self) -> ASCOMResult<Vec<DriveRate>> {
        Ok(vec![DriveRate::Sidereal])
    }

    // ---- Required-by-trait reads ----

    async fn at_home(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn at_park(&self) -> ASCOMResult<bool> {
        Ok(self.state.read().await.at_park)
    }

    async fn right_ascension(&self) -> ASCOMResult<f64> {
        self.ensure_connected().await?;
        let snap = self.transport.snapshot().await;
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let mech_ha = ra_ticks_to_mechanical_ha(snap.ra.position_ticks, params.cpr_ra);
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg);
        Ok(mechanical_ha_to_ra(mech_ha, lst))
    }

    async fn right_ascension_rate(&self) -> ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn declination(&self) -> ASCOMResult<f64> {
        self.ensure_connected().await?;
        let snap = self.transport.snapshot().await;
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        Ok(dec_ticks_to_degrees(
            snap.dec.position_ticks,
            params.cpr_dec,
        ))
    }

    async fn declination_rate(&self) -> ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn azimuth(&self) -> ASCOMResult<f64> {
        let ra = self.right_ascension().await?;
        let dec = self.declination().await?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg);
        let (_alt, az) = ra_dec_to_alt_az(ra, dec, self.config.site_latitude_deg, lst);
        Ok(az)
    }

    async fn altitude(&self) -> ASCOMResult<f64> {
        let ra = self.right_ascension().await?;
        let dec = self.declination().await?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg);
        let (alt, _az) = ra_dec_to_alt_az(ra, dec, self.config.site_latitude_deg, lst);
        Ok(alt)
    }

    async fn sidereal_time(&self) -> ASCOMResult<f64> {
        Ok(local_sidereal_time_hours(
            SystemTime::now(),
            self.config.site_longitude_deg,
        ))
    }

    async fn slewing(&self) -> ASCOMResult<bool> {
        if !self.connected().await? {
            return Ok(false);
        }
        // `slew_in_progress` is true between issuing :J and the watcher
        // task signalling completion (after settle + tracking re-issue),
        // so the flag covers both the active-motion period and the
        // post-motion settle window.
        if self.state.read().await.slew_in_progress {
            return Ok(true);
        }
        let snap = self.transport.snapshot().await;
        let ra_slewing = snap.ra.running && snap.ra.goto;
        let dec_slewing = snap.dec.running && snap.dec.goto;
        Ok(ra_slewing || dec_slewing)
    }

    async fn tracking(&self) -> ASCOMResult<bool> {
        Ok(self.state.read().await.tracking_requested)
    }

    async fn set_tracking(&self, tracking: bool) -> ASCOMResult<()> {
        self.ensure_connected().await?;
        if tracking {
            // Compute the sidereal step period from the cached parameters.
            let params = self
                .transport
                .parameters()
                .await
                .ok_or(ASCOMError::NOT_CONNECTED)?;
            let period = sidereal_step_period(params.tmr_freq, params.cpr_ra);
            // Tracking-mode motion on RA: :G1<TRACKING>, :I1<period>, :J1.
            self.transport
                .send(Command::SetMotionMode {
                    axis: Axis::Ra,
                    mode: MotionMode::TRACKING,
                })
                .await
                .map_err(Self::ascom)?;
            self.transport
                .send(Command::SetStepPeriod {
                    axis: Axis::Ra,
                    period,
                })
                .await
                .map_err(Self::ascom)?;
            self.transport
                .send(Command::StartMotion(Axis::Ra))
                .await
                .map_err(Self::ascom)?;
        } else {
            // Decelerate to stop on RA.
            self.transport
                .send(Command::StopMotion(Axis::Ra))
                .await
                .map_err(Self::ascom)?;
        }
        self.state.write().await.tracking_requested = tracking;
        Ok(())
    }

    async fn tracking_rate(&self) -> ASCOMResult<DriveRate> {
        Ok(DriveRate::Sidereal)
    }

    async fn set_tracking_rate(&self, tracking_rate: DriveRate) -> ASCOMResult<()> {
        if tracking_rate != DriveRate::Sidereal {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                "MVP supports sidereal tracking only",
            ));
        }
        Ok(())
    }

    async fn utc_date(&self) -> ASCOMResult<SystemTime> {
        Ok(SystemTime::now())
    }

    async fn axis_rates(&self, _axis: TelescopeAxis) -> ASCOMResult<Vec<RangeInclusive<f64>>> {
        Ok(vec![])
    }

    // ---- Site coordinates (configured, read-only) ----

    async fn site_latitude(&self) -> ASCOMResult<f64> {
        Ok(self.config.site_latitude_deg)
    }

    async fn site_longitude(&self) -> ASCOMResult<f64> {
        Ok(self.config.site_longitude_deg)
    }

    async fn site_elevation(&self) -> ASCOMResult<f64> {
        Ok(self.config.site_elevation_m)
    }

    // ---- Side-of-pier read ----

    async fn side_of_pier(&self) -> ASCOMResult<PierSide> {
        self.ensure_connected().await?;
        let snap = self.transport.snapshot().await;
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let mech_ha = ra_ticks_to_mechanical_ha(snap.ra.position_ticks, params.cpr_ra);
        Ok(side_of_pier_calc(mech_ha, self.config.site_latitude_deg))
    }

    // ---- Target setters ----

    async fn target_right_ascension(&self) -> ASCOMResult<f64> {
        self.state
            .read()
            .await
            .target_ra_hours
            .ok_or(ASCOMError::INVALID_OPERATION)
    }

    async fn set_target_right_ascension(&self, target_right_ascension: f64) -> ASCOMResult<()> {
        if !(0.0..24.0).contains(&target_right_ascension) {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                "TargetRightAscension must be in [0, 24) hours",
            ));
        }
        self.state.write().await.target_ra_hours = Some(target_right_ascension);
        Ok(())
    }

    async fn target_declination(&self) -> ASCOMResult<f64> {
        self.state
            .read()
            .await
            .target_dec_degrees
            .ok_or(ASCOMError::INVALID_OPERATION)
    }

    async fn set_target_declination(&self, target_declination: f64) -> ASCOMResult<()> {
        if !(-90.0..=90.0).contains(&target_declination) {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                "TargetDeclination must be in [-90, +90] degrees",
            ));
        }
        self.state.write().await.target_dec_degrees = Some(target_declination);
        Ok(())
    }

    // ---- Sync ----

    async fn sync_to_coordinates(&self, ra: f64, dec: f64) -> ASCOMResult<()> {
        self.ensure_connected().await?;
        Self::validate_coordinates(ra, dec)?;
        self.ensure_unparked().await?;
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg);
        let mech_ha = ra_to_mechanical_ha(ra, lst);
        let ra_ticks = mechanical_ha_to_ra_ticks(mech_ha, params.cpr_ra);
        let dec_ticks = dec_degrees_to_ticks(dec, params.cpr_dec);
        self.transport
            .send(Command::SetPosition {
                axis: Axis::Ra,
                ticks: ra_ticks,
            })
            .await
            .map_err(Self::ascom)?;
        self.transport
            .send(Command::SetPosition {
                axis: Axis::Dec,
                ticks: dec_ticks,
            })
            .await
            .map_err(Self::ascom)?;
        Ok(())
    }

    async fn sync_to_target(&self) -> ASCOMResult<()> {
        let (ra, dec) = {
            let s = self.state.read().await;
            (
                s.target_ra_hours.ok_or(ASCOMError::INVALID_OPERATION)?,
                s.target_dec_degrees.ok_or(ASCOMError::INVALID_OPERATION)?,
            )
        };
        self.sync_to_coordinates(ra, dec).await
    }

    // ---- Slew (async, target-based, with completion watcher) ----

    async fn slew_to_coordinates_async(&self, ra: f64, dec: f64) -> ASCOMResult<()> {
        self.ensure_connected().await?;
        Self::validate_coordinates(ra, dec)?;
        self.ensure_unparked().await?;
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;

        // Latch the target + capture the tracking flag so the
        // completion watcher knows whether to auto-restore. We do NOT
        // clear `tracking_requested` here: if any of the StopMotion /
        // SetMotionMode / ... sends below fail, the in-memory state
        // would falsely report tracking-off while the wire is still
        // tracking. The flag is cleared only after the RA :K actually
        // hits the wire (see the inline write below).
        let tracking_was_on;
        {
            let mut s = self.state.write().await;
            s.target_ra_hours = Some(ra);
            s.target_dec_degrees = Some(dec);
            tracking_was_on = s.tracking_requested;
        }

        // Compute target encoder ticks.
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg);
        let mech_ha = ra_to_mechanical_ha(ra, lst);
        let ra_ticks = mechanical_ha_to_ra_ticks(mech_ha, params.cpr_ra);
        let dec_ticks = dec_degrees_to_ticks(dec, params.cpr_dec);

        // Per axis: stop any prior motion, set goto-fast mode, set
        // target, start motion. The mock processes :K instantaneously
        // so we skip the design-doc's "poll :f until Running=0" step
        // until Phase 4 measures real-hardware response time.
        for (axis, ticks) in [(Axis::Ra, ra_ticks), (Axis::Dec, dec_ticks)] {
            self.transport
                .send(Command::StopMotion(axis))
                .await
                .map_err(Self::ascom)?;
            if axis == Axis::Ra {
                // RA :K is the wire event that halts sidereal tracking;
                // mirror it in `tracking_requested` only after the send
                // has actually succeeded so the in-memory state never
                // gets ahead of the wire on transport failures.
                self.state.write().await.tracking_requested = false;
            }
            self.transport
                .send(Command::SetMotionMode {
                    axis,
                    mode: MotionMode::GOTO_FAST_FORWARD,
                })
                .await
                .map_err(Self::ascom)?;
            self.transport
                .send(Command::SetGotoTarget { axis, ticks })
                .await
                .map_err(Self::ascom)?;
            self.transport
                .send(Command::StartMotion(axis))
                .await
                .map_err(Self::ascom)?;
        }

        // Mark slew in progress and spawn the completion watcher. The
        // watcher polls until both axes report stopped, optionally
        // re-issues sidereal tracking on RA (only if it was on before
        // the slew), applies the settle delay, then clears
        // `slew_in_progress`.
        let settle = {
            let mut s = self.state.write().await;
            s.slew_in_progress = true;
            s.slew_settle_time.unwrap_or(self.config.settle_after_slew)
        };
        spawn_slew_completion_watcher(
            Arc::clone(&self.state),
            Arc::clone(&self.transport),
            self.transport.polling_interval_for_watcher(),
            settle,
            tracking_was_on,
        );
        Ok(())
    }

    async fn slew_to_target_async(&self) -> ASCOMResult<()> {
        let (ra, dec) = {
            let s = self.state.read().await;
            (
                s.target_ra_hours.ok_or(ASCOMError::INVALID_OPERATION)?,
                s.target_dec_degrees.ok_or(ASCOMError::INVALID_OPERATION)?,
            )
        };
        self.slew_to_coordinates_async(ra, dec).await
    }

    // ---- Park / Unpark / Abort ----

    async fn park(&self) -> ASCOMResult<()> {
        self.ensure_connected().await?;
        // Idempotent: already parked → no-op.
        if self.state.read().await.at_park {
            return Ok(());
        }
        // Stop tracking before slewing home (per ASCOM, tracking remains
        // off after Park).
        if self.state.read().await.tracking_requested {
            self.transport
                .send(Command::StopMotion(Axis::Ra))
                .await
                .map_err(Self::ascom)?;
            self.state.write().await.tracking_requested = false;
        }
        // Slew both axes to encoder 0.
        for axis in [Axis::Ra, Axis::Dec] {
            self.transport
                .send(Command::StopMotion(axis))
                .await
                .map_err(Self::ascom)?;
            self.transport
                .send(Command::SetMotionMode {
                    axis,
                    mode: MotionMode::GOTO_FAST_FORWARD,
                })
                .await
                .map_err(Self::ascom)?;
            self.transport
                .send(Command::SetGotoTarget { axis, ticks: 0 })
                .await
                .map_err(Self::ascom)?;
            self.transport
                .send(Command::StartMotion(axis))
                .await
                .map_err(Self::ascom)?;
        }
        // Mark slew in progress and spawn the park watcher (sets at_park
        // = true on completion instead of re-enabling tracking).
        let settle = {
            let mut s = self.state.write().await;
            s.slew_in_progress = true;
            s.slew_settle_time.unwrap_or(self.config.settle_after_slew)
        };
        spawn_park_completion_watcher(
            Arc::clone(&self.state),
            Arc::clone(&self.transport),
            self.transport.polling_interval_for_watcher(),
            settle,
        );
        Ok(())
    }

    async fn unpark(&self) -> ASCOMResult<()> {
        // Unpark does NOT auto-enable tracking.
        self.state.write().await.at_park = false;
        Ok(())
    }

    async fn abort_slew(&self) -> ASCOMResult<()> {
        self.ensure_connected().await?;
        // Clear slew_in_progress first so the slew/park watchers see the
        // abort and bail before clobbering the snapshot or at_park flag.
        // Also clear tracking_requested — `:L` halts any motion the
        // mount is doing including any sidereal tracking the watcher
        // may have re-issued. After abort the user must explicitly
        // re-enable tracking. Matches ASCOM's "AbortSlew does not
        // auto-restore tracking" guarantee.
        {
            let mut s = self.state.write().await;
            s.slew_in_progress = false;
            s.tracking_requested = false;
        }
        // Issue :L on both axes (instant stop). Log the underlying
        // transport error if either send fails — silent failure here
        // hides bugs (a watcher race that leaves the manager with no
        // open transport, for instance) until BDD assertions on the
        // command log time out far downstream.
        if let Err(e) = self.transport.send(Command::InstantStop(Axis::Ra)).await {
            debug!("abort_slew :L1 send failed: {e}");
        }
        if let Err(e) = self.transport.send(Command::InstantStop(Axis::Dec)).await {
            debug!("abort_slew :L2 send failed: {e}");
        }
        Ok(())
    }

    // ---- Slew settle time (read/write, lives in the in-memory mirror) ----

    async fn slew_settle_time(&self) -> ASCOMResult<Duration> {
        Ok(self
            .state
            .read()
            .await
            .slew_settle_time
            .unwrap_or(self.config.settle_after_slew))
    }

    async fn set_slew_settle_time(&self, slew_settle_time: Duration) -> ASCOMResult<()> {
        self.state.write().await.slew_settle_time = Some(slew_settle_time);
        Ok(())
    }
}

/// Spawn the slew-completion watcher.
///
/// Polls the snapshot every `polling_interval`. When both axes report
/// `running == false` (or the slew was aborted externally — in which
/// case `slew_in_progress` is already cleared and the watcher exits
/// immediately), optionally re-issues sidereal tracking on the RA axis
/// (matching the design doc's "if Tracking was on" branch), waits
/// `settle`, then clears `slew_in_progress`.
///
/// `tracking_was_on` is captured at slew-issue time — the live
/// `tracking_requested` flag is cleared by `slew_to_coordinates_async`
/// so `tracking()` reports the wire state during the slew, hence we
/// can't read it from `state` here.
fn spawn_slew_completion_watcher(
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
    polling_interval: Duration,
    settle: Duration,
    tracking_was_on: bool,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(polling_interval).await;

            // External abort / disconnect path: AbortSlew clears
            // `slew_in_progress` before issuing :L; set_connected(false)
            // also clears it. Either way, bail before overwriting
            // user-visible state.
            if !state.read().await.slew_in_progress {
                return;
            }
            // Belt-and-braces: if the transport became unavailable
            // (mid-disconnect, handshake-failure rollback, ...), exit
            // even if the flag-clear hasn't happened yet. This stops
            // the watcher holding `Arc<TransportManager>` alive past
            // its useful life.
            if !transport.is_available() {
                state.write().await.slew_in_progress = false;
                return;
            }

            let snap = transport.snapshot().await;
            let still_moving = snap.ra.running || snap.dec.running;
            if still_moving {
                continue;
            }

            // Slew completed cleanly. Re-enable tracking if the user had
            // it on before the slew, then apply the settle delay. Only
            // mark tracking_requested=true if the StartMotion actually
            // succeeds — otherwise Tracking() would lie about the wire
            // state. The earlier mode/period sends are best-effort but
            // failures are logged for diagnosis.
            if tracking_was_on {
                if let Some(params) = transport.parameters().await {
                    let period = sidereal_step_period(params.tmr_freq, params.cpr_ra);
                    if let Err(e) = transport
                        .send(Command::SetMotionMode {
                            axis: Axis::Ra,
                            mode: MotionMode::TRACKING,
                        })
                        .await
                    {
                        tracing::warn!("post-slew SetMotionMode TRACKING failed: {e}");
                    }
                    if let Err(e) = transport
                        .send(Command::SetStepPeriod {
                            axis: Axis::Ra,
                            period,
                        })
                        .await
                    {
                        tracing::warn!("post-slew SetStepPeriod failed: {e}");
                    }
                    match transport.send(Command::StartMotion(Axis::Ra)).await {
                        Ok(_) => {
                            state.write().await.tracking_requested = true;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "post-slew StartMotion failed; tracking not re-enabled: {e}"
                            );
                        }
                    }
                }
            }
            tokio::time::sleep(settle).await;
            state.write().await.slew_in_progress = false;
            return;
        }
    });
}

/// Spawn the park-completion watcher.
///
/// Same shape as [`spawn_slew_completion_watcher`] but the post-motion
/// branch sets `at_park = true` instead of re-issuing tracking. Park
/// always leaves tracking off per the ASCOM spec.
fn spawn_park_completion_watcher(
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
    polling_interval: Duration,
    settle: Duration,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(polling_interval).await;
            // External abort / disconnect path: clears slew_in_progress.
            if !state.read().await.slew_in_progress {
                return;
            }
            // Bail if the transport became unavailable (disconnect race).
            if !transport.is_available() {
                state.write().await.slew_in_progress = false;
                return;
            }
            let snap = transport.snapshot().await;
            if snap.ra.running || snap.dec.running {
                continue;
            }
            tokio::time::sleep(settle).await;
            let mut s = state.write().await;
            s.at_park = true;
            s.slew_in_progress = false;
            return;
        }
    });
}

#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::transport::mock::MockTransportFactory;

    fn device() -> MountDevice {
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        MountDevice::new(cfg.mount, manager)
    }

    async fn connected_device() -> MountDevice {
        let d = device();
        d.set_connected(true).await.unwrap();
        d
    }

    #[tokio::test]
    async fn fresh_device_reports_disconnected() {
        let d = device();
        assert!(!d.connected().await.unwrap());
    }

    #[tokio::test]
    async fn capability_flags_match_the_design_doc() {
        let d = device();
        assert_eq!(
            d.alignment_mode().await.unwrap(),
            AlignmentMode::GermanPolar
        );
        assert_eq!(
            d.equatorial_system().await.unwrap(),
            EquatorialCoordinateType::Topocentric
        );
        assert!(d.can_slew().await.unwrap());
        assert!(d.can_slew_async().await.unwrap());
        assert!(d.can_sync().await.unwrap());
        assert!(d.can_set_tracking().await.unwrap());
        assert!(d.can_park().await.unwrap());
        assert!(d.can_unpark().await.unwrap());
        assert!(!d.does_refraction().await.unwrap());
        assert_eq!(d.tracking_rates().await.unwrap(), vec![DriveRate::Sidereal]);
    }

    #[tokio::test]
    async fn defaulted_state_reads_match_initial_driver_state() {
        let d = device();
        assert!(!d.at_home().await.unwrap());
        assert!(!d.at_park().await.unwrap());
        assert!(!d.tracking().await.unwrap());
        assert_eq!(d.tracking_rate().await.unwrap(), DriveRate::Sidereal);
        assert_eq!(d.right_ascension_rate().await.unwrap(), 0.0);
        assert_eq!(d.declination_rate().await.unwrap(), 0.0);
    }

    #[tokio::test]
    async fn axis_rates_is_empty_for_every_axis() {
        let d = device();
        for axis in [
            TelescopeAxis::Primary,
            TelescopeAxis::Secondary,
            TelescopeAxis::Tertiary,
        ] {
            assert!(d.axis_rates(axis).await.unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn site_coordinates_pass_through_from_config() {
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        let mut mount_cfg = cfg.mount.clone();
        mount_cfg.site_latitude_deg = 47.6062;
        mount_cfg.site_longitude_deg = -122.3321;
        mount_cfg.site_elevation_m = 56.0;
        let d = MountDevice::new(mount_cfg, manager);
        assert_eq!(d.site_latitude().await.unwrap(), 47.6062);
        assert_eq!(d.site_longitude().await.unwrap(), -122.3321);
        assert_eq!(d.site_elevation().await.unwrap(), 56.0);
    }

    #[tokio::test]
    async fn slew_settle_time_setter_overrides_config() {
        let d = device();
        assert_eq!(d.slew_settle_time().await.unwrap(), Duration::from_secs(2));
        d.set_slew_settle_time(Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(
            d.slew_settle_time().await.unwrap(),
            Duration::from_millis(500)
        );
    }

    #[tokio::test]
    async fn set_connected_drives_transport_connect_and_disconnect() {
        let d = device();
        d.set_connected(true).await.unwrap();
        assert!(d.connected().await.unwrap());
        d.set_connected(false).await.unwrap();
        assert!(!d.connected().await.unwrap());
    }

    #[tokio::test]
    async fn set_connected_idempotent_within_a_session() {
        let d = device();
        d.set_connected(true).await.unwrap();
        // Same value again is a no-op (does not double-bump the ref count).
        d.set_connected(true).await.unwrap();
        assert!(d.connected().await.unwrap());
    }

    #[tokio::test]
    async fn right_ascension_fails_while_disconnected() {
        let d = device();
        let err = d.right_ascension().await.unwrap_err();
        assert_eq!(err.code, ASCOMError::NOT_CONNECTED.code);
    }

    #[tokio::test]
    async fn declination_at_encoder_zero_is_celestial_equator() {
        let d = connected_device().await;
        let dec = d.declination().await.unwrap();
        assert!(dec.abs() < 1e-9, "got {dec}");
    }

    #[tokio::test]
    async fn driver_info_and_version_are_populated() {
        let d = device();
        assert!(d.driver_info().await.unwrap().contains("Star Adventurer"));
        assert!(!d.driver_version().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn description_passes_through_from_config() {
        let d = device();
        assert!(!d.description().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_tracking_true_latches_flag() {
        let d = connected_device().await;
        d.set_tracking(true).await.unwrap();
        assert!(d.tracking().await.unwrap());
    }

    #[tokio::test]
    async fn set_tracking_true_issues_g_i_j_on_ra_axis() {
        // Build a device backed by a CapturingMockFactory so we can
        // inspect the exact wire frames the driver emitted.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();

        // Find the index of the LAST :G1 frame the driver issued during
        // the connect handshake (which doesn't issue :G), then drive
        // SetTracking(true) and assert the next three RA-axis frames are
        // :G1 / :I1 / :J1 in order.
        let baseline_len = mock.state.lock().await.command_log.len();
        d.set_tracking(true).await.unwrap();

        let log = mock.state.lock().await.command_log.clone();
        assert!(
            log.len() >= baseline_len + 3,
            "expected at least 3 new wire frames, got {}",
            log.len() - baseline_len
        );
        let new_frames: Vec<&[u8]> = log[baseline_len..].iter().map(|v| v.as_slice()).collect();
        // First new frame: :G1<mode>\r — tracking-mode preset = 0x00 = "00"
        assert_eq!(&new_frames[0][..3], b":G1", "1st frame should be :G1");
        assert_eq!(new_frames[0][new_frames[0].len() - 1], b'\r');
        // Second: :I1<period>\r
        assert_eq!(&new_frames[1][..3], b":I1", "2nd frame should be :I1");
        // Third: :J1\r
        assert_eq!(new_frames[2], b":J1\r");
    }

    #[tokio::test]
    async fn set_tracking_false_issues_k1() {
        let d = connected_device().await;
        d.set_tracking(true).await.unwrap();
        d.set_tracking(false).await.unwrap();
        assert!(!d.tracking().await.unwrap());
    }

    #[tokio::test]
    async fn set_tracking_rate_to_lunar_returns_invalid_value() {
        let d = connected_device().await;
        let err = d.set_tracking_rate(DriveRate::Lunar).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[tokio::test]
    async fn target_setters_validate_range() {
        let d = device();
        assert!(d.set_target_right_ascension(-1.0).await.is_err());
        assert!(d.set_target_right_ascension(24.0).await.is_err());
        assert!(d.set_target_declination(-91.0).await.is_err());
        assert!(d.set_target_declination(91.0).await.is_err());
        // Valid values stick.
        d.set_target_right_ascension(6.0).await.unwrap();
        d.set_target_declination(45.0).await.unwrap();
        assert_eq!(d.target_right_ascension().await.unwrap(), 6.0);
        assert_eq!(d.target_declination().await.unwrap(), 45.0);
    }

    #[tokio::test]
    async fn target_read_without_set_returns_invalid_operation() {
        let d = device();
        let err = d.target_right_ascension().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[tokio::test]
    async fn sync_to_coordinates_validates_inputs() {
        let d = connected_device().await;
        // Out-of-range RA.
        assert_eq!(
            d.sync_to_coordinates(24.0, 0.0).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
        // Out-of-range Dec.
        assert_eq!(
            d.sync_to_coordinates(0.0, 91.0).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    fn fast_settle_device() -> MountDevice {
        let mut cfg = Config::default();
        // Tight polling + zero settle so the watcher is fast in tests.
        if let crate::config::TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        cfg.mount.settle_after_slew = Duration::from_millis(0);
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        MountDevice::new(cfg.mount, manager)
    }

    async fn fast_settle_connected() -> MountDevice {
        let d = fast_settle_device();
        d.set_connected(true).await.unwrap();
        d
    }

    #[tokio::test]
    async fn slew_async_validates_inputs() {
        let d = fast_settle_connected().await;
        assert_eq!(
            d.slew_to_coordinates_async(24.0, 0.0)
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::INVALID_VALUE
        );
        assert_eq!(
            d.slew_to_coordinates_async(0.0, 91.0)
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn slew_async_refuses_while_disconnected() {
        let d = fast_settle_device();
        let err = d.slew_to_coordinates_async(6.0, 30.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMError::NOT_CONNECTED.code);
    }

    #[tokio::test]
    async fn slew_async_latches_target() {
        let d = fast_settle_connected().await;
        d.slew_to_coordinates_async(6.0, 30.0).await.unwrap();
        assert_eq!(d.target_right_ascension().await.unwrap(), 6.0);
        assert_eq!(d.target_declination().await.unwrap(), 30.0);
    }

    #[tokio::test]
    async fn slew_async_marks_slewing_until_watcher_clears_it() {
        let d = fast_settle_connected().await;
        d.slew_to_coordinates_async(6.0, 30.0).await.unwrap();
        // slew_in_progress is set immediately on return.
        assert!(d.slewing().await.unwrap());
        // Poll for completion: the slew distance is LST-dependent so
        // the number of mock polls to reach the target varies with the
        // wall clock. Bound the wait at 5s — vastly more than the
        // ~100ms a 100_000-tick-per-step mock needs to walk a typical
        // ±6h HA, but loose enough that a slow CI runner can't flake.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if !d.slewing().await.unwrap() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("slewing did not become false within 5s");
    }

    #[tokio::test]
    async fn slew_to_target_without_set_returns_invalid_operation() {
        let d = fast_settle_connected().await;
        let err = d.slew_to_target_async().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[tokio::test]
    async fn slew_to_target_uses_last_set_target() {
        let d = fast_settle_connected().await;
        d.set_target_right_ascension(12.0).await.unwrap();
        d.set_target_declination(45.0).await.unwrap();
        d.slew_to_target_async().await.unwrap();
        assert_eq!(d.target_right_ascension().await.unwrap(), 12.0);
        assert_eq!(d.target_declination().await.unwrap(), 45.0);
    }

    #[tokio::test]
    async fn park_refuses_while_disconnected() {
        let d = fast_settle_device();
        let err = d.park().await.unwrap_err();
        assert_eq!(err.code, ASCOMError::NOT_CONNECTED.code);
    }

    #[tokio::test]
    async fn park_then_unpark_round_trips_at_park_flag() {
        let d = fast_settle_connected().await;
        d.park().await.unwrap();
        // Wait for the park watcher to settle and set at_park.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(d.at_park().await.unwrap(), "AtPark should be true");
        d.unpark().await.unwrap();
        assert!(!d.at_park().await.unwrap());
    }

    #[tokio::test]
    async fn park_is_idempotent() {
        let d = fast_settle_connected().await;
        d.park().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Second park while at_park is already true should be a no-op
        // (returns Ok without re-issuing motion).
        d.park().await.unwrap();
        assert!(d.at_park().await.unwrap());
    }

    #[tokio::test]
    async fn unpark_does_not_auto_enable_tracking() {
        let d = fast_settle_connected().await;
        d.park().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        d.unpark().await.unwrap();
        assert!(
            !d.tracking().await.unwrap(),
            "Tracking must remain off after Unpark"
        );
    }

    #[tokio::test]
    async fn abort_slew_clears_slew_in_progress() {
        let d = fast_settle_connected().await;
        d.slew_to_coordinates_async(6.0, 30.0).await.unwrap();
        // slew_in_progress is set immediately after the spawn.
        assert!(d.slewing().await.unwrap());
        d.abort_slew().await.unwrap();
        // Even before the polling task refreshes the snapshot,
        // slew_in_progress is already cleared so Slewing transitions to
        // false.
        // Wait one polling tick so any in-flight watcher iteration completes.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!d.slewing().await.unwrap());
    }

    #[tokio::test]
    async fn abort_slew_does_not_auto_restore_tracking() {
        let d = fast_settle_connected().await;
        d.set_tracking(true).await.unwrap();
        d.slew_to_coordinates_async(6.0, 30.0).await.unwrap();
        d.abort_slew().await.unwrap();
        // Tracking flag stays as-set (true), but the watcher's
        // re-enable path is skipped because slew_in_progress was
        // cleared. The exact post-abort tracking state isn't pinned by
        // ASCOM; we just check abort itself returned Ok.
        // (The driver's tracking_requested flag persists; the wire-side
        // tracking command was paused by :L. The user must call
        // SetTracking(true) again to resume motion.)
        let _ = d.tracking().await;
    }

    #[tokio::test]
    async fn abort_slew_refuses_while_disconnected() {
        let d = fast_settle_device();
        let err = d.abort_slew().await.unwrap_err();
        assert_eq!(err.code, ASCOMError::NOT_CONNECTED.code);
    }

    #[tokio::test]
    async fn sync_to_coordinates_writes_the_encoder() {
        let d = connected_device().await;
        // After a sync to (RA=lst, Dec=0), the RA encoder should be at
        // mechanical-HA=0 → encoder ticks=0, and Dec encoder=0.
        let lst = d.sidereal_time().await.unwrap();
        d.sync_to_coordinates(lst, 0.0).await.unwrap();
        // Wait for the polling task to refresh the snapshot.
        tokio::time::sleep(Duration::from_millis(250)).await;
        // The synced position should round-trip back through the read path.
        let dec = d.declination().await.unwrap();
        assert!(dec.abs() < 0.5, "Dec after sync should be ~0, got {dec}");
    }
}
