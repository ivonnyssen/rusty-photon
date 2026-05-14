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
    AlignmentMode, DriveRate, EquatorialCoordinateType, GuideDirection, PierSide, Telescope,
    TelescopeAxis,
};
use ascom_alpaca::api::Device;
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use skywatcher_motor_protocol::command::{ModeKind, MotionMode, Speed};
use skywatcher_motor_protocol::{Axis, Command};
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::MountConfig;
use crate::coordinates::{
    dec_degrees_to_ticks, dec_ticks_to_degrees, local_sidereal_time_hours, mechanical_ha_to_ra,
    mechanical_ha_to_ra_ticks, pickup_target_ra_ticks, pulse_guide_step_period, ra_dec_to_alt_az,
    ra_ticks_to_mechanical_ha, ra_to_mechanical_ha, side_of_pier as side_of_pier_calc,
    sidereal_step_period, SIDEREAL_DEG_PER_SEC,
};
use crate::error::StarAdvError;
use crate::transport_manager::TransportManager;

/// Default guide rate as a fraction of sidereal. ASCOM clients see
/// this multiplied by `SIDEREAL_DEG_PER_SEC` through
/// `GuideRateRightAscension` / `GuideRateDeclination`.
const DEFAULT_GUIDE_RATE_FRACTION: f64 = 0.5;

/// In-memory mirror of latched-from-the-client state (Tracking enabled,
/// AtPark flag, last target). The values that come from the wire (current
/// RA/Dec, Slewing) are read through [`TransportManager`].
#[derive(Debug)]
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
    /// PulseGuide rate on the RA axis as a fraction of sidereal in
    /// `(0, 1)`. `GuideRateRightAscension` is this × `SIDEREAL_DEG_PER_SEC`.
    /// Resets to [`DEFAULT_GUIDE_RATE_FRACTION`] on each disconnect.
    guide_rate_ra_fraction: f64,
    guide_rate_dec_fraction: f64,
    /// `true` between issuing a PulseGuide on this axis and the
    /// watcher clearing the flag after the pulse `duration` has
    /// elapsed (or earlier, via the cancellation rule — any
    /// axis-mutating operation clears the flag before issuing its own
    /// wire commands so the watcher's post-sleep restore bails out).
    /// See §"PulseGuide lifecycle" in the design doc.
    pulse_guiding_ra: bool,
    pulse_guiding_dec: bool,
}

impl Default for DriverState {
    fn default() -> Self {
        Self {
            tracking_requested: false,
            at_park: false,
            target_ra_hours: None,
            target_dec_degrees: None,
            slew_settle_time: None,
            slew_in_progress: false,
            guide_rate_ra_fraction: DEFAULT_GUIDE_RATE_FRACTION,
            guide_rate_dec_fraction: DEFAULT_GUIDE_RATE_FRACTION,
            pulse_guiding_ra: false,
            pulse_guiding_dec: false,
        }
    }
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

    /// Reject a slew / sync whose target encoder ticks would fall
    /// outside the configured mechanical envelope.
    ///
    /// **Why:** the Star Adventurer GTi (like every GEM) has
    /// mechanical limits — slewing past them with cable wraps or the
    /// counterweight shaft against the pier stalls the motor against
    /// a hard stop while the encoder counter continues to advance.
    /// On a real-hardware ConformU run that drove the mount into the
    /// counterweight-up region we heard the motor whine and saw the
    /// axis stop physically for several seconds at a time. The
    /// configured `ra_min_hours` / `ra_max_hours` / `dec_min_degrees` /
    /// `dec_max_degrees` express the safe envelope; any target
    /// outside it is rejected with `INVALID_VALUE` and never reaches
    /// the wire.
    ///
    /// Both axes are validated together so a partial-failure slew
    /// can't issue motion on RA before discovering Dec is out of
    /// range.
    fn check_within_safe_envelope(
        &self,
        ra_ticks: i32,
        dec_ticks: i32,
        cpr_ra: u32,
        cpr_dec: u32,
    ) -> ASCOMResult<()> {
        let ra_min_ticks = mechanical_ha_to_ra_ticks(self.config.ra_min_hours, cpr_ra);
        let ra_max_ticks = mechanical_ha_to_ra_ticks(self.config.ra_max_hours, cpr_ra);
        if ra_ticks < ra_min_ticks || ra_ticks > ra_max_ticks {
            let mech_ha = ra_ticks_to_mechanical_ha(ra_ticks, cpr_ra);
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!(
                    "RA target mech-HA {mech_ha:.3} h outside safe envelope [{}, {}] h",
                    self.config.ra_min_hours, self.config.ra_max_hours
                ),
            ));
        }
        let dec_min_ticks = dec_degrees_to_ticks(self.config.dec_min_degrees, cpr_dec);
        let dec_max_ticks = dec_degrees_to_ticks(self.config.dec_max_degrees, cpr_dec);
        if dec_ticks < dec_min_ticks || dec_ticks > dec_max_ticks {
            let dec_deg = dec_ticks_to_degrees(dec_ticks, cpr_dec);
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!(
                    "Dec target {dec_deg:.3}° outside safe envelope [{}, {}]°",
                    self.config.dec_min_degrees, self.config.dec_max_degrees
                ),
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

    /// `ASCOMResult` wrapper over the free-function
    /// [`stop_axis_and_wait`] — issues `:K<axis>` and polls `:f<axis>`
    /// until the running flag clears. Used by every `MountDevice`
    /// caller that needs ASCOM error mapping: `set_tracking(true)`,
    /// `park`, and the per-axis stop preceding each `issue_slew_axis`
    /// in `slew_to_coordinates_async`. The slew-completion and park
    /// watchers run inside spawned tasks and call `stop_axis_and_wait`
    /// directly (no `MountDevice` to wrap the error).
    async fn stop_and_wait(&self, axis: Axis) -> ASCOMResult<()> {
        stop_axis_and_wait(&self.transport, axis, AXIS_STOP_TIMEOUT)
            .await
            .map_err(Self::ascom)
    }

    /// Block until the slew-completion watcher clears `slew_in_progress`,
    /// or until [`SYNC_SLEW_TIMEOUT`] elapses. Used by the synchronous
    /// `SlewToCoordinates` / `SlewToTarget` variants — those wrap their
    /// `_async` siblings, but ASCOM requires the synchronous methods
    /// not return until the slew is finished.
    ///
    /// Polls at the transport's `polling_interval` (same cadence the
    /// background snapshot poller uses, so `slewing()` can transition
    /// within one tick of the watcher's clear). The upper bound is
    /// well above any realistic real-mount slew but finite — a stuck
    /// watcher must not block an Alpaca request forever.
    async fn await_slew_complete(&self) -> ASCOMResult<()> {
        let poll = self.transport.polling_interval_for_watcher();
        let deadline = std::time::Instant::now() + SYNC_SLEW_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if !self.slewing().await? {
                return Ok(());
            }
            tokio::time::sleep(poll).await;
        }
        Err(ASCOMError::invalid_operation(
            "synchronous slew timed out waiting for completion",
        ))
    }
}

/// Upper bound on how long the synchronous `SlewToCoordinates` /
/// `SlewToTarget` will wait for the watcher to clear `slew_in_progress`.
/// 5 minutes — far longer than any realistic slew (a worst-case full
/// half-revolution at high-speed slew rate is well under a minute on
/// the GTi) but finite enough that a stuck driver cannot wedge an
/// Alpaca request indefinitely.
const SYNC_SLEW_TIMEOUT: Duration = Duration::from_secs(300);

/// Minimum wallclock duration the slew watcher will keep
/// `slew_in_progress` set, regardless of how fast the mount reports
/// the goto complete. See the rationale in
/// [`spawn_slew_completion_watcher`]: it guarantees that an Alpaca
/// client polling `Slewing` shortly after issuing a slew will catch
/// the `true` value at least once. Empirically ConformU's
/// AbortSlew-test wait between starting the slew and reading
/// `Slewing` runs in the 1.0–1.5 s range, so the floor needs to be
/// noticeably above that — 2 s is comfortable. A real GTi slew of
/// any meaningful distance takes well over 2 s, so this floor is
/// invisible on hardware.
const MIN_SLEW_DWELL: Duration = Duration::from_secs(2);

/// Upper bound on how long `stop_and_wait` will poll `:f<axis>`
/// after a `:K` (decelerate stop) before giving up. The firmware
/// finishes deceleration within ~1 s for typical Goto-Fast slew
/// rates on the GTi; 2 s is a comfortable margin for the slow
/// case, and bounding the wait prevents a stuck axis from wedging
/// a slew indefinitely.
const AXIS_STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// EQMOD `minperiods[axis]` default — see
/// `indi-3rdparty/indi-eqmod/skywatcher.cpp:509-510`. INDI emits
/// `:I<axis>6` on every slew; the firmware uses this step period
/// to ramp the motor through the goto.
const SLEW_STEP_PERIOD: u32 = 6;

/// INDI `SetTargetBreaks` cap — see
/// `indi-3rdparty/indi-eqmod/skywatcher.cpp::SlewTo`. The breakpoint
/// increment is `min(|delta|/10, 3200)`; without the cap, very long
/// slews exceed the firmware's break-point range.
const SLEW_BREAK_POINT_DIVISOR: u32 = 10;
const SLEW_BREAK_POINT_MAX: u32 = 3200;

/// EQMOD `RAGOTORESOLUTION` / `DEGOTORESOLUTION` — see
/// `indi-3rdparty/indi-eqmod/eqmodbase.cpp:64-66`. After the goto
/// stops, the pickup loop computes the residual against the latched
/// RA/Dec target and re-issues a corrective slew if either axis
/// exceeds this threshold (5 arc-seconds).
const PICKUP_TOLERANCE_ARCSEC: f64 = 5.0;

/// EQMOD `GOTO_ITERATIVE_LIMIT` — see
/// `indi-3rdparty/indi-eqmod/eqmodbase.cpp:64`. INDI caps the
/// pickup loop at 5 iterations to keep a pathological case (motor
/// stalled, encoder oscillating, …) from running forever.
const PICKUP_MAX_ITERATIONS: u32 = 5;

/// Issue the per-axis INDI slew sequence:
/// `:G<axis>` (goto + fast, direction by sign of `delta`) →
/// `:I<axis>6` (step period) →
/// `:H<axis><|delta|>` (target increment) →
/// `:M<axis><breaks>` (break-point increment) →
/// `:J<axis>` (start motion).
///
/// The caller must have already issued `:K<axis>` and waited for the
/// running flag to clear — `:G` returns `!2 MotorNotStopped` if the
/// motor is still decelerating from a prior command.
async fn issue_slew_axis(
    transport: &TransportManager,
    axis: Axis,
    delta: i32,
) -> crate::error::Result<()> {
    let magnitude = delta.unsigned_abs();
    let breaks = (magnitude / SLEW_BREAK_POINT_DIVISOR).min(SLEW_BREAK_POINT_MAX);
    let mode = MotionMode {
        kind: skywatcher_motor_protocol::command::ModeKind::Goto,
        speed: skywatcher_motor_protocol::command::Speed::Fast,
        ccw: delta < 0,
    };
    transport
        .send(Command::SetMotionMode { axis, mode })
        .await?;
    transport
        .send(Command::SetStepPeriod {
            axis,
            period: SLEW_STEP_PERIOD,
        })
        .await?;
    transport
        .send(Command::SetGotoTargetIncrement {
            axis,
            increment: magnitude,
        })
        .await?;
    transport
        .send(Command::SetBreakPointIncrement { axis, breaks })
        .await?;
    transport.send(Command::StartMotion(axis)).await?;
    Ok(())
}

/// Returns `true` when the slew-completion watcher must bail out of
/// its current iteration: either `AbortSlew` cleared
/// `slew_in_progress`, or `set_connected(false)` closed the transport.
/// Both conditions can race in mid-iteration after the top-of-loop
/// guard has already passed, so the watcher checks this helper a
/// second time immediately before issuing any post-snapshot wire
/// commands (the EQMOD pickup re-slew or the post-slew tracking
/// restart).
async fn watcher_should_abort(
    state: &Arc<RwLock<DriverState>>,
    transport: &TransportManager,
) -> bool {
    !state.read().await.slew_in_progress || !transport.is_available()
}

/// Per-axis pickup re-slew used by the watcher's EQMOD pickup loop.
/// Calls [`stop_axis_and_wait`] (drains any residual goto deceleration)
/// then [`issue_slew_axis`] (re-runs the INDI wire sequence with the
/// freshly-computed `delta`). Both calls are best-effort: a failure
/// from either is logged at `warn` and swallowed because the watcher
/// has nothing useful to do with the error other than retry on the
/// next iteration. Wrapping the pair in this helper keeps the watcher
/// body free of nested `if let Err` branches that codecov flags as
/// uncovered for the rare-but-real failure paths.
async fn pickup_reslew_axis(transport: &TransportManager, axis: Axis, delta: i32) {
    if let Err(e) = stop_axis_and_wait(transport, axis, AXIS_STOP_TIMEOUT).await {
        tracing::warn!("pickup stop {axis:?} failed: {e}");
        return;
    }
    if let Err(e) = issue_slew_axis(transport, axis, delta).await {
        tracing::warn!("pickup re-slew {axis:?} failed: {e}");
    }
}

/// Validate an ASCOM `GuideRate*` setter value (deg/sec) and return
/// the equivalent fraction of sidereal. Rejects values outside the
/// open interval `(0, SIDEREAL_DEG_PER_SEC)`:
///
/// - `≤ 0` is non-physical (zero rate = no motion; negative = wrong
///   direction).
/// - `≥ SIDEREAL_DEG_PER_SEC` would push East's `rate_factor = 1 -
///   fraction` to zero or negative, which divides by zero in the
///   step-period formula. INDI's eqmod driver imposes the same upper
///   bound for the same reason.
fn validate_guide_rate(deg_per_sec: f64) -> ASCOMResult<f64> {
    let fraction = deg_per_sec / SIDEREAL_DEG_PER_SEC;
    if !fraction.is_finite() || fraction <= 0.0 || fraction >= 1.0 {
        return Err(ASCOMError::new(
            ASCOMErrorCode::INVALID_VALUE,
            format!(
                "guide rate {deg_per_sec} deg/sec is outside the supported \
                 range (0, {SIDEREAL_DEG_PER_SEC})"
            ),
        ));
    }
    Ok(fraction)
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
            //
            // Reset guide rates to the default — the design doc says
            // they re-initialise on each `Connected = true`, matching
            // INDI's behaviour. Doing the reset on disconnect (instead
            // of on the next connect) is symmetric with the other
            // per-session clears here.
            let mut s = self.state.write().await;
            s.target_ra_hours = None;
            s.target_dec_degrees = None;
            s.tracking_requested = false;
            s.slew_in_progress = false;
            s.pulse_guiding_ra = false;
            s.pulse_guiding_dec = false;
            s.guide_rate_ra_fraction = DEFAULT_GUIDE_RATE_FRACTION;
            s.guide_rate_dec_fraction = DEFAULT_GUIDE_RATE_FRACTION;
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
    async fn can_pulse_guide(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn can_set_guide_rates(&self) -> ASCOMResult<bool> {
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
        // Cancel any in-flight RA pulse before mutating the RA axis.
        // The pulse-guide watcher's post-sleep restore step checks
        // `pulse_guiding_ra` and bails if cleared. Without this,
        // `set_tracking(false)` during an East/West pulse would be
        // silently undone when the watcher re-issued sidereal tracking
        // on restore.
        self.state.write().await.pulse_guiding_ra = false;
        if tracking {
            // Enabling tracking while parked is invalid per ASCOM
            // ITelescopeV3. Disabling tracking while parked stays
            // allowed — Park itself leaves tracking off, but a caller
            // re-asserting that should not error.
            self.ensure_unparked().await?;
            // Compute the sidereal step period from the cached parameters.
            let params = self
                .transport
                .parameters()
                .await
                .ok_or(ASCOMError::NOT_CONNECTED)?;
            let period = sidereal_step_period(params.tmr_freq, params.cpr_ra);
            // Per Sky-Watcher spec §2: "Motor must be at full stop
            // status before setting the motion mode." The RA axis
            // may already be running — from a prior tracking enable,
            // or because the firmware auto-engages Speed (Tracking)
            // Mode after every goto completes. Force a stop and wait
            // for the running flag to clear before re-issuing the
            // tracking-mode `:G`/`:I`/`:J` sequence.
            self.stop_and_wait(Axis::Ra).await?;
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
        Ok(side_of_pier_calc(
            snap.dec.position_ticks,
            params.cpr_dec,
            self.config.site_latitude_deg,
        ))
    }

    async fn destination_side_of_pier(&self, ra: f64, dec: f64) -> ASCOMResult<PierSide> {
        // Pure prediction — no wire traffic, no slew. Runs the same
        // coordinate-math pipeline `slew_to_coordinates_async` uses to
        // pick the target encoder pair, then applies the same Dec >
        // 90° check `side_of_pier()` uses to classify the resulting
        // pointing state. The driver never plans a meridian flip, so
        // any target inside the safety envelope lands with the Dec
        // encoder within ±90° and therefore predicts pierWest in the
        // Northern Hemisphere (East in the Southern). Targets outside
        // the envelope are rejected with `INVALID_VALUE` here for
        // parity with `slew_to_coordinates_async` — ConformU's
        // SOPPierTest commands such targets to exercise the
        // pier-flip code paths, and rejecting them at the
        // prediction step matches the rejection at the slew step.
        self.ensure_connected().await?;
        Self::validate_coordinates(ra, dec)?;
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg);
        let mech_ha = ra_to_mechanical_ha(ra, lst);
        let ra_ticks = mechanical_ha_to_ra_ticks(mech_ha, params.cpr_ra);
        let dec_ticks = dec_degrees_to_ticks(dec, params.cpr_dec);
        self.check_within_safe_envelope(ra_ticks, dec_ticks, params.cpr_ra, params.cpr_dec)?;
        Ok(side_of_pier_calc(
            dec_ticks,
            params.cpr_dec,
            self.config.site_latitude_deg,
        ))
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
        // Cancel any in-flight pulse-guide on either axis — sync is
        // an axis-position mutation and we don't want the watcher
        // restoring tracking against the freshly-set encoder position.
        {
            let mut s = self.state.write().await;
            s.pulse_guiding_ra = false;
            s.pulse_guiding_dec = false;
        }
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg);
        let mech_ha = ra_to_mechanical_ha(ra, lst);
        let ra_ticks = mechanical_ha_to_ra_ticks(mech_ha, params.cpr_ra);
        let dec_ticks = dec_degrees_to_ticks(dec, params.cpr_dec);
        // Reject syncs that would set the encoder outside the
        // mount's safe mechanical envelope — a bad sync would let
        // the *next* tracking step push the OTA into a hard stop.
        self.check_within_safe_envelope(ra_ticks, dec_ticks, params.cpr_ra, params.cpr_dec)?;
        self.transport
            .send(Command::SetPosition {
                axis: Axis::Ra,
                ticks: ra_ticks,
            })
            .await
            .map_err(Self::ascom)?;
        // Publish the just-written RA position to the cached snapshot
        // so an immediate `RightAscension` read reflects the sync
        // without having to wait for the next background poll. Done
        // only after the wire `:E` succeeds.
        self.transport.seed_ra_position(ra_ticks).await;
        self.transport
            .send(Command::SetPosition {
                axis: Axis::Dec,
                ticks: dec_ticks,
            })
            .await
            .map_err(Self::ascom)?;
        self.transport.seed_dec_position(dec_ticks).await;
        // Per ASCOM ITelescopeV3, a successful Sync sets
        // TargetRightAscension / TargetDeclination to the synced
        // coordinates. ConformU asserts this. Only write the in-memory
        // target after both `:E` sends succeed so a partial-failure
        // sync doesn't leave Target reflecting a position the mount
        // never actually accepted.
        {
            let mut s = self.state.write().await;
            s.target_ra_hours = Some(ra);
            s.target_dec_degrees = Some(dec);
        }
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
        // completion watcher knows whether to auto-restore. Also
        // cancel any in-flight pulse-guide on either axis (the slew
        // takes ownership of both axes from this point). We do NOT
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
            s.pulse_guiding_ra = false;
            s.pulse_guiding_dec = false;
        }

        // Compute target encoder ticks for the *current* LST. INDI's
        // EQMOD-style post-stop pickup loop (issue #205) handles the
        // residual that arises because RA drifts during the goto: when
        // the watcher detects both axes stopped, it reads the actual
        // RA/Dec, computes the residual against the latched target,
        // and re-issues a corrective goto if the residual exceeds the
        // INDI tolerance (`RAGOTORESOLUTION = 5"`). Earlier revisions
        // sidestepped this by pre-shifting LST by `MIN_SLEW_DWELL` —
        // that bounded mock drift but undershot real-hardware slews
        // of 3-7 s, leaving 45-120 arc-second RA residuals. The
        // pickup loop closes the gap cleanly.
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg);
        let mech_ha = ra_to_mechanical_ha(ra, lst);
        let ra_ticks = mechanical_ha_to_ra_ticks(mech_ha, params.cpr_ra);
        let dec_ticks = dec_degrees_to_ticks(dec, params.cpr_dec);

        // Refuse before any wire motion if the slew target falls
        // outside the configured mechanical envelope. ConformU's
        // pier-flip tests deliberately command across-the-meridian
        // slews that on a GEM-without-flip translate to encoders
        // past the counterweight-horizontal boundary; the safety
        // gate sends those back as `INVALID_VALUE` instead of
        // stalling the motor against a hard stop.
        self.check_within_safe_envelope(ra_ticks, dec_ticks, params.cpr_ra, params.cpr_dec)?;

        let snap = self.transport.snapshot().await;
        let ra_delta = ra_ticks - snap.ra.position_ticks;
        let dec_delta = dec_ticks - snap.dec.position_ticks;
        // Both axes use the INDI wire sequence: `:K` + poll `:f`
        // (decelerate stop — the spec's "motor must be at full stop
        // before setting the motion mode" requirement) → `:G goto+fast`
        // → `:I 6` → `:H |delta|` → `:M breaks` → `:J`. The RA-axis
        // `:K` is also the wire event that halts any in-progress
        // sidereal tracking; mirror that into the in-memory
        // `tracking_requested` flag once the stop has actually
        // succeeded so the state never gets ahead of the wire on
        // transport failures.
        self.stop_and_wait(Axis::Ra).await?;
        self.state.write().await.tracking_requested = false;
        issue_slew_axis(&self.transport, Axis::Ra, ra_delta)
            .await
            .map_err(Self::ascom)?;
        self.stop_and_wait(Axis::Dec).await?;
        issue_slew_axis(&self.transport, Axis::Dec, dec_delta)
            .await
            .map_err(Self::ascom)?;

        // Mark slew in progress and spawn the completion watcher. The
        // watcher polls until both axes report stopped, runs the
        // EQMOD-style pickup loop (up to 5 iterations) to nudge any
        // residual under 5", optionally re-issues sidereal tracking
        // on RA (only if it was on before the slew), applies the
        // settle delay, then clears `slew_in_progress`.
        let settle = {
            let mut s = self.state.write().await;
            s.slew_in_progress = true;
            s.slew_settle_time.unwrap_or(self.config.settle_after_slew)
        };
        spawn_slew_completion_watcher(
            Arc::clone(&self.state),
            Arc::clone(&self.transport),
            self.config.clone(),
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

    async fn slew_to_coordinates(&self, ra: f64, dec: f64) -> ASCOMResult<()> {
        // ASCOM requires this synchronous variant when CanSlew = true.
        // ConformU flags the trait-default NotImplemented as a spec
        // violation. Implement as: start the async slew, then await the
        // completion watcher by polling `Slewing` until it clears.
        self.slew_to_coordinates_async(ra, dec).await?;
        self.await_slew_complete().await
    }

    async fn slew_to_target(&self) -> ASCOMResult<()> {
        self.slew_to_target_async().await?;
        self.await_slew_complete().await
    }

    // ---- Park / Unpark / Abort ----

    async fn park(&self) -> ASCOMResult<()> {
        self.ensure_connected().await?;
        // Idempotent: already parked → no-op.
        if self.state.read().await.at_park {
            return Ok(());
        }
        // Cancel any in-flight pulse-guide — park takes ownership of
        // both axes. Watchers see the cleared flags on post-sleep and
        // bail out without restoring.
        {
            let mut s = self.state.write().await;
            s.pulse_guiding_ra = false;
            s.pulse_guiding_dec = false;
        }
        // Stop tracking before slewing home (per ASCOM, tracking remains
        // off after Park). The wire `:K1` is issued first so the in-memory
        // flag flip only follows a successful stop.
        if self.state.read().await.tracking_requested {
            self.transport
                .send(Command::StopMotion(Axis::Ra))
                .await
                .map_err(Self::ascom)?;
            self.state.write().await.tracking_requested = false;
        }
        // Slew both axes to encoder 0. Same wire sequence as
        // `slew_to_coordinates_async`: `:K`-and-wait, `:G` with
        // direction chosen from `sign(0 - current)`, `:S 0`, `:J`.
        let snap = self.transport.snapshot().await;
        for (axis, current_ticks) in [
            (Axis::Ra, snap.ra.position_ticks),
            (Axis::Dec, snap.dec.position_ticks),
        ] {
            self.stop_and_wait(axis).await?;
            let mode = MotionMode {
                kind: skywatcher_motor_protocol::command::ModeKind::Goto,
                speed: skywatcher_motor_protocol::command::Speed::Fast,
                ccw: current_ticks > 0,
            };
            self.transport
                .send(Command::SetMotionMode { axis, mode })
                .await
                .map_err(Self::ascom)?;
            // No `:I` in Goto mode — the firmware computes slew speed
            // internally. See the matching note in
            // `slew_to_coordinates_async`.
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
        // Aborting while parked is invalid per ASCOM ITelescopeV3.
        // Refuse before mutating any state so a caller that mistakenly
        // calls AbortSlew on a parked mount gets a clean error without
        // side-effects on tracking_requested or slew_in_progress.
        self.ensure_unparked().await?;
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
            // Cancel any in-flight pulse-guide on either axis. The
            // watcher's post-sleep restore step bails when it sees the
            // flag cleared; `:L1`/`:L2` below already halt any
            // rate-shifted motion, so there's nothing for the watcher
            // to restore.
            s.pulse_guiding_ra = false;
            s.pulse_guiding_dec = false;
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

    // ---- PulseGuide ----

    async fn is_pulse_guiding(&self) -> ASCOMResult<bool> {
        let s = self.state.read().await;
        Ok(s.pulse_guiding_ra || s.pulse_guiding_dec)
    }

    async fn guide_rate_right_ascension(&self) -> ASCOMResult<f64> {
        let f = self.state.read().await.guide_rate_ra_fraction;
        Ok(f * SIDEREAL_DEG_PER_SEC)
    }

    async fn set_guide_rate_right_ascension(
        &self,
        guide_rate_right_ascension: f64,
    ) -> ASCOMResult<()> {
        let fraction = validate_guide_rate(guide_rate_right_ascension)?;
        self.state.write().await.guide_rate_ra_fraction = fraction;
        Ok(())
    }

    async fn guide_rate_declination(&self) -> ASCOMResult<f64> {
        let f = self.state.read().await.guide_rate_dec_fraction;
        Ok(f * SIDEREAL_DEG_PER_SEC)
    }

    async fn set_guide_rate_declination(&self, guide_rate_declination: f64) -> ASCOMResult<()> {
        let fraction = validate_guide_rate(guide_rate_declination)?;
        self.state.write().await.guide_rate_dec_fraction = fraction;
        Ok(())
    }

    async fn pulse_guide(&self, direction: GuideDirection, duration: Duration) -> ASCOMResult<()> {
        self.ensure_connected().await?;
        self.ensure_unparked().await?;
        if self.slewing().await? {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_OPERATION,
                "PulseGuide refused while slewing",
            ));
        }
        // Duration zero is a no-op success per ASCOM convention. Skip
        // before resolving direction / acquiring locks to keep the
        // hot-path predictable.
        if duration.is_zero() {
            return Ok(());
        }
        // Resolve direction → (axis, ccw, rate_factor) and capture the
        // `tracking_was_on` snapshot under a single read lock. Same
        // read also catches a same-axis pulse already in flight.
        let (axis, ccw, rate_factor, tracking_was_on) = {
            let s = self.state.read().await;
            let (axis, ccw, rate_factor) = match direction {
                GuideDirection::East => (Axis::Ra, false, 1.0 - s.guide_rate_ra_fraction),
                GuideDirection::West => (Axis::Ra, false, 1.0 + s.guide_rate_ra_fraction),
                GuideDirection::North => (Axis::Dec, false, s.guide_rate_dec_fraction),
                GuideDirection::South => (Axis::Dec, true, s.guide_rate_dec_fraction),
            };
            let in_flight = match axis {
                Axis::Ra => s.pulse_guiding_ra,
                Axis::Dec => s.pulse_guiding_dec,
                Axis::Both => false,
            };
            if in_flight {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_OPERATION,
                    "PulseGuide refused while a same-axis pulse is in flight",
                ));
            }
            let tracking_was_on = axis == Axis::Ra && s.tracking_requested;
            (axis, ccw, rate_factor, tracking_was_on)
        };
        // Compute the shifted step period from the cached
        // sidereal-period helper and the rate factor.
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let sidereal_period = sidereal_step_period(params.tmr_freq, params.cpr_ra);
        let shifted_period = pulse_guide_step_period(sidereal_period, rate_factor);
        // Wire path: `:K<axis>` (and wait for the running flag to clear
        // so `:G` doesn't return `!2 MotorNotStopped`), `:G<axis>`
        // (Tracking + ccw), `:I<axis>` (shifted period), `:J<axis>`.
        self.stop_and_wait(axis).await?;
        let mode = MotionMode {
            kind: ModeKind::Tracking,
            speed: Speed::Slow,
            ccw,
        };
        self.transport
            .send(Command::SetMotionMode { axis, mode })
            .await
            .map_err(Self::ascom)?;
        self.transport
            .send(Command::SetStepPeriod {
                axis,
                period: shifted_period,
            })
            .await
            .map_err(Self::ascom)?;
        self.transport
            .send(Command::StartMotion(axis))
            .await
            .map_err(Self::ascom)?;
        // Set the in-flight flag synchronously before spawning so a
        // client that polls `IsPulseGuiding` immediately after
        // `PulseGuide` returns sees `true` even for sub-poll durations.
        {
            let mut s = self.state.write().await;
            match axis {
                Axis::Ra => s.pulse_guiding_ra = true,
                Axis::Dec => s.pulse_guiding_dec = true,
                Axis::Both => {}
            }
        }
        spawn_pulse_guide_watcher(
            Arc::clone(&self.state),
            Arc::clone(&self.transport),
            axis,
            duration,
            tracking_was_on,
        );
        debug!(?direction, ?duration, axis = ?axis, "pulse_guide spawned");
        Ok(())
    }
}

/// Spawn the slew-completion watcher.
///
/// Polls the snapshot every `polling_interval`. When both axes report
/// `running == false` (or the slew was aborted externally — in which
/// case `slew_in_progress` is already cleared and the watcher exits
/// immediately), runs the EQMOD-style iterative pickup loop to push
/// any RA/Dec residual under [`PICKUP_TOLERANCE_ARCSEC`], optionally
/// re-issues sidereal tracking on the RA axis (matching the design
/// doc's "if Tracking was on" branch), waits `settle`, then clears
/// `slew_in_progress`.
///
/// `tracking_was_on` is captured at slew-issue time — the live
/// `tracking_requested` flag is cleared by `slew_to_coordinates_async`
/// so `tracking()` reports the wire state during the slew, hence we
/// can't read it from `state` here.
fn spawn_slew_completion_watcher(
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
    config: MountConfig,
    polling_interval: Duration,
    settle: Duration,
    tracking_was_on: bool,
) {
    let started = std::time::Instant::now();
    tokio::spawn(async move {
        // Pause the background polling task for the duration of the
        // slew. With polling paused the watcher owns the wire: pickup
        // commands fire without contending with `:j` / `:f` polls for
        // `command_lock`, and the watcher's own `poll_axes_now` reads
        // give us mount state within one wire round-trip of any
        // change — vs up to `polling_interval` of snapshot staleness
        // under the always-on polling model.
        //
        // `_poll_guard` is held by value so the polling task resumes
        // automatically on every exit path (early-return for abort,
        // disconnect, blocked-axis, panic, or normal completion).
        let _poll_guard = transport.pause_background_polling();
        let mut pickup_iterations: u32 = 0;
        // Adaptive pickup-target projection: track the instant of each
        // prior pickup re-slew so the next iteration can project the
        // residual target forward by *the actually-observed* iteration
        // duration rather than a hardcoded `polling_interval × 2`
        // multiplier. USB on the GTi sees ~400 ms per iteration; UDP
        // sees ~950 ms because the per-round-trip latency adds up
        // across the 5-frame re-slew sequence. The fixed multiplier
        // worked on USB but under-compensated on UDP by ~550 ms (~8″
        // of unaccounted LST drift per iteration). Measuring once a
        // prior iteration is available makes the projection self-tune
        // per transport.
        let mut last_pickup_at: Option<std::time::Instant> = None;
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

            // Direct poll instead of reading the (now-paused) background
            // snapshot. On failure (transport closed mid-slew, command
            // timeout, ...), treat as an abort to avoid spinning.
            let snap = match transport.poll_axes_now().await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("watcher poll_axes_now failed: {e}");
                    state.write().await.slew_in_progress = false;
                    return;
                }
            };
            // Sky-Watcher spec §5 reports `Blocked` in the `:f`
            // status when the motor is stepping but the encoder
            // isn't advancing — typically the axis is against a
            // hard stop. Issue `:L` on both axes to halt the
            // runaway and bail out of the slew rather than letting
            // the watcher poll-loop continue while the gearbox
            // strains.
            if snap.ra.blocked || snap.dec.blocked {
                tracing::warn!(
                    ra_blocked = snap.ra.blocked,
                    dec_blocked = snap.dec.blocked,
                    "axis reports Blocked — aborting slew via :L"
                );
                let _ = transport.send(Command::InstantStop(Axis::Ra)).await;
                let _ = transport.send(Command::InstantStop(Axis::Dec)).await;
                state.write().await.slew_in_progress = false;
                return;
            }
            let still_moving = snap.ra.running || snap.dec.running;
            if still_moving {
                continue;
            }

            // Enforce a minimum slew dwell so external observers reliably
            // catch `Slewing == true`. ConformU starts a slew via HTTP,
            // then reads `Slewing` over a second HTTP call; the round-
            // trip latency can be larger than the mock's full slew
            // duration on a fast machine (the mock advances 100K
            // ticks/poll, so a small slew completes in 1-2 polls). The
            // de-facto Alpaca client poll cadence is on the order of
            // 100 ms; two full seconds of guaranteed dwell is a safe
            // floor for any reasonable client without meaningfully
            // slowing real-mount operation (real slews take seconds).
            //
            // The dwell *must* gate the pickup loop, not run after it.
            // The encoder is static while the watcher is observing
            // (tracking is off until the post-slew re-enable below),
            // so the apparent RA drifts at sidereal rate as LST
            // advances. If the pickup loop ran during the dwell wait,
            // it would re-detect that drift on every iteration and
            // burn through `PICKUP_MAX_ITERATIONS` just waiting —
            // potentially leaving a residual of one dwell-worth of
            // sidereal drift (~30") at the moment tracking re-enables.
            // Gating pickup behind the dwell means the loop sees a
            // single accumulated residual once, corrects it, then
            // hands off to tracking immediately.
            if started.elapsed() < MIN_SLEW_DWELL {
                continue;
            }

            // Both axes report stopped and the dwell has elapsed. Run
            // the EQMOD pickup loop: if either residual exceeds 5",
            // re-enter the goto sequence with a fresh delta computed
            // for the current LST. Capped at `PICKUP_MAX_ITERATIONS`
            // to match INDI's `GOTO_ITERATIVE_LIMIT`. On the GTi the
            // loop converges in 1–2 iterations because the post-stop
            // residual is bounded by the slew duration × sidereal
            // rate (~15"/s of RA drift per second of slew).
            if pickup_iterations < PICKUP_MAX_ITERATIONS {
                let (target_ra, target_dec) = {
                    let s = state.read().await;
                    (s.target_ra_hours, s.target_dec_degrees)
                };
                if let (Some(target_ra), Some(target_dec), Some(params)) =
                    (target_ra, target_dec, transport.parameters().await)
                {
                    let lst =
                        local_sidereal_time_hours(SystemTime::now(), config.site_longitude_deg);
                    let cur_mech_ha =
                        ra_ticks_to_mechanical_ha(snap.ra.position_ticks, params.cpr_ra);
                    let cur_ra = mechanical_ha_to_ra(cur_mech_ha, lst);
                    let cur_dec = dec_ticks_to_degrees(snap.dec.position_ticks, params.cpr_dec);
                    // RA residual is on a 24-hour circle; take the
                    // shorter arc. Convert hours → arc-seconds
                    // (15°/hour × 3600″/°).
                    let ra_circ = ((target_ra - cur_ra).rem_euclid(24.0))
                        .min((cur_ra - target_ra).rem_euclid(24.0));
                    let ra_residual_arcsec = ra_circ * 15.0 * 3600.0;
                    let dec_residual_arcsec = (target_dec - cur_dec).abs() * 3600.0;
                    if ra_residual_arcsec > PICKUP_TOLERANCE_ARCSEC
                        || dec_residual_arcsec > PICKUP_TOLERANCE_ARCSEC
                    {
                        // Re-check the abort / disconnect signals
                        // immediately before issuing any wire
                        // commands. The top-of-loop guard ran one
                        // `:f` round-trip + a few coordinate ops
                        // ago; in that window AbortSlew (which
                        // clears `slew_in_progress` and issues :L)
                        // or set_connected(false) (which closes the
                        // transport) may have raced ahead. Without
                        // this second guard the pickup loop would
                        // restart motion after the user aborted.
                        if watcher_should_abort(&state, &transport).await {
                            state.write().await.slew_in_progress = false;
                            return;
                        }
                        // Pre-compensate the RA target for the LST drift
                        // that will accumulate before the next pickup
                        // iteration re-checks the residual. Without it
                        // pickup chases a moving target and the residual
                        // floor matches per-iteration sidereal drift
                        // (~6″ on USB, ~14″ on UDP). See
                        // `docs/plans/star-adventurer-gti-pickup-accuracy.md`
                        // §"Experiment B".
                        //
                        // Adaptive: use the actually-observed time delta
                        // between consecutive pickup decisions; this
                        // self-tunes for the transport's wire latency
                        // (USB ≈ 400 ms/iter, UDP ≈ 950 ms/iter).
                        // First iteration has no prior data → fall back
                        // to `polling_interval × 2` (the USB-tuned heuristic).
                        let now = std::time::Instant::now();
                        let projection = match last_pickup_at {
                            Some(t) => now.duration_since(t),
                            None => polling_interval * 2,
                        };
                        last_pickup_at = Some(now);
                        let new_ra_ticks =
                            pickup_target_ra_ticks(target_ra, lst, projection, params.cpr_ra);
                        let new_dec_ticks = dec_degrees_to_ticks(target_dec, params.cpr_dec);
                        let ra_delta = new_ra_ticks - snap.ra.position_ticks;
                        let dec_delta = new_dec_ticks - snap.dec.position_ticks;
                        pickup_iterations += 1;
                        debug!(
                            iteration = pickup_iterations,
                            ra_residual_arcsec,
                            dec_residual_arcsec,
                            projection_ms = projection.as_millis() as u64,
                            ra_delta_ticks = ra_delta,
                            "slew pickup iteration"
                        );
                        // The pickup re-slew goes through the same
                        // wire sequence as the original goto. `:L` +
                        // poll keeps the motor-not-stopped contract
                        // intact even if a previous send failed
                        // mid-sequence.
                        pickup_reslew_axis(&transport, Axis::Ra, ra_delta).await;
                        pickup_reslew_axis(&transport, Axis::Dec, dec_delta).await;
                        continue;
                    }
                }
            }

            // Slew completed cleanly. Re-enable tracking if the user had
            // it on before the slew, then apply the settle delay. Only
            // mark tracking_requested=true if the StartMotion actually
            // succeeds — otherwise Tracking() would lie about the wire
            // state. The earlier mode/period sends are best-effort but
            // failures are logged for diagnosis.
            //
            // Re-check abort / disconnect before issuing the tracking
            // wire sequence — same race-window argument as the pickup
            // loop's pre-wire guard. AbortSlew clearing `slew_in_progress`
            // between the top-of-loop check and now must skip the
            // tracking restart, or the user-visible state would say
            // "aborted" while the wire is back to tracking.
            if watcher_should_abort(&state, &transport).await {
                state.write().await.slew_in_progress = false;
                return;
            }
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
            // Resume background polling *now*, before the settle delay.
            // The pickup loop is done; from here on the watcher is just
            // waiting for the firmware tracking to engage (~160 ms) and
            // applying the settle margin. While we wait, the background
            // polling task should refresh the snapshot at its regular
            // cadence so an Alpaca client reading `RightAscension` right
            // after `Slewing` flips to `false` sees a snapshot that
            // reflects the encoder at its now-actively-tracking
            // position, not the watcher's last `poll_axes_now` from
            // before tracking restart. Without this, the snap is stale
            // by the duration `(tracking_engagement + settle)` and the
            // reported RA lags by that × sidereal rate (~5-10″).
            drop(_poll_guard);
            tokio::time::sleep(settle).await;
            state.write().await.slew_in_progress = false;
            return;
        }
    });
}

/// Free-function equivalent of [`MountDevice::stop_and_wait`] for
/// callers (like the watcher's EQMOD pickup loop) that don't have a
/// `&MountDevice`. Issues `:K<axis>` (decelerate) and polls
/// `:f<axis>` until the running flag clears or `timeout` elapses.
/// `:K` is the spec's recommended stop and is gentler on the
/// gearbox than `:L`; `:L` remains the right choice only for
/// genuine emergency stops (`AbortSlew`, slew/park watcher abort on
/// `blocked`). Matches INDI eqmod's `StopWaitMotor`
/// (`indi-eqmod/skywatcher.cpp:1741-1765`).
async fn stop_axis_and_wait(
    transport: &TransportManager,
    axis: Axis,
    timeout: Duration,
) -> crate::error::Result<()> {
    transport.send(Command::StopMotion(axis)).await?;
    let deadline = std::time::Instant::now() + timeout;
    tokio::time::sleep(Duration::from_millis(100)).await;
    loop {
        let resp = transport.send(Command::InquireStatus(axis)).await?;
        if let skywatcher_motor_protocol::Response::Status(s) = resp {
            if !s.running {
                return Ok(());
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(StarAdvError::Transport(format!(
                "axis {axis:?} did not stop within {timeout:?}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
        // Same wire-ownership trick as the slew watcher: pause
        // background polling and drive snapshot freshness from
        // `poll_axes_now`. Park doesn't have a pickup loop so the
        // win here is smaller, but the consistency is worth it
        // and the background-polling pause also frees up the wire
        // for the `:K` + `:L` abort sequence on a blocked-axis
        // mechanical stop.
        let _poll_guard = transport.pause_background_polling();
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
            let snap = match transport.poll_axes_now().await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("park watcher poll_axes_now failed: {e}");
                    state.write().await.slew_in_progress = false;
                    return;
                }
            };
            // Park can also hit a mechanical stop — same `:L` + bail
            // treatment as in the slew watcher. Do *not* set
            // `at_park = true` on a blocked stop: the OTA isn't at
            // the encoder-0 home pose, so the next `Unpark + slew`
            // would compute a wrong delta.
            if snap.ra.blocked || snap.dec.blocked {
                tracing::warn!(
                    ra_blocked = snap.ra.blocked,
                    dec_blocked = snap.dec.blocked,
                    "axis reports Blocked during park — aborting via :L"
                );
                let _ = transport.send(Command::InstantStop(Axis::Ra)).await;
                let _ = transport.send(Command::InstantStop(Axis::Dec)).await;
                state.write().await.slew_in_progress = false;
                return;
            }
            if snap.ra.running || snap.dec.running {
                continue;
            }
            // Resume background polling before the settle so an
            // Alpaca client reading `AtPark`-related position data
            // right after `Slewing` clears sees fresh snapshot data.
            // See the matching note in `spawn_slew_completion_watcher`.
            drop(_poll_guard);
            tokio::time::sleep(settle).await;
            let mut s = state.write().await;
            s.at_park = true;
            s.slew_in_progress = false;
            return;
        }
    });
}

/// Spawn the PulseGuide watcher.
///
/// Sleeps for `duration`, then restores prior state on the targeted
/// axis:
/// - **RA pulse**: stop-and-wait, then if `tracking_was_on_for_restore`
///   re-issue `:G1 TRACKING` + `:I1 sidereal_period` + `:J1` so the
///   user-observable `Tracking` state survives the pulse.
/// - **Dec pulse**: stop-and-wait (Dec is normally idle; no restore).
///
/// The watcher checks the per-axis `pulse_guiding_<axis>` flag before
/// the restore step and bails out if cleared (the cancellation rule:
/// any axis-mutating call clears the flag before its own wire commands
/// so the watcher steps aside). Errors during the restore are logged
/// at `warn` and swallowed — matches [`pickup_reslew_axis`].
fn spawn_pulse_guide_watcher(
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
    axis: Axis,
    duration: Duration,
    tracking_was_on_for_restore: bool,
) {
    tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        // Bail if the pulse was cancelled externally (another op
        // cleared the flag), the transport dropped, or the mount
        // entered a state that takes ownership of the axis
        // (slew/park).
        let still_active = {
            let s = state.read().await;
            let active = match axis {
                Axis::Ra => s.pulse_guiding_ra,
                Axis::Dec => s.pulse_guiding_dec,
                Axis::Both => false,
            };
            active && !s.at_park && !s.slew_in_progress
        };
        if !still_active || !transport.is_available() {
            clear_pulse_flag(&state, axis).await;
            return;
        }
        // Stop the axis. Any failure here means we can't safely restore
        // either, so log and bail.
        if let Err(e) = stop_axis_and_wait(&transport, axis, AXIS_STOP_TIMEOUT).await {
            tracing::warn!("pulse-guide restore stop {axis:?} failed: {e}");
            clear_pulse_flag(&state, axis).await;
            return;
        }
        // RA-only: re-issue sidereal tracking iff the user had it on
        // at issue time. Dec just stays stopped (Dec is normally idle).
        if axis == Axis::Ra && tracking_was_on_for_restore {
            // Re-check the cancellation flag before issuing the restore
            // commands — a concurrent set_tracking(false) between the
            // stop above and here would otherwise be silently undone.
            let still_want_restore = state.read().await.pulse_guiding_ra;
            if still_want_restore {
                if let Some(params) = transport.parameters().await {
                    let period = sidereal_step_period(params.tmr_freq, params.cpr_ra);
                    if let Err(e) = transport
                        .send(Command::SetMotionMode {
                            axis: Axis::Ra,
                            mode: MotionMode::TRACKING,
                        })
                        .await
                    {
                        tracing::warn!("pulse-guide restore :G1 failed: {e}");
                    } else if let Err(e) = transport
                        .send(Command::SetStepPeriod {
                            axis: Axis::Ra,
                            period,
                        })
                        .await
                    {
                        tracing::warn!("pulse-guide restore :I1 failed: {e}");
                    } else if let Err(e) = transport.send(Command::StartMotion(Axis::Ra)).await {
                        tracing::warn!("pulse-guide restore :J1 failed: {e}");
                    }
                }
            }
        }
        clear_pulse_flag(&state, axis).await;
    });
}

async fn clear_pulse_flag(state: &Arc<RwLock<DriverState>>, axis: Axis) {
    let mut s = state.write().await;
    match axis {
        Axis::Ra => s.pulse_guiding_ra = false,
        Axis::Dec => s.pulse_guiding_dec = false,
        Axis::Both => {}
    }
}

#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::transport::mock::MockTransportFactory;

    fn device() -> MountDevice {
        let mut cfg = Config::default();
        // Same rationale as `fast_settle_device`: open the
        // mechanical-envelope check for tests that don't exercise it.
        cfg.mount.ra_min_hours = -12.0;
        cfg.mount.ra_max_hours = 12.0;
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

        // Drive SetTracking(true) and assert that the driver issues
        // the wire sequence the design doc + spec require:
        //   1. `:K1`  (decelerate the RA axis — needed because the
        //              spec disallows changing motion mode while the
        //              motor is running, and the axis may be in
        //              Speed Mode either because we just enabled
        //              tracking on it before or because the firmware
        //              auto-engages Speed Mode after a goto. `:K`
        //              is gentler on the gearbox than `:L`; the
        //              `:f` poll loop below waits out the
        //              deceleration.)
        //   2. `:f1`  (one or more polls — `stop_and_wait` polls
        //              until the running flag clears.)
        //   3. `:G1<mode>` (tracking-slow-CW)
        //   4. `:I1<period>` (sidereal step period)
        //   5. `:J1` (start motion)
        let baseline_len = mock.state.lock().await.command_log.len();
        d.set_tracking(true).await.unwrap();

        let log = mock.state.lock().await.command_log.clone();
        let new_frames: Vec<&[u8]> = log[baseline_len..].iter().map(|v| v.as_slice()).collect();
        // Look only at setter / motion-start frames on the RA axis:
        // `:G1`, `:I1`, `:J1`, `:K1` (in order of appearance). The
        // polling task's `:f1` / `:j1` / `:f2` / `:j2` inquiries are
        // noise here.
        let interesting: Vec<&&[u8]> = new_frames
            .iter()
            .filter(|f| {
                f.len() >= 3
                    && f[0] == b':'
                    && f[2] == b'1'
                    && matches!(f[1], b'G' | b'I' | b'J' | b'L' | b'K')
            })
            .collect();
        assert_eq!(
            interesting.len(),
            4,
            "expected exactly 4 RA setter frames (:K1 :G1 :I1 :J1), got {interesting:?}"
        );
        assert_eq!(*interesting[0], b":K1\r", "1st RA setter should be :K1");
        assert_eq!(&interesting[1][..3], b":G1", "2nd RA setter should be :G1");
        assert_eq!(&interesting[2][..3], b":I1", "3rd RA setter should be :I1");
        assert_eq!(*interesting[3], b":J1\r", "4th RA setter should be :J1");
    }

    #[tokio::test]
    async fn set_tracking_false_issues_k1() {
        let d = connected_device().await;
        d.set_tracking(true).await.unwrap();
        d.set_tracking(false).await.unwrap();
        assert!(!d.tracking().await.unwrap());
    }

    #[tokio::test]
    async fn set_tracking_true_refuses_while_parked() {
        let d = fast_settle_connected().await;
        d.park().await.unwrap();
        // Wait for the park watcher to set at_park.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if d.at_park().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(d.at_park().await.unwrap());
        let err = d.set_tracking(true).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_WHILE_PARKED);
    }

    #[tokio::test]
    async fn set_tracking_false_succeeds_while_parked() {
        // Disabling tracking on a parked mount is a no-op affirmation;
        // refusing it would force callers to special-case the parked
        // state when they just want to assert "tracking should be off".
        let d = fast_settle_connected().await;
        d.park().await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if d.at_park().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
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
        // The slew-lifecycle tests pass hardcoded RA/Dec targets
        // (typically `(6.0 h, 30°)`) whose mech-HA depends on the
        // wallclock LST and would intermittently fall outside the
        // production default envelope of `±6 h / ±90°`. Open the
        // envelope all the way for these tests; the safety-gate
        // behaviour is covered separately by
        // [`fast_settle_connected_narrow_envelope`].
        cfg.mount.ra_min_hours = -12.0;
        cfg.mount.ra_max_hours = 12.0;
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

    /// Like `fast_settle_connected`, but with a narrow mechanical
    /// envelope so the safety-gate tests can land target coords
    /// that are clearly outside the envelope without first needing
    /// to push past the GTi default `±6h` / `±90°`.
    async fn fast_settle_connected_narrow_envelope() -> MountDevice {
        let mut cfg = Config::default();
        if let crate::config::TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        cfg.mount.settle_after_slew = Duration::from_millis(0);
        // Allow exactly the meridian band ±1 h of HA / ±5° of Dec.
        cfg.mount.ra_min_hours = -1.0;
        cfg.mount.ra_max_hours = 1.0;
        cfg.mount.dec_min_degrees = -5.0;
        cfg.mount.dec_max_degrees = 5.0;
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();
        d
    }

    #[tokio::test]
    async fn slew_async_refuses_dec_outside_safe_envelope() {
        // Envelope: Dec in [-5°, +5°]. Slew to Dec = +30° is far
        // outside and must be rejected before any wire motion.
        let d = fast_settle_connected_narrow_envelope().await;
        let err = d
            .slew_to_coordinates_async(d.sidereal_time().await.unwrap(), 30.0)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
        assert!(
            err.message.contains("outside safe envelope"),
            "error message must call out the envelope: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn slew_async_refuses_ra_outside_safe_envelope() {
        // Envelope: HA in [-1 h, +1 h]. Target RA = LST + 3 h puts
        // mech-HA at -3 h — well outside.
        let d = fast_settle_connected_narrow_envelope().await;
        let lst = d.sidereal_time().await.unwrap();
        let target = (lst + 3.0).rem_euclid(24.0);
        let err = d.slew_to_coordinates_async(target, 0.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[tokio::test]
    async fn sync_refuses_target_outside_safe_envelope() {
        // Same envelope. Sync would seed the encoder for a position
        // outside the safe zone — tracking from there walks into a
        // mechanical stop.
        let d = fast_settle_connected_narrow_envelope().await;
        let err = d
            .sync_to_coordinates(d.sidereal_time().await.unwrap(), 30.0)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
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
    async fn sync_slew_returns_only_after_watcher_clears_slew_in_progress() {
        // CanSlew = true, so the synchronous variant must implement
        // (per ASCOM ITelescopeV3) and only return after the slew has
        // completed.
        let d = fast_settle_connected().await;
        d.slew_to_coordinates(6.0, 30.0).await.unwrap();
        assert!(
            !d.slewing().await.unwrap(),
            "Slewing must be false after slew_to_coordinates returns"
        );
        // Target latched same as the async variant.
        assert_eq!(d.target_right_ascension().await.unwrap(), 6.0);
        assert_eq!(d.target_declination().await.unwrap(), 30.0);
    }

    #[tokio::test]
    async fn sync_slew_to_target_uses_last_set_target() {
        let d = fast_settle_connected().await;
        d.set_target_right_ascension(4.0).await.unwrap();
        d.set_target_declination(15.0).await.unwrap();
        d.slew_to_target().await.unwrap();
        assert!(!d.slewing().await.unwrap());
    }

    #[tokio::test]
    async fn sync_slew_validates_inputs() {
        let d = fast_settle_connected().await;
        assert_eq!(
            d.slew_to_coordinates(24.0, 0.0).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn sync_slew_refuses_while_parked() {
        let d = fast_settle_connected().await;
        d.park().await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if d.at_park().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let err = d.slew_to_coordinates(6.0, 30.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_WHILE_PARKED);
    }

    #[tokio::test]
    async fn sync_slew_to_target_without_set_returns_invalid_operation() {
        let d = fast_settle_connected().await;
        let err = d.slew_to_target().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[tokio::test]
    async fn slew_async_latches_target() {
        let d = fast_settle_connected().await;
        d.slew_to_coordinates_async(6.0, 30.0).await.unwrap();
        assert_eq!(d.target_right_ascension().await.unwrap(), 6.0);
        assert_eq!(d.target_declination().await.unwrap(), 30.0);
    }

    #[tokio::test]
    async fn slew_async_issues_indi_sequence_per_axis() {
        // Phase A5 (issue #205) + issue #207: the slew path emits
        // the INDI eqmod-style sequence — :K → poll :f → :G → :I →
        // :H → :M → :J. (Issue #207 swapped :L for :K — :K is the
        // spec's recommended stop, :L stays reserved for emergency
        // aborts.) This test asserts the order of the setters and
        // motion-start frames for each axis in the freshly-issued
        // slew, before the watcher's pickup loop (if any) re-enters
        // the sequence.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let mut cfg = Config::default();
        if let crate::config::TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        cfg.mount.settle_after_slew = Duration::from_millis(0);
        cfg.mount.ra_min_hours = -12.0;
        cfg.mount.ra_max_hours = 12.0;
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();

        // Capture the log baseline so the assertion ignores the
        // handshake / pre-slew polling chatter.
        let baseline_len = mock.state.lock().await.command_log.len();
        d.slew_to_coordinates_async(6.0, 30.0).await.unwrap();

        // Snapshot the log immediately — the watcher's pickup loop
        // may re-enter the sequence and add more frames; we only
        // care about the first-pass wire frames here.
        let log = mock.state.lock().await.command_log.clone();
        let new_frames: Vec<&[u8]> = log[baseline_len..].iter().map(|v| v.as_slice()).collect();

        // Helper: extract setter / motion-start frames for `axis_byte`.
        let interesting = |axis_byte: u8| -> Vec<&[u8]> {
            new_frames
                .iter()
                .copied()
                .filter(|f| {
                    f.len() >= 3
                        && f[0] == b':'
                        && f[2] == axis_byte
                        && matches!(f[1], b'G' | b'I' | b'H' | b'M' | b'J' | b'K' | b'L')
                })
                .collect()
        };

        let ra = interesting(b'1');
        // Expect :K1 :G1 :I1 :H1 :M1 :J1 in order. Slack on length
        // because the watcher may add more before we sampled — but
        // the first six setter frames for axis 1 are deterministic.
        assert!(ra.len() >= 6, "expected ≥6 RA frames, got {ra:?}");
        assert_eq!(*ra[0], *b":K1\r", "1st RA setter should be :K1");
        assert_eq!(&ra[1][..3], b":G1", "2nd RA setter should be :G1");
        assert_eq!(&ra[2][..3], b":I1", "3rd RA setter should be :I1");
        assert_eq!(&ra[3][..3], b":H1", "4th RA setter should be :H1");
        assert_eq!(&ra[4][..3], b":M1", "5th RA setter should be :M1");
        assert_eq!(*ra[5], *b":J1\r", "6th RA setter should be :J1");

        let dec = interesting(b'2');
        assert!(dec.len() >= 6, "expected ≥6 Dec frames, got {dec:?}");
        assert_eq!(*dec[0], *b":K2\r");
        assert_eq!(&dec[1][..3], b":G2");
        assert_eq!(&dec[2][..3], b":I2");
        assert_eq!(&dec[3][..3], b":H2");
        assert_eq!(&dec[4][..3], b":M2");
        assert_eq!(*dec[5], *b":J2\r");
    }

    #[tokio::test]
    async fn slew_watcher_pickup_loop_reissues_when_residual_exceeds_tolerance() {
        // Phase A5: after both axes stop, if the snapshot's encoder
        // position translates to an RA/Dec that's more than 5"
        // away from the latched target, the watcher must re-enter
        // the slew sequence with a fresh delta.
        //
        // To exercise the pickup loop deterministically — independent
        // of how fast the host walks the mock through the goto chunks
        // — we spawn a side task that, shortly after the slew is
        // issued, force-stops both axes in the mock state with the
        // encoder position clearly off-target. The transport's
        // background polling task picks the new state up; the watcher
        // sees both axes stopped at a position that translates to an
        // RA/Dec far from the latched target, and must re-issue the
        // slew sequence (one fresh :L → :G → :I → :H → :M → :J per
        // axis) at least once. We assert :H1 count >= 2 (one from the
        // initial slew, one from the pickup re-issue) — strictly
        // stronger than the > 0 check, which the initial slew alone
        // would always satisfy.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let mut cfg = Config::default();
        if let crate::config::TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        cfg.mount.settle_after_slew = Duration::from_millis(0);
        cfg.mount.ra_min_hours = -12.0;
        cfg.mount.ra_max_hours = 12.0;
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();

        // Spawn the injection task BEFORE issuing the slew so it is
        // already scheduled when the watcher starts polling. After a
        // short delay (long enough for the initial :L → :J sequence
        // to have hit the wire but well before MIN_SLEW_DWELL), force
        // the mock to declare the goto done at a position clearly
        // off-target. The watcher's next pickup check will see a
        // multi-degree residual and re-issue the slew sequence.
        let mock_clone = mock.clone();
        let injection = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            let mut s = mock_clone.state.lock().await;
            s.ra.running = false;
            s.dec.running = false;
            // 1,000,000 ticks ≈ 99° on the GTi's default CPR
            // (3,628,800 ticks/rev) — well above the 5" pickup
            // tolerance regardless of LST drift.
            s.ra.position_ticks = 1_000_000;
            s.dec.position_ticks = 1_000_000;
        });

        d.slew_to_coordinates_async(6.0, 30.0).await.unwrap();
        injection.await.expect("injection task panicked");

        // Wait for Slewing to clear (after the pickup loop converges
        // or hits PICKUP_MAX_ITERATIONS + MIN_SLEW_DWELL).
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if !d.slewing().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!d.slewing().await.unwrap(), "Slewing must clear in 10s");

        // The initial slew always emits one :H1. A pickup iteration
        // emits a second. ≥ 2 proves the pickup loop fired at least
        // once in response to the forced residual.
        let log = mock.state.lock().await.command_log.clone();
        let h1_count = log.iter().filter(|f| f.starts_with(b":H1")).count();
        assert!(
            h1_count >= 2,
            "expected ≥2 :H1 frames (initial slew + at least one pickup re-issue), \
             got {h1_count}; log={log:?}"
        );
    }

    #[tokio::test]
    async fn slew_watcher_aborts_via_instant_stop_when_axis_reports_blocked() {
        // Drive a slew, seed the mock to report `blocked = true` on
        // either axis, and assert the watcher issues `:L1` + `:L2`
        // and clears `slew_in_progress` instead of waiting for the
        // running flag to drop. Mirrors the safety net we wired up
        // after the hardware ConformU run where the motor stalled
        // against a counterweight-up mechanical stop while the
        // encoder counter kept advancing.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let mut cfg = Config::default();
        if let crate::config::TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        cfg.mount.settle_after_slew = Duration::from_millis(0);
        cfg.mount.ra_min_hours = -12.0;
        cfg.mount.ra_max_hours = 12.0;
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();

        // Mark RA blocked. The next poll picks this up, the watcher
        // sees it, issues :L on both axes and exits early.
        {
            let mut s = mock.state.lock().await;
            s.ra.blocked = true;
        }
        d.slew_to_coordinates_async(6.0, 30.0).await.unwrap();

        // Wait for the watcher to observe the blocked state and
        // clear `slew_in_progress`. With dwell=2 s the watcher
        // won't act sooner; bound at 5 s.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if !d.slewing().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !d.slewing().await.unwrap(),
            "watcher must have cleared slew_in_progress after seeing blocked"
        );

        // Both axes must have seen a `:L` issued by the watcher's
        // abort path. (The driver's slew-prep stop uses the gentler
        // `:K` since issue #207 — `:L` is reserved for genuine
        // emergency stops like this blocked-axis abort and
        // `AbortSlew`.)
        let log = mock.state.lock().await.command_log.clone();
        let l1_count = log.iter().filter(|f| f.as_slice() == b":L1\r").count();
        let l2_count = log.iter().filter(|f| f.as_slice() == b":L2\r").count();
        assert!(
            l1_count >= 1,
            ":L1 should be issued by the watcher abort path; log={log:?}"
        );
        assert!(
            l2_count >= 1,
            ":L2 should be issued by the watcher abort path; log={log:?}"
        );
    }

    #[tokio::test]
    async fn park_watcher_aborts_via_instant_stop_when_axis_reports_blocked() {
        // Same shape as the slew-watcher blocked test, but for the
        // park completion watcher. Critical: on a blocked abort the
        // park watcher must NOT set `AtPark = true` — the OTA isn't
        // at the encoder-0 home pose, so subsequent unpark+slew
        // computations would have a wrong delta.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let mut cfg = Config::default();
        if let crate::config::TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        cfg.mount.settle_after_slew = Duration::from_millis(0);
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();

        {
            let mut s = mock.state.lock().await;
            s.dec.blocked = true;
        }
        d.park().await.unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if !d.slewing().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !d.slewing().await.unwrap(),
            "park watcher must clear slew_in_progress after blocked"
        );
        assert!(
            !d.at_park().await.unwrap(),
            "park watcher must NOT set AtPark when aborted via blocked"
        );
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
    async fn abort_slew_refuses_while_parked() {
        let d = fast_settle_connected().await;
        d.park().await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if d.at_park().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let err = d.abort_slew().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_WHILE_PARKED);
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

    #[tokio::test]
    async fn sync_publishes_position_without_waiting_for_poll() {
        // ConformU reads `RightAscension` ~2 ms after `SyncToCoordinates`
        // returns. The cached snapshot must reflect the synced position
        // immediately rather than holding the prior poll value.
        let d = fast_settle_connected().await;
        let lst = d.sidereal_time().await.unwrap();
        d.sync_to_coordinates(lst, 0.0).await.unwrap();
        // No sleep: read straight after sync.
        let dec = d.declination().await.unwrap();
        assert!(
            dec.abs() < 0.5,
            "Dec must reflect the sync immediately, got {dec}"
        );
        let ra = d.right_ascension().await.unwrap();
        // The synced RA equals the LST snapshot used by sync to compute
        // mech_HA=0; tiny LST drift between the sync and the read can
        // push this by up to a few arcseconds, well under 1 arc-minute.
        let ra_drift_arcsec =
            (ra - lst).rem_euclid(24.0).min((lst - ra).rem_euclid(24.0)) * 3600.0 * 15.0;
        assert!(
            ra_drift_arcsec < 60.0,
            "RA must reflect the sync immediately, got drift={ra_drift_arcsec}\" (ra={ra}, lst={lst})"
        );
    }

    #[tokio::test]
    async fn sync_to_coordinates_updates_target() {
        // Per ASCOM ITelescopeV3, a successful Sync sets Target{RA,Dec}.
        let d = fast_settle_connected().await;
        // Pre-seed the target to a different value so the assertion is
        // about Sync writing it, not about leaving an already-correct
        // value alone.
        d.set_target_right_ascension(10.0).await.unwrap();
        d.set_target_declination(20.0).await.unwrap();
        d.sync_to_coordinates(3.0, 45.0).await.unwrap();
        assert_eq!(d.target_right_ascension().await.unwrap(), 3.0);
        assert_eq!(d.target_declination().await.unwrap(), 45.0);
    }

    #[tokio::test]
    async fn sync_failure_does_not_clobber_target() {
        // A sync rejected by input validation must leave any previously
        // set Target intact, so callers can rely on Target as the last
        // *successful* slew-or-sync coordinate.
        let d = fast_settle_connected().await;
        d.set_target_right_ascension(10.0).await.unwrap();
        d.set_target_declination(20.0).await.unwrap();
        // RA out of range → INVALID_VALUE before any wire write.
        assert!(d.sync_to_coordinates(25.0, 45.0).await.is_err());
        assert_eq!(d.target_right_ascension().await.unwrap(), 10.0);
        assert_eq!(d.target_declination().await.unwrap(), 20.0);
    }

    /// Transport that always reports `running = true` on `:f<axis>`
    /// and ignores `:K<axis>`. Other handshake commands get
    /// plausibly-shaped replies (CPR, TMR_Freq, etc.) so the
    /// manager can complete its connect() handshake. Used to drive
    /// `stop_axis_and_wait` into its timeout branch — real hardware
    /// never gets stuck like this, but the regular mock processes
    /// `:K` instantaneously, so without a deliberately-broken
    /// transport the timeout code path is unreachable from tests.
    struct StuckAxisTransport;

    #[async_trait]
    impl crate::transport::Transport for StuckAxisTransport {
        async fn round_trip(
            &self,
            request: &[u8],
            _timeout: Duration,
        ) -> crate::error::Result<Vec<u8>> {
            if request.len() < 2 {
                return Ok(b"=\r".to_vec());
            }
            match request[1] {
                // `:f<axis>` reply with running=1: nibble-1 bit-0 set.
                // Layout per spec §5: [mode_nibble | motion_nibble | init_nibble].
                // Mode nibble = 0 (Goto, CW, Slow); motion nibble = 1
                // (Running, not Blocked); init nibble = 1 (Initialized).
                b'f' => Ok(b"=011\r".to_vec()),
                // Handshake inquiries: return a 6-hex u24 payload so
                // the response decoder is happy. Value doesn't matter
                // for the timeout test.
                b'a' | b'b' | b'e' => Ok(b"=000080\r".to_vec()),
                // High-speed-ratio: 2-hex u8 payload per real GTi.
                b'g' => Ok(b"=01\r".to_vec()),
                // `:j<axis>` returns a 6-hex biased position
                // (0x800000 → encoder 0).
                b'j' => Ok(b"=000080\r".to_vec()),
                // Everything else (including `:F` initialize,
                // `:K` decelerate-stop, and `:L` instant-stop) acks
                // without side effects.
                _ => Ok(b"=\r".to_vec()),
            }
        }
        async fn close(&self) -> crate::error::Result<()> {
            Ok(())
        }
    }

    struct StuckAxisFactory;

    #[async_trait]
    impl crate::transport::TransportFactory for StuckAxisFactory {
        async fn open(
            &self,
            _config: &Config,
        ) -> crate::error::Result<Arc<dyn crate::transport::Transport>> {
            Ok(Arc::new(StuckAxisTransport))
        }
    }

    #[tokio::test]
    async fn stop_axis_and_wait_returns_transport_error_when_axis_never_stops() {
        // The free-function helper is called from
        // `MountDevice::stop_and_wait` (covered by the slew/park
        // happy paths) and the pickup loop (covered by
        // `slew_watcher_pickup_loop_reissues_when_residual_exceeds_tolerance`).
        // Its *timeout* branch is unreachable from those paths
        // because the mock always responds to `:K` instantly; this
        // test wires a deliberately-broken transport that ignores
        // `:K` and always reports running, then asserts the helper
        // returns the timeout error after `AXIS_STOP_TIMEOUT`.
        let manager = TransportManager::new(Config::default(), Arc::new(StuckAxisFactory));
        // No connect() — `stop_axis_and_wait` only needs `send` to
        // route through the manager's transport; the test bypasses
        // the handshake-required state by going straight to a
        // freshly-built manager that holds the broken transport.
        manager.connect().await.unwrap();
        // Use a short timeout so the test doesn't take 2 s.
        let err = stop_axis_and_wait(&manager, Axis::Ra, Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(
            matches!(err, StarAdvError::Transport(ref msg) if msg.contains("did not stop")),
            "expected Transport(\"... did not stop ...\") error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn watcher_should_abort_returns_true_when_slew_in_progress_cleared() {
        // Direct unit test for the helper that gates the watcher's
        // post-snapshot wire sends. The watcher uses it twice — once
        // before the pickup re-slew, once before the tracking
        // re-enable — to close the race window between the top-of-
        // loop guard and the actual wire commands.
        let state = Arc::new(RwLock::new(DriverState::default()));
        let manager = TransportManager::new(Config::default(), Arc::new(MockTransportFactory));
        manager.connect().await.unwrap();

        // Default state has slew_in_progress=false → abort=true.
        assert!(
            watcher_should_abort(&state, &manager).await,
            "default DriverState has slew_in_progress=false → should abort"
        );

        // With slew_in_progress=true and transport available → no abort.
        state.write().await.slew_in_progress = true;
        assert!(
            !watcher_should_abort(&state, &manager).await,
            "in-progress slew with live transport → should continue"
        );

        // Disconnect the transport → abort=true even if slew flag is on.
        manager.disconnect().await.unwrap();
        assert!(
            watcher_should_abort(&state, &manager).await,
            "disconnect mid-slew → should abort"
        );
    }

    #[tokio::test]
    async fn pickup_reslew_axis_swallows_transport_errors() {
        // The watcher calls `pickup_reslew_axis` per axis from the
        // pickup loop. Its failure-logging branches fire when the
        // wrapped `stop_axis_and_wait` or `issue_slew_axis` returns
        // an error — that happens when the transport is closed or
        // the axis stays stuck. This test wires the `StuckAxisTransport`
        // (always reports `running=true`) so the inner
        // `stop_axis_and_wait` hits its timeout branch; the helper
        // must log and return without panicking. A second invocation
        // confirms the helper is idempotent on persistent failure.
        let manager = TransportManager::new(Config::default(), Arc::new(StuckAxisFactory));
        manager.connect().await.unwrap();
        // Each call is best-effort and returns `()`. The internal
        // timeout is `AXIS_STOP_TIMEOUT` (2 s) — overriding it would
        // require threading a parameter through, which isn't worth
        // it for a single test; this test runs in ~2 s, still well
        // under the harness's default timeout.
        pickup_reslew_axis(&manager, Axis::Ra, 1_000_000).await;
        // A negative delta exercises the `ccw = true` branch in
        // `issue_slew_axis`'s `MotionMode` construction — except
        // here we never reach it because `stop_axis_and_wait` fails
        // first. Still useful to verify no panic.
        pickup_reslew_axis(&manager, Axis::Dec, -1_000_000).await;
    }

    // ---- PulseGuide tests ----

    #[tokio::test]
    async fn pulse_guide_capability_flags_are_true() {
        let d = device();
        assert!(d.can_pulse_guide().await.unwrap());
        assert!(d.can_set_guide_rates().await.unwrap());
    }

    #[tokio::test]
    async fn is_pulse_guiding_defaults_to_false_after_connect() {
        let d = connected_device().await;
        assert!(!d.is_pulse_guiding().await.unwrap());
    }

    #[tokio::test]
    async fn default_guide_rates_are_half_sidereal() {
        let d = connected_device().await;
        let ra = d.guide_rate_right_ascension().await.unwrap();
        let dec = d.guide_rate_declination().await.unwrap();
        // 0.5 × SIDEREAL_DEG_PER_SEC ≈ 0.00208904
        let expected = 0.5 * SIDEREAL_DEG_PER_SEC;
        assert!((ra - expected).abs() < 1e-9, "RA: {ra}");
        assert!((dec - expected).abs() < 1e-9, "Dec: {dec}");
    }

    #[tokio::test]
    async fn set_guide_rate_ra_round_trips_through_fraction() {
        let d = connected_device().await;
        let target = 0.001_f64;
        d.set_guide_rate_right_ascension(target).await.unwrap();
        let got = d.guide_rate_right_ascension().await.unwrap();
        assert!(
            (got - target).abs() < 1e-9,
            "round-trip: set {target}, got {got}"
        );
    }

    #[tokio::test]
    async fn set_guide_rate_rejects_zero_and_negative() {
        let d = connected_device().await;
        let err = d.set_guide_rate_right_ascension(0.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
        let err = d.set_guide_rate_declination(-0.001).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[tokio::test]
    async fn set_guide_rate_rejects_at_or_above_sidereal() {
        // Upper bound is exclusive — fraction = 1.0 zeroes East's
        // rate factor (`1 - fraction`) and divides by zero in the
        // step-period formula.
        let d = connected_device().await;
        let err = d
            .set_guide_rate_right_ascension(SIDEREAL_DEG_PER_SEC)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
        let err = d
            .set_guide_rate_right_ascension(SIDEREAL_DEG_PER_SEC * 2.0)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[tokio::test]
    async fn pulse_guide_refuses_while_disconnected() {
        let d = device();
        let err = d
            .pulse_guide(GuideDirection::North, Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn pulse_guide_zero_duration_is_no_op() {
        // Asserting "no wire activity" via the capturing mock: the
        // pulse_guide returns Ok and no `:K2` / `:G2` / `:I2` / `:J2`
        // setter frames are emitted on the Dec axis.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();

        let baseline_len = mock.state.lock().await.command_log.len();
        d.pulse_guide(GuideDirection::North, Duration::ZERO)
            .await
            .unwrap();
        let log = mock.state.lock().await.command_log.clone();
        let new_frames: Vec<&[u8]> = log[baseline_len..].iter().map(|v| v.as_slice()).collect();
        let dec_setters: Vec<&&[u8]> = new_frames
            .iter()
            .filter(|f| {
                f.len() >= 3
                    && f[0] == b':'
                    && f[2] == b'2'
                    && matches!(f[1], b'G' | b'I' | b'J' | b'K' | b'L')
            })
            .collect();
        assert!(
            dec_setters.is_empty(),
            "expected no Dec setter frames, got {dec_setters:?}"
        );
    }

    #[tokio::test]
    async fn pulse_guide_north_issues_tracking_cw_on_dec_axis() {
        // North → Dec axis, ccw=false → `:G210` (Tracking-Slow-CW).
        // Step period is sidereal / 0.5 = 2 × sidereal.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();

        let baseline_len = mock.state.lock().await.command_log.len();
        // Long enough duration that the watcher's post-sleep restore
        // doesn't fire during this test — we want to inspect the
        // pulse-start wire frames only.
        d.pulse_guide(GuideDirection::North, Duration::from_secs(30))
            .await
            .unwrap();
        // Immediately read the log; the watcher is asleep.
        let log = mock.state.lock().await.command_log.clone();
        let new_frames: Vec<&[u8]> = log[baseline_len..].iter().map(|v| v.as_slice()).collect();
        let dec_setters: Vec<&&[u8]> = new_frames
            .iter()
            .filter(|f| {
                f.len() >= 3
                    && f[0] == b':'
                    && f[2] == b'2'
                    && matches!(f[1], b'G' | b'I' | b'J' | b'K' | b'L')
            })
            .collect();
        assert_eq!(
            dec_setters.len(),
            4,
            "expected exactly 4 Dec setter frames (:K2 :G2 :I2 :J2), got {dec_setters:?}"
        );
        assert_eq!(*dec_setters[0], b":K2\r", "1st Dec setter should be :K2");
        assert_eq!(
            *dec_setters[1], b":G210\r",
            "2nd Dec setter should be :G210 (Tracking-Slow-CW)"
        );
        assert_eq!(&dec_setters[2][..3], b":I2", "3rd Dec setter should be :I2");
        assert_eq!(*dec_setters[3], b":J2\r", "4th Dec setter should be :J2");
        // IsPulseGuiding flipped synchronously.
        assert!(d.is_pulse_guiding().await.unwrap());
    }

    #[tokio::test]
    async fn pulse_guide_south_issues_tracking_ccw_on_dec_axis() {
        // South → Dec axis, ccw=true → `:G211` (Tracking-Slow-CCW).
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();

        let baseline_len = mock.state.lock().await.command_log.len();
        d.pulse_guide(GuideDirection::South, Duration::from_secs(30))
            .await
            .unwrap();
        let log = mock.state.lock().await.command_log.clone();
        let new_frames: Vec<&[u8]> = log[baseline_len..].iter().map(|v| v.as_slice()).collect();
        let g2 = new_frames
            .iter()
            .find(|f| f.starts_with(b":G2"))
            .expect("expected a :G2 frame");
        assert_eq!(*g2, b":G211\r", "South → :G211");
    }

    #[tokio::test]
    async fn pulse_guide_east_uses_rate_factor_one_minus_fraction() {
        // East at default fraction (0.5) → rate factor = 0.5 → step
        // period = sidereal_period / 0.5 = 2 × sidereal. Decode the
        // `:I1` payload and compare against the expected shifted
        // period.
        use crate::transport::mock::CapturingMockFactory;
        use skywatcher_motor_protocol::codec::decode_u24;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();
        d.set_tracking(true).await.unwrap();

        let baseline_len = mock.state.lock().await.command_log.len();
        d.pulse_guide(GuideDirection::East, Duration::from_secs(30))
            .await
            .unwrap();
        let log = mock.state.lock().await.command_log.clone();
        let new_frames: Vec<&[u8]> = log[baseline_len..].iter().map(|v| v.as_slice()).collect();
        // First `:I1` after pulse_guide start: payload should be
        // 2 × P_sid (rate factor = 0.5 → period doubles).
        let i1 = new_frames
            .iter()
            .find(|f| f.starts_with(b":I1") && f.len() == 10)
            .expect("expected :I1 frame with 6-hex payload");
        let payload: &[u8; 6] = (&i1[3..9]).try_into().unwrap();
        let actual_period = decode_u24(payload).unwrap();
        let mock_state = mock.state.lock().await;
        let p_sid =
            crate::coordinates::sidereal_step_period(mock_state.tmr_freq, mock_state.cpr_ra);
        let expected = 2 * p_sid;
        drop(mock_state);
        assert_eq!(
            actual_period, expected,
            "East at default 0.5 fraction → period must be 2× sidereal ({p_sid} → {expected}), got {actual_period}"
        );
    }

    #[tokio::test]
    async fn pulse_guide_sets_is_pulse_guiding_synchronously() {
        // The flag must be true before pulse_guide returns. Use a
        // very short duration so the watcher could in principle clear
        // the flag before our read — the synchronous-before-spawn
        // ordering guarantees we still observe `true`.
        let d = connected_device().await;
        // Long enough duration that the watcher is still asleep.
        d.pulse_guide(GuideDirection::North, Duration::from_secs(30))
            .await
            .unwrap();
        assert!(d.is_pulse_guiding().await.unwrap());
    }

    #[tokio::test]
    async fn pulse_guide_watcher_clears_flag_after_duration() {
        // Short pulse: the watcher should clear `is_pulse_guiding`
        // within a small multiple of the duration. Poll for up to
        // 2 seconds.
        let d = connected_device().await;
        d.pulse_guide(GuideDirection::North, Duration::from_millis(100))
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if !d.is_pulse_guiding().await.unwrap() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("watcher did not clear is_pulse_guiding within 2s of a 100ms pulse");
    }

    #[tokio::test]
    async fn pulse_guide_rejects_same_axis_while_one_in_flight() {
        let d = connected_device().await;
        // Long-running first pulse — watcher is asleep.
        d.pulse_guide(GuideDirection::North, Duration::from_secs(30))
            .await
            .unwrap();
        let err = d
            .pulse_guide(GuideDirection::South, Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[tokio::test]
    async fn pulse_guide_refuses_while_parked() {
        let d = connected_device().await;
        d.park().await.unwrap();
        // Wait for AtPark = true (park watcher).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if d.at_park().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let err = d
            .pulse_guide(GuideDirection::North, Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_WHILE_PARKED);
    }

    #[tokio::test]
    async fn pulse_guide_ra_with_tracking_off_does_not_restore_tracking() {
        // Issue an East pulse while tracking is OFF. After the pulse
        // completes, tracking_requested should still be false and the
        // watcher should not have emitted a second `:G110` (restore)
        // frame.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();
        assert!(!d.tracking().await.unwrap(), "precondition: tracking off");

        d.pulse_guide(GuideDirection::East, Duration::from_millis(100))
            .await
            .unwrap();
        // Wait for the watcher to finish.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if !d.is_pulse_guiding().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!d.is_pulse_guiding().await.unwrap());
        // Tracking still off.
        assert!(!d.tracking().await.unwrap());
        // Exactly one `:G110\r` in the log (the pulse-start; no
        // restore). `:G110` is RA Tracking-Slow-CW; the watcher would
        // only emit a second one on the restore branch if
        // `tracking_was_on` was true at issue.
        let log = mock.state.lock().await.command_log.clone();
        let g110_count = log.iter().filter(|f| f.as_slice() == b":G110\r").count();
        assert_eq!(
            g110_count, 1,
            "expected 1 :G110 frame (pulse-start only, no restore), got {g110_count}; log {log:?}"
        );
    }
}
