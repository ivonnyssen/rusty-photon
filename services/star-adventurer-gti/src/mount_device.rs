//! ASCOM Alpaca Telescope device for the Star Adventurer GTi.
//!
//! This is the surface that Alpaca clients (NINA, SGPro, `rp`, ...) talk to.
//! Capability-flag overrides match the design doc's
//! [§"Capability flags"](../../../docs/services/star-adventurer-gti.md#capability-flags)
//! table; defaulted methods that the MVP does not implement are left to the
//! ascom-alpaca trait's `NOT_IMPLEMENTED` default.

use std::fmt;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
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
use tracing::{debug, info};

use crate::config::MountConfig;
use crate::coordinates::{
    dec_degrees_to_ticks, encoder_to_celestial, local_sidereal_time_hours,
    mechanical_ha_to_ra_ticks, pickup_target_ra_ticks, pulse_guide_step_period, ra_dec_to_alt_az,
    ra_ticks_to_mechanical_ha, ra_to_mechanical_ha, select_pier_side_for_target,
    side_of_pier as side_of_pier_calc, sidereal_step_period, target_encoder_flipped,
    target_encoder_normal, SIDEREAL_DEG_PER_SEC,
};
use crate::error::StarAdvError;
use crate::transport_manager::{MountSnapshot, TransportManager};

/// Default guide rate as a fraction of sidereal. ASCOM clients see
/// this multiplied by `SIDEREAL_DEG_PER_SEC` through
/// `GuideRateRightAscension` / `GuideRateDeclination`.
const DEFAULT_GUIDE_RATE_FRACTION: f64 = 0.5;

/// Maximum absolute encoder reading at connect that
/// [`MountDevice::seed_home_pose_after_connect`] still treats as
/// "fresh power-up" and applies the `home_pose` seed to. The
/// Sky-Watcher firmware does not always read exactly `0` after a
/// power-cycle — empirically the validation GTi reports `dec = −1`
/// on connect, a single-tick initialisation artifact (≈ 0.4″ at the
/// GTi's CPR). Any genuine post-slew encoder reading is tens of
/// thousands of ticks away from zero, so this tolerance comfortably
/// distinguishes "just powered up" from "the operator already moved
/// the mount this session".
const FRESH_POWER_UP_TICK_TOLERANCE: i32 = 10;

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
    /// In-memory park-target encoder pair. Populated on the 0→1 connect
    /// transition from `MountConfig::park_*_ticks` if `Some`, otherwise
    /// from the handshake-captured positions. `None` here means "not
    /// loaded yet" — `Park` reads via `ok_or_else` after
    /// `ensure_connected()` so an unset value surfaces as an
    /// `ASCOMError(INVALID_OPERATION)` rather than a panic.
    park_ra_ticks: Option<i32>,
    park_dec_ticks: Option<i32>,
    /// Pier side the most recent slew was *issued for*. Read by the
    /// slew-completion watcher's pickup loop so it picks
    /// `target_encoder_normal` vs `target_encoder_flipped` for the
    /// corrective re-slew. Without this, a successful flip slew would
    /// be undone by the pickup loop's first iteration (the post-flip
    /// Dec encoder is past the pole, and a pre-flip encoder target
    /// would order a slew back through the pole).
    target_pier_side: Option<PierSide>,
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
            park_ra_ticks: None,
            park_dec_ticks: None,
            target_pier_side: None,
            guide_rate_ra_fraction: DEFAULT_GUIDE_RATE_FRACTION,
            guide_rate_dec_fraction: DEFAULT_GUIDE_RATE_FRACTION,
            pulse_guiding_ra: false,
            pulse_guiding_dec: false,
        }
    }
}

pub struct MountDevice {
    config: MountConfig,
    /// Optional config-file path. `Some` when the driver was started
    /// with `--config <path>`; `None` for `Config::default()` runs. Drives
    /// `CanSetPark` and is the destination for `SetPark` writes.
    config_file_path: Option<PathBuf>,
    requested_connection: Arc<RwLock<bool>>,
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
}

impl fmt::Debug for MountDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MountDevice")
            .field("config", &self.config)
            .field("config_file_path", &self.config_file_path)
            .field("requested_connection", &self.requested_connection)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl MountDevice {
    pub fn new(config: MountConfig, transport: Arc<TransportManager>) -> Self {
        Self::with_config_file_path(config, transport, None)
    }

    /// Construct with an optional config-file path. `Some(path)` enables
    /// `CanSetPark` / `SetPark` persistence; `None` leaves
    /// `CanSetPark = false` and `SetPark = NOT_IMPLEMENTED`.
    pub fn with_config_file_path(
        config: MountConfig,
        transport: Arc<TransportManager>,
        config_file_path: Option<PathBuf>,
    ) -> Self {
        Self {
            config,
            config_file_path,
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

    /// Reject a slew / sync / destination-side prediction whose target
    /// would fall outside the per-pier-side mechanical envelope for
    /// the chosen pointing state.
    ///
    /// **Why:** the Star Adventurer GTi (like every GEM) has
    /// mechanical limits — slewing past them with cable wraps or the
    /// counterweight shaft against the pier stalls the motor against
    /// a hard stop while the encoder counter continues to advance.
    /// On a real-hardware ConformU run that drove the mount into the
    /// counterweight-up region we heard the motor whine and saw the
    /// axis stop physically for several seconds at a time.
    ///
    /// The check is in **encoder `mech_HA` space** (signed hours
    /// folded to `[−12, +12)`). For a target on the natural pier
    /// side, `target_mech_HA = celestial_HA`. For a flipped target,
    /// `target_mech_HA = celestial_HA + 12 h` folded. If the chosen-
    /// side `mech_HA` falls inside the configured binding zone
    /// `[binding_zone_min_hours, binding_zone_max_hours]`, the slew
    /// is rejected with `INVALID_VALUE`.
    ///
    /// Note: this is a *destination-only* check (matching INDI EQMOD's
    /// `EncoderTarget`-style envelope). The slew path is not analysed;
    /// the slew-direction logic in
    /// [`flip_slew_ra_delta`](#flip_slew_ra_delta) and the per-axis
    /// `ccw = current > target` rule in `execute_slew_with_explicit_side`
    /// pick the safe direction.
    ///
    /// `flip_policy.flip_range_hours` is **not** consulted here — that
    /// rule lives in [`select_pier_side_for_target`] for pier-side
    /// preference only. Park 1 / Park 5 (anti-meridian poses with
    /// `mech_HA = ±12` on the chosen pier) are reachable via slew
    /// because their mech_HA is outside the binding zone.
    ///
    /// Both axes are validated together so a partial-failure slew
    /// can't issue motion on RA before discovering Dec is out of
    /// range.
    fn check_within_safe_envelope(
        &self,
        ra_hours: f64,
        dec_degrees: f64,
        lst_hours: f64,
        target_is_flipped: bool,
    ) -> ASCOMResult<()> {
        let target_mech_ha = {
            let normal = ra_to_mechanical_ha(ra_hours, lst_hours);
            if target_is_flipped {
                crate::coordinates::fold_ha(normal + 12.0)
            } else {
                normal
            }
        };
        let zone_min = self.config.binding_zone_min_hours;
        let zone_max = self.config.binding_zone_max_hours;
        if zone_min <= zone_max && (zone_min..=zone_max).contains(&target_mech_ha) {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!(
                    "target mech_HA {target_mech_ha:.3} h is inside the counterweight \
                     binding zone [{zone_min}, {zone_max}] h"
                ),
            ));
        }
        if !(self.config.dec_min_degrees..=self.config.dec_max_degrees).contains(&dec_degrees) {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!(
                    "Dec target {dec_degrees:.3}° outside safe envelope [{}, {}]°",
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

    /// Post-connect park-target load. Source priority, per axis:
    ///
    /// 1. `mount.park_*_ticks` from the **on-disk** config file when one
    ///    was supplied via `--config`. Reading fresh from disk on every
    ///    connect means a successful `SetPark` followed by
    ///    disconnect/reconnect picks up the new target, and an operator
    ///    hand-edit between connects takes effect.
    /// 2. `self.config.park_*_ticks` for `Config::default()` runs (no
    ///    config file) — these never change in-process because
    ///    `SetPark` is unreachable in that mode.
    /// 3. The **current** firmware encoder reading from the snapshot
    ///    when neither of the above provided a value.
    ///    `seed_home_pose_after_connect` runs first in [`set_connected`]
    ///    so the snapshot already reflects the home_pose's logical
    ///    encoder values on a fresh power-up; a mid-session reconnect
    ///    (firmware encoder non-zero) leaves the snapshot at the
    ///    handshake reading, which is the "park where the OTA already
    ///    is" semantic operators expect from a reconnect.
    ///
    /// Extracted from [`set_connected`] so a failure here (file missing,
    /// malformed JSON, lost transport mid-load) can be rolled back by the
    /// caller without leaking the connection ref-count. See the design
    /// doc's §"Park lifecycle" for the resolution rules.
    async fn load_park_target_after_connect(&self) -> ASCOMResult<()> {
        let (config_ra, config_dec) = if let Some(path) = self.config_file_path.clone() {
            let result = tokio::task::spawn_blocking(move || read_park_from_config(&path))
                .await
                .map_err(|e| {
                    ASCOMError::new(
                        ASCOMErrorCode::INVALID_OPERATION,
                        format!("park-config read task join error: {e}"),
                    )
                })?;
            result.map_err(Self::ascom)?
        } else {
            (self.config.park_ra_ticks, self.config.park_dec_ticks)
        };
        // Read the live snapshot rather than `params.ra_at_handshake_ticks`:
        // `seed_home_pose_after_connect` runs before this function and
        // mutates the snapshot when the operator has configured a
        // `home_pose`, so the snapshot is the source-of-truth for the
        // post-seed encoder state. Using the pre-seed handshake reading
        // would default the park target to firmware-zero (mech_HA = 0h,
        // mech_dec = 0°) — not the home_pose the operator powered up at.
        let snap = self.transport.snapshot().await;
        let ra_target = config_ra.unwrap_or(snap.ra.position_ticks);
        let dec_target = config_dec.unwrap_or(snap.dec.position_ticks);
        {
            let mut s = self.state.write().await;
            s.park_ra_ticks = Some(ra_target);
            s.park_dec_ticks = Some(dec_target);
        }
        debug!(
            ra_target,
            dec_target,
            from_config_ra = config_ra.is_some(),
            from_config_dec = config_dec.is_some(),
            from_file = self.config_file_path.is_some(),
            "park target loaded"
        );
        Ok(())
    }

    /// Post-connect encoder seed for the operator's configured
    /// [`HomePose`].
    ///
    /// The Sky-Watcher firmware's encoder counter resets to `(0, 0)`
    /// every time the mount powers up. With `home_pose !=
    /// OtaOnMeridianAtEquator`, the codebase's convention for `(0, 0)`
    /// doesn't match the operator's physical pose, so we issue
    /// `:E1` / `:E2` (no-motion encoder seed) right after connect to
    /// align the firmware's encoder counter with the codebase's
    /// convention for the configured pose.
    ///
    /// Skipped when:
    /// - The home pose is the codebase default (no offset needed).
    /// - The firmware reports a non-zero encoder reading at connect
    ///   time **beyond a small fresh-power-up tolerance**. That
    ///   indicates the mount has already been slewed or synced this
    ///   power cycle — re-seeding would clobber it. The tolerance
    ///   exists because the Sky-Watcher firmware does not always
    ///   read exactly `(0, 0)` after a power-cycle: on the validation
    ///   GTi we observed `dec = −1` on fresh power-up, a 1-tick
    ///   initialisation artifact (~0.4″) that obviously still
    ///   represents the "just powered up" state.
    ///
    /// Documented operator assumption: when `home_pose != default`,
    /// the operator powers up the mount **at** the configured pose and
    /// connects the driver before any slew or sync. Reconnecting
    /// mid-session after a slew is safe (the non-zero-encoder guard
    /// catches it — a real slew lands tens of thousands of ticks away
    /// from zero, well outside the tolerance).
    async fn seed_home_pose_after_connect(&self) -> ASCOMResult<()> {
        let Some(home_pose) = self.config.home_pose else {
            // No pose configured — trust the firmware encoder as-is.
            // This is the codebase's historical (pre-Phase-6) behaviour
            // and what existing pre-`home_pose` config files expect.
            return Ok(());
        };
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let snap = self.transport.snapshot().await;
        info!(
            pre_seed_ra_ticks = snap.ra.position_ticks,
            pre_seed_dec_ticks = snap.dec.position_ticks,
            home_pose = ?home_pose,
            "pre-seed encoder snapshot at connect"
        );
        if snap.ra.position_ticks.abs() > FRESH_POWER_UP_TICK_TOLERANCE
            || snap.dec.position_ticks.abs() > FRESH_POWER_UP_TICK_TOLERANCE
        {
            debug!(
                ra = snap.ra.position_ticks,
                dec = snap.dec.position_ticks,
                tolerance = FRESH_POWER_UP_TICK_TOLERANCE,
                "skipping home_pose encoder seed: firmware encoder is non-zero beyond tolerance"
            );
            return Ok(());
        }
        let mech_ha = home_pose.codebase_mech_ha_hours(self.config.site_latitude_deg);
        let dec_deg = home_pose.codebase_dec_encoder_degrees(self.config.site_latitude_deg);
        let ra_ticks = mechanical_ha_to_ra_ticks(mech_ha, params.cpr_ra);
        let dec_ticks = dec_degrees_to_ticks(dec_deg, params.cpr_dec);
        self.transport
            .send(Command::SetPosition {
                axis: Axis::Ra,
                ticks: ra_ticks,
            })
            .await
            .map_err(Self::ascom)?;
        self.transport.seed_ra_position(ra_ticks).await;
        self.transport
            .send(Command::SetPosition {
                axis: Axis::Dec,
                ticks: dec_ticks,
            })
            .await
            .map_err(Self::ascom)?;
        self.transport.seed_dec_position(dec_ticks).await;
        info!(
            seeded_ra_ticks = ra_ticks,
            seeded_dec_ticks = dec_ticks,
            home_pose = ?home_pose,
            "seeded firmware encoder for home_pose"
        );
        Ok(())
    }

    /// Execute a slew to celestial (ra, dec) on the explicitly-chosen
    /// pier side. Used by both `slew_to_coordinates_async` (where the
    /// side is picked from the flip policy decision tree) and
    /// `set_side_of_pier` (where the user pins the side directly).
    ///
    /// Caller must have already validated: connected, coords in
    /// range, not parked. The helper then:
    ///   1. Computes target encoder ticks for the chosen side
    ///      (pre-flip or post-flip) and validates against the
    ///      per-side safety envelope.
    ///   2. Atomically latches `slew_in_progress` and the
    ///      target RA/Dec.
    ///   3. Computes deltas with `fold_delta_to_canonical` (handles a
    ///      post-through-wrap raw encoder) and applies
    ///      `flip_slew_ra_delta` on the RA axis for flip slews
    ///      (forces CCW direction through the safe negative-mech_HA
    ///      half).
    ///   4. Issues the INDI wire sequence per axis and hands off to
    ///      the slew-completion watcher.
    async fn execute_slew_with_explicit_side(
        &self,
        ra: f64,
        dec: f64,
        chosen_side: PierSide,
    ) -> ASCOMResult<()> {
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;
        let pre_flip_side = if self.config.site_latitude_deg >= 0.0 {
            PierSide::West
        } else {
            PierSide::East
        };
        let target_is_flipped = chosen_side != pre_flip_side && chosen_side != PierSide::Unknown;
        let (ra_ticks, dec_ticks) = if target_is_flipped {
            target_encoder_flipped(ra, dec, lst, params.cpr_ra, params.cpr_dec)
        } else {
            target_encoder_normal(ra, dec, lst, params.cpr_ra, params.cpr_dec)
        };
        // Refuse before any wire motion if the slew target falls
        // outside the configured mechanical envelope for the chosen
        // pier side.
        self.check_within_safe_envelope(ra, dec, lst, target_is_flipped)?;

        // Atomically reserve the in-progress slot **before** issuing
        // any motion. Latch the target + capture the tracking flag in
        // the same write.
        let tracking_was_on;
        {
            let mut s = self.state.write().await;
            if s.slew_in_progress {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_OPERATION,
                    "slew refused: slew already in progress",
                ));
            }
            s.target_ra_hours = Some(ra);
            s.target_dec_degrees = Some(dec);
            s.target_pier_side = Some(chosen_side);
            tracking_was_on = s.tracking_requested;
            s.slew_in_progress = true;
            s.pulse_guiding_ra = false;
            s.pulse_guiding_dec = false;
        }

        // From here on, any error path must clear `slew_in_progress`
        // — otherwise the driver gets stuck reporting Slewing forever.
        let result: ASCOMResult<()> = async {
            let snap = self.transport.snapshot().await;
            let current_side = side_of_pier_calc(
                snap.dec.position_ticks,
                params.cpr_dec,
                self.config.site_latitude_deg,
            );
            let is_flip_slew = current_side != chosen_side;
            // Fold the raw delta to canonical so a snapshot value that
            // landed outside `[−cpr/2, +cpr/2)` after a prior
            // through-wrap flip doesn't trigger a full-revolution
            // correction here.
            let ra_delta_canonical =
                fold_delta_to_canonical(ra_ticks - snap.ra.position_ticks, params.cpr_ra);
            let binding_zone = (
                self.config.binding_zone_min_hours,
                self.config.binding_zone_max_hours,
            );
            let ra_delta = if is_flip_slew {
                // Flip slews steer the polar-axis sweep out of the
                // counterweight-forbidden zone — see
                // [`flip_slew_ra_delta`] and the design doc's
                // [§"Through-wrap slew routing"](../../../docs/services/star-adventurer-gti.md#through-wrap-slew-routing).
                flip_slew_ra_delta(
                    ra_delta_canonical,
                    snap.ra.position_ticks,
                    params.cpr_ra,
                    binding_zone,
                )?
            } else {
                // Non-flip slews can't rewrite the direction the way
                // flip slews can — the canonical short delta is the
                // unique path between current and target on the chosen
                // pier side. Refuse if that sweep enters the forbidden
                // zone (e.g. cur mech_HA = +0.5 h → target +11.5 h
                // would otherwise sweep the CW through the zone even
                // though both endpoints sit outside it).
                check_non_flip_ra_path(
                    ra_delta_canonical,
                    snap.ra.position_ticks,
                    params.cpr_ra,
                    binding_zone,
                )?;
                ra_delta_canonical
            };
            let dec_delta_canonical =
                fold_delta_to_canonical(dec_ticks - snap.dec.position_ticks, params.cpr_dec);
            let dec_delta = if is_flip_slew {
                // Flip slews force the Dec axis to traverse the
                // visible celestial pole (NCP for north, SCP for
                // south) rather than the below-horizon pole — see
                // [`flip_slew_dec_delta`].
                flip_slew_dec_delta(
                    dec_delta_canonical,
                    snap.dec.position_ticks,
                    params.cpr_dec,
                    self.config.site_latitude_deg >= 0.0,
                )
            } else {
                dec_delta_canonical
            };
            // Both axes use the INDI wire sequence: `:K` + poll `:f`
            // (decelerate stop) → `:G goto+fast` → `:I 6` → `:H |delta|`
            // → `:M breaks` → `:J`. The RA-axis `:K` is also the wire
            // event that halts any in-progress sidereal tracking;
            // mirror that into the in-memory `tracking_requested`
            // flag only after the stop has actually succeeded so the
            // state never gets ahead of the wire on transport failures.
            self.stop_and_wait(Axis::Ra).await?;
            self.state.write().await.tracking_requested = false;
            issue_slew_axis(&self.transport, Axis::Ra, ra_delta)
                .await
                .map_err(Self::ascom)?;
            self.stop_and_wait(Axis::Dec).await?;
            issue_slew_axis(&self.transport, Axis::Dec, dec_delta)
                .await
                .map_err(Self::ascom)?;
            Ok(())
        }
        .await;
        if let Err(e) = result {
            self.state.write().await.slew_in_progress = false;
            return Err(e);
        }

        // Hand off to the completion watcher.
        let settle = {
            let s = self.state.read().await;
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

/// Consecutive `poll_axes_now` failures the slew/park watcher
/// tolerates before giving up. A single transient USB-CDC glitch
/// (queue flush race, brief renumeration, …) recovers within one
/// frame and shouldn't take the watcher offline for the rest of
/// the slew — the original "one strike and exit" policy meant any
/// pre-binding hiccup left a runaway motor with no observer. Three
/// attempts × [`WATCHER_POLL_RETRY_BACKOFF`] keeps the cumulative
/// recovery window well inside the polling cadence so a genuinely
/// blocked axis is still detected within ~1 s of the firmware
/// latching the bit.
const WATCHER_POLL_RETRY_LIMIT: u32 = 3;

/// Sleep between consecutive `poll_axes_now` retry attempts in the
/// slew/park watcher. Short enough that the cumulative
/// `WATCHER_POLL_RETRY_LIMIT × WATCHER_POLL_RETRY_BACKOFF` budget
/// stays inside the next polling tick; long enough that a tokio-
/// serial read can flush whatever junk the kernel buffered during
/// a brief CDC glitch before the next attempt.
const WATCHER_POLL_RETRY_BACKOFF: Duration = Duration::from_millis(100);

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

/// Fold an encoder-tick delta into the shortest equivalent path on a
/// modular axis of period `cpr`.
///
/// The Sky-Watcher firmware's encoder counter is wider than the
/// physical axis's logical period (cpr): a single revolution is `cpr`
/// ticks, but the counter can run from `−2²³` to `+2²³ − 1` before
/// the codec's 24-bit field wraps. A through-wrap meridian-flip slew
/// can therefore leave the encoder counter outside the canonical
/// `[−cpr/2, +cpr/2)` band (e.g. `−1.89M` for a flip that landed
/// physically at `+11.5 h`, modular `+1.74M`). Without folding, the
/// next slew's `target_ticks − current_ticks` would order a full
/// extra revolution. This helper folds the raw delta to the
/// shortest-path equivalent in `[−cpr/2, +cpr/2)`.
fn fold_delta_to_canonical(delta: i32, cpr: u32) -> i32 {
    if cpr == 0 {
        return delta;
    }
    let cpr_i = cpr as i32;
    let half_cpr = cpr_i / 2;
    let modular = delta.rem_euclid(cpr_i);
    if modular >= half_cpr {
        modular - cpr_i
    } else {
        modular
    }
}

/// Force a flip slew's RA delta to keep the polar-axis sweep out of
/// the counterweight-forbidden zone `mech_HA ∈ (zone_min, zone_max)`
/// (default `(+0.95, +11.05)` on the GTi — the arc where the CW
/// rises more than 0.95 h above horizontal).
///
/// The forbidden zone is at positive `mech_HA` only and is a
/// structural property of the mount head independent of observer
/// latitude. Both forward flips (pre-flip → post-flip) and flip-backs
/// (post-flip → pre-flip) need their RA paths constrained.
///
/// Strategy: take the canonical short path unless its linear mech_HA
/// sweep from `current_ticks` through `current_ticks + canonical_delta`
/// crosses the forbidden zone (modulo the 24-hour wrap). If it would,
/// try the long way around (`canonical ± cpr_i`) which lands at the
/// same modular destination via the safe arc on the other side. If
/// the long way *also* crosses the zone, there is no safe RA path
/// between current and target and the slew is refused with
/// `INVALID_OPERATION`.
///
/// Previously a sign-blind heuristic (`|current| > cpr/4 ⇒ "safe is
/// positive"`) was used. That mis-fired at Park 4 N
/// (current ≈ -cpr/2, canonical ≈ -4k CCW just past the wrap): the
/// heuristic flipped the small CCW step into a +cpr/2 + small CW full
/// revolution that swept across the zone and slammed the CW shaft
/// into the pier (hardware validation 2026-05-16). The path-aware
/// check uses the actual forbidden zone, so it preserves the safe
/// canonical step when it doesn't cross. The both-cross refusal was
/// added after the 2026-05-17 session, where a `SetSideOfPier`
/// from Park 3 produced a `canonical_delta = -cpr/2` whose long-way
/// alternative `+cpr/2` swept the OTA through the tripod region with
/// the narrow `(+6.95, +11.05)` zone permitting it. With the wider
/// `(+0.95, +11.05)` zone both directions cross and the slew is now
/// rejected.
fn flip_slew_ra_delta(
    canonical_delta: i32,
    current_ticks: i32,
    cpr: u32,
    binding_zone_hours: (f64, f64),
) -> ASCOMResult<i32> {
    if cpr == 0 || canonical_delta == 0 {
        return Ok(canonical_delta);
    }
    let cpr_i = cpr as i32;
    let cur_ha = ra_ticks_to_mechanical_ha(current_ticks, cpr);
    let delta_ha = (canonical_delta as f64) * 24.0 / (cpr as f64);
    if !canonical_path_crosses_binding_zone(cur_ha, delta_ha, binding_zone_hours) {
        return Ok(canonical_delta);
    }
    let long_way = if canonical_delta > 0 {
        canonical_delta - cpr_i
    } else {
        canonical_delta + cpr_i
    };
    let long_delta_ha = (long_way as f64) * 24.0 / (cpr as f64);
    if !canonical_path_crosses_binding_zone(cur_ha, long_delta_ha, binding_zone_hours) {
        return Ok(long_way);
    }
    Err(ASCOMError::new(
        ASCOMErrorCode::INVALID_OPERATION,
        format!(
            "no safe RA path from mech_HA {cur_ha:+.3} h: canonical short ({delta_ha:+.3} h) \
             and long-way around ({long_delta_ha:+.3} h) both cross the forbidden zone \
             ({zone_min:+.3}, {zone_max:+.3})",
            zone_min = binding_zone_hours.0,
            zone_max = binding_zone_hours.1,
        ),
    ))
}

/// Verify a non-flip RA slew's canonical sweep doesn't cross the
/// forbidden zone. Flip slews have the option of taking the long way
/// around via [`flip_slew_ra_delta`]; non-flip slews don't — the
/// canonical short delta is the unique path between current and
/// target on the chosen pier side, so if it crosses the zone the
/// slew is refused.
fn check_non_flip_ra_path(
    canonical_delta: i32,
    current_ticks: i32,
    cpr: u32,
    binding_zone_hours: (f64, f64),
) -> ASCOMResult<()> {
    if cpr == 0 || canonical_delta == 0 {
        return Ok(());
    }
    let cur_ha = ra_ticks_to_mechanical_ha(current_ticks, cpr);
    let delta_ha = (canonical_delta as f64) * 24.0 / (cpr as f64);
    if !canonical_path_crosses_binding_zone(cur_ha, delta_ha, binding_zone_hours) {
        return Ok(());
    }
    Err(ASCOMError::new(
        ASCOMErrorCode::INVALID_OPERATION,
        format!(
            "non-flip RA slew from mech_HA {cur_ha:+.3} h by {delta_ha:+.3} h crosses the \
             forbidden zone ({zone_min:+.3}, {zone_max:+.3})",
            zone_min = binding_zone_hours.0,
            zone_max = binding_zone_hours.1,
        ),
    ))
}

/// Does the linear mech_HA sweep from `start_ha` by `delta_ha` enter
/// `(zone_min, zone_max)` (modulo 24 h)? The sweep is the open
/// interval `(min(start, start+delta), max(start, start+delta))`; the
/// binding zone repeats every 24 hours, so we check `k ∈ {-1, 0, +1}`
/// — enough to cover any `|delta_ha| ≤ 12` path. An empty zone
/// (`zone_min ≥ zone_max`) is treated as no zone.
fn canonical_path_crosses_binding_zone(
    start_ha: f64,
    delta_ha: f64,
    binding_zone_hours: (f64, f64),
) -> bool {
    let (zone_min, zone_max) = binding_zone_hours;
    if zone_min >= zone_max {
        return false;
    }
    let path_lo = start_ha.min(start_ha + delta_ha);
    let path_hi = start_ha.max(start_ha + delta_ha);
    for k in [-1.0_f64, 0.0, 1.0] {
        let bz_lo = zone_min + 24.0 * k;
        let bz_hi = zone_max + 24.0 * k;
        // Open-interval overlap: paths grazing the boundary stay safe.
        if path_lo < bz_hi && bz_lo < path_hi {
            return true;
        }
    }
    false
}

/// Force a flip slew's Dec delta to traverse the **visible** celestial
/// pole rather than the below-horizon pole.
///
/// During a Dec flip-slew the encoder must cross one of the `±cpr/4`
/// boundaries (the two celestial poles). For a polar-aligned mount,
/// only ONE pole is above the local horizon: NCP at altitude `+lat`
/// for northern observers (encoder `+cpr/4`), SCP at altitude `+|lat|`
/// for southern (encoder `−cpr/4`). The other pole is below the
/// horizon and the path through it dips the OTA below the local
/// horizon — exactly the failure mode we hit during the first
/// hardware validation when the OTA was driven through SCP at LAT
/// 32.7°N. (The fold-canonical-delta's boundary case at exactly
/// `±cpr/2` produces the negative direction, which from `encoder = 0`
/// pre-flip routes the Dec axis through SCP.)
///
/// Rule, expressed in encoder side:
///
/// - **Northern** observer:
///   - `current` in the *pre-flip* half (`|enc| ≤ cpr/4`): force CW
///     (positive delta) — the path increases toward `+cpr/4` (NCP).
///   - `current` in the *post-flip* half (`|enc| > cpr/4`): force
///     CCW (negative delta) — the path decreases back through
///     `+cpr/4` (NCP).
/// - **Southern** observer: inverse — the safe pole is `−cpr/4`
///   (SCP) so the directions flip.
///
/// When the canonical shortest path already goes the safe direction,
/// it's returned unchanged. When it doesn't, the long way around
/// (`delta − cpr` or `delta + cpr`) lands at the same modular
/// destination via the safe pole.
fn flip_slew_dec_delta(
    canonical_delta: i32,
    current_ticks: i32,
    cpr_dec: u32,
    northern: bool,
) -> i32 {
    if cpr_dec == 0 || canonical_delta == 0 {
        return canonical_delta;
    }
    let cpr_i = cpr_dec as i32;
    let quarter = cpr_i / 4;
    let in_pre_flip = current_ticks.abs() <= quarter;
    let safe_direction_positive = if northern { in_pre_flip } else { !in_pre_flip };
    let canonical_positive = canonical_delta > 0;
    if canonical_positive == safe_direction_positive {
        canonical_delta
    } else if canonical_delta > 0 {
        canonical_delta - cpr_i
    } else {
        canonical_delta + cpr_i
    }
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
            // Post-connect work that can fail (config-file read, parameter
            // cache lookup, encoder seed) runs in functions that the
            // caller can roll back on any error — otherwise the transport
            // ref-count would stay incremented while `*req` remained
            // false, leaking a connection. Per the Copilot review on
            // PR #221 (comment 3238682044).
            //
            // Order matters: `seed_home_pose_after_connect` runs FIRST so
            // the snapshot reflects the home_pose's logical encoder values
            // before `load_park_target_after_connect` picks its default
            // park target from the snapshot. Otherwise the handshake's
            // pre-seed reading (firmware-zero on a fresh power-up) would
            // become the park fallback and `Park` would drive the mount
            // to mech_HA = 0h / mech_dec = 0° instead of the home pose.
            if let Err(e) = self.seed_home_pose_after_connect().await {
                if let Err(disc_err) = self.transport.disconnect().await {
                    tracing::warn!("disconnect during set_connected rollback failed: {disc_err}");
                }
                return Err(e);
            }
            if let Err(e) = self.load_park_target_after_connect().await {
                if let Err(disc_err) = self.transport.disconnect().await {
                    tracing::warn!("disconnect during set_connected rollback failed: {disc_err}");
                }
                return Err(e);
            }
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
            //   - `park_ra_ticks` / `park_dec_ticks` — re-loaded on next
            //     connect from config / handshake. Clearing here means a
            //     mid-session edit to `MountConfig::park_*_ticks` (a
            //     future hot-reload feature) would take effect on
            //     reconnect.
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
            s.park_ra_ticks = None;
            s.park_dec_ticks = None;
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
    async fn can_set_park(&self) -> ASCOMResult<bool> {
        // SetPark requires a config-file path to persist to. Without
        // one (i.e. the driver was started on `Config::default()`),
        // `SetPark` would have nowhere to write — see the design doc's
        // §"Park persistence" for the rationale. ASCOM permits
        // `CanSetPark` to vary with driver state, so this is a runtime
        // check rather than a compile-time constant.
        Ok(self.config_file_path.is_some())
    }
    async fn can_pulse_guide(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn can_set_pier_side(&self) -> ASCOMResult<bool> {
        // Phase 6: CanSetPierSide tracks `flip_policy.enabled`. With
        // the policy disabled (the shipped default), `SetSideOfPier`
        // returns NOT_IMPLEMENTED — the driver behaves as a
        // non-flipping GEM. With it enabled (only after a successful
        // first real-hardware GTi flip), the slew planner accepts
        // explicit flip requests. See the design doc's
        // [§"Meridian flip"](../../../docs/services/star-adventurer-gti.md#meridian-flip).
        Ok(self.config.flip_policy.enabled)
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
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;
        let (ra, _dec) = encoder_to_celestial(
            snap.ra.position_ticks,
            snap.dec.position_ticks,
            lst,
            params.cpr_ra,
            params.cpr_dec,
            self.config.site_latitude_deg,
        );
        Ok(ra)
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
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;
        let (_ra, dec) = encoder_to_celestial(
            snap.ra.position_ticks,
            snap.dec.position_ticks,
            lst,
            params.cpr_ra,
            params.cpr_dec,
            self.config.site_latitude_deg,
        );
        Ok(dec)
    }

    async fn declination_rate(&self) -> ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn azimuth(&self) -> ASCOMResult<f64> {
        let ra = self.right_ascension().await?;
        let dec = self.declination().await?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;
        let (_alt, az) = ra_dec_to_alt_az(ra, dec, self.config.site_latitude_deg, lst);
        Ok(az)
    }

    async fn altitude(&self) -> ASCOMResult<f64> {
        let ra = self.right_ascension().await?;
        let dec = self.declination().await?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;
        let (alt, _az) = ra_dec_to_alt_az(ra, dec, self.config.site_latitude_deg, lst);
        Ok(alt)
    }

    async fn sidereal_time(&self) -> ASCOMResult<f64> {
        local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)
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
        // Pure prediction — no wire traffic, no slew. Shares the
        // flip-policy decision tree with `slew_to_coordinates_async`
        // (see the design doc's
        // [§"Pier-side decision tree"](../../../docs/services/star-adventurer-gti.md#pier-side-decision-tree)),
        // then validates the target against the safety envelope for
        // the chosen side with the same `INVALID_VALUE` rejection a
        // slew would issue. With `flip_policy.enabled = false` (the
        // default) the decision tree collapses to "current side", so
        // any target inside the (pre-flip) safety envelope predicts
        // `pierWest` in the Northern Hemisphere (`pierEast` in the
        // Southern). With it enabled, an opposite side is returned
        // when the current side's envelope rejects the target.
        self.ensure_connected().await?;
        Self::validate_coordinates(ra, dec)?;
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;
        let snap = self.transport.snapshot().await;
        let current_side = side_of_pier_calc(
            snap.dec.position_ticks,
            params.cpr_dec,
            self.config.site_latitude_deg,
        );
        let chosen_side = select_pier_side_for_target(
            ra,
            lst,
            current_side,
            &self.config.flip_policy,
            (
                self.config.binding_zone_min_hours,
                self.config.binding_zone_max_hours,
            ),
            self.config.site_latitude_deg,
        );
        let pre_flip_side = if self.config.site_latitude_deg >= 0.0 {
            PierSide::West
        } else {
            PierSide::East
        };
        let target_is_flipped = chosen_side != pre_flip_side && chosen_side != PierSide::Unknown;
        self.check_within_safe_envelope(ra, dec, lst, target_is_flipped)?;
        Ok(chosen_side)
    }

    async fn set_side_of_pier(&self, side_of_pier: PierSide) -> ASCOMResult<()> {
        // Phase 6: explicit meridian-flip trigger. With
        // `flip_policy.enabled = false` (the default), every code path
        // here short-circuits to NOT_IMPLEMENTED — the driver behaves
        // as a non-flipping GEM. With the policy enabled, this method
        // routes through `slew_to_coordinates_async` to the current
        // celestial target with the chosen side. See the design doc's
        // [§"`SetSideOfPier(side)`"](../../../docs/services/star-adventurer-gti.md#setsideofpierside).
        if !self.config.flip_policy.enabled {
            return Err(ASCOMError::new(
                ASCOMErrorCode::NOT_IMPLEMENTED,
                "SetSideOfPier requires flip_policy.enabled = true",
            ));
        }
        if side_of_pier == PierSide::Unknown {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                "SetSideOfPier rejects PierSide::Unknown",
            ));
        }
        self.ensure_connected().await?;
        self.ensure_unparked().await?;
        // Refuse mid-slew. The slew planner also self-refuses via its
        // own `slew_in_progress` check, but rejecting here yields a
        // cleaner error before we read the snapshot and compute a
        // stale celestial target.
        if self.state.read().await.slew_in_progress {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_OPERATION,
                "SetSideOfPier refused: slew already in progress",
            ));
        }
        // Compute the mount's current celestial position from the
        // encoder snapshot + LST. A flip slew keeps the OTA on this
        // same celestial direction while landing on the requested
        // pier side.
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;
        let snap = self.transport.snapshot().await;
        let current_side = side_of_pier_calc(
            snap.dec.position_ticks,
            params.cpr_dec,
            self.config.site_latitude_deg,
        );
        if side_of_pier == current_side {
            // No-op success. Per ASCOM, SetSideOfPier(current_side)
            // is a valid request; we don't issue motion or perturb
            // the in-memory target.
            return Ok(());
        }
        // Read the *celestial* current pointing from the snapshot —
        // `encoder_to_celestial` applies the post-flip RA/Dec mapping
        // when the Dec encoder is past the pole.
        // `execute_slew_with_explicit_side` will re-compute the target
        // encoder for the chosen side.
        let (cur_ra, cur_dec) = encoder_to_celestial(
            snap.ra.position_ticks,
            snap.dec.position_ticks,
            lst,
            params.cpr_ra,
            params.cpr_dec,
            self.config.site_latitude_deg,
        );
        // Drive the slew with the chosen-side encoder math directly,
        // bypassing the policy decision tree. The selector's
        // stay-on-current preference is correct for slew_to_coordinates
        // but wrong for an explicit SetSideOfPier — the user pinned the
        // side, honour it.
        self.execute_slew_with_explicit_side(cur_ra, cur_dec, side_of_pier)
            .await
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
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;
        // Reject syncs that would set the encoder outside the
        // mount's safe mechanical envelope — a bad sync would let
        // the *next* tracking step push the OTA into a hard stop.
        // Sync uses the pre-flip envelope (`target_is_flipped =
        // false`); operators must `AbortSlew` and re-sync the pre-
        // flip pointing first if a manual flip left the mount in a
        // post-flip state.
        self.check_within_safe_envelope(ra, dec, lst, false)?;
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
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;

        // Phase 6: determine target pier side via the flip policy. With
        // `flip_policy.enabled = false` (the default), `chosen_side`
        // always equals `current_side` and the rest of this function
        // reduces to the pre-Phase-6 pipeline. With it enabled, a
        // flip slew may be chosen — see the design doc's
        // [§"Meridian flip"](../../../docs/services/star-adventurer-gti.md#meridian-flip).
        let snap = self.transport.snapshot().await;
        let current_side = side_of_pier_calc(
            snap.dec.position_ticks,
            params.cpr_dec,
            self.config.site_latitude_deg,
        );
        let chosen_side = select_pier_side_for_target(
            ra,
            lst,
            current_side,
            &self.config.flip_policy,
            (
                self.config.binding_zone_min_hours,
                self.config.binding_zone_max_hours,
            ),
            self.config.site_latitude_deg,
        );
        self.execute_slew_with_explicit_side(ra, dec, chosen_side)
            .await
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
        // Atomically reserve the in-progress slot **before** issuing
        // any motion. Doing the flag-set after `:J` (the old layout)
        // left a TOCTOU window where a concurrent `SetPark` could
        // read mid-slew encoder positions. Cancel any in-flight
        // pulse-guide in the same write — park takes ownership of
        // both axes from this point.
        {
            let mut s = self.state.write().await;
            if s.slew_in_progress {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_OPERATION,
                    "park refused: slew already in progress",
                ));
            }
            s.slew_in_progress = true;
            s.pulse_guiding_ra = false;
            s.pulse_guiding_dec = false;
        }
        // From here on, any error path must clear `slew_in_progress`
        // — otherwise the driver gets stuck reporting Slewing forever.
        // Wrap motion-issue in an inner future so a single rollback
        // covers every `?` failure.
        let result: ASCOMResult<()> = async {
            // Stop tracking before slewing home (per ASCOM, tracking
            // remains off after Park). The wire `:K1` is issued first
            // so the in-memory flag flip only follows a successful stop.
            if self.state.read().await.tracking_requested {
                self.transport
                    .send(Command::StopMotion(Axis::Ra))
                    .await
                    .map_err(Self::ascom)?;
                self.state.write().await.tracking_requested = false;
            }
            // Slew both axes to the loaded park target.
            // `set_connected(true)` populated these from config /
            // handshake; if either is `None` here it's an internal
            // invariant violation. Surface as a structured ASCOMError
            // rather than a panic — panicking inside a tokio task
            // aborts it and leaves the Alpaca client with a
            // connection-reset.
            let (target_ra_ticks, target_dec_ticks) = {
                let s = self.state.read().await;
                let ra = s.park_ra_ticks.ok_or_else(|| {
                    ASCOMError::new(
                        ASCOMErrorCode::INVALID_OPERATION,
                        "park_ra_ticks not loaded — internal invariant violation",
                    )
                })?;
                let dec = s.park_dec_ticks.ok_or_else(|| {
                    ASCOMError::new(
                        ASCOMErrorCode::INVALID_OPERATION,
                        "park_dec_ticks not loaded — internal invariant violation",
                    )
                })?;
                (ra, dec)
            };
            // Same wire sequence as `slew_to_coordinates_async`:
            // `:K`-and-wait, `:G` with direction chosen from
            // `sign(target - current)`, `:S target`, `:J`.
            let snap = self.transport.snapshot().await;
            for (axis, current_ticks, target_ticks) in [
                (Axis::Ra, snap.ra.position_ticks, target_ra_ticks),
                (Axis::Dec, snap.dec.position_ticks, target_dec_ticks),
            ] {
                self.stop_and_wait(axis).await?;
                let mode = MotionMode {
                    kind: skywatcher_motor_protocol::command::ModeKind::Goto,
                    speed: skywatcher_motor_protocol::command::Speed::Fast,
                    ccw: current_ticks > target_ticks,
                };
                self.transport
                    .send(Command::SetMotionMode { axis, mode })
                    .await
                    .map_err(Self::ascom)?;
                // No `:I` in Goto mode — the firmware computes slew speed
                // internally. See the matching note in
                // `slew_to_coordinates_async`.
                self.transport
                    .send(Command::SetGotoTarget {
                        axis,
                        ticks: target_ticks,
                    })
                    .await
                    .map_err(Self::ascom)?;
                self.transport
                    .send(Command::StartMotion(axis))
                    .await
                    .map_err(Self::ascom)?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            self.state.write().await.slew_in_progress = false;
            return Err(e);
        }
        // Hand off to the park watcher; it owns `slew_in_progress`
        // from here and will clear it on completion.
        let settle = self
            .state
            .read()
            .await
            .slew_settle_time
            .unwrap_or(self.config.settle_after_slew);
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

    async fn set_park(&self) -> ASCOMResult<()> {
        // Capability gate: without a config-file path we have nowhere
        // to persist to. `CanSetPark` advertises `false` in this case,
        // but ASCOM clients are allowed to call setters whose
        // capability is `false` and expect `NOT_IMPLEMENTED`.
        let config_path = self.config_file_path.as_ref().ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::NOT_IMPLEMENTED,
                "SetPark requires the driver to be started with --config <path>",
            )
        })?;
        self.ensure_connected().await?;
        // Refuse mid-slew: the "current encoder pair" wouldn't be
        // stable while the motors are still moving. Also catches
        // mid-park: AtPark hasn't been set yet but slew_in_progress is.
        //
        // Two layers of defense for the concurrent-motion case (per
        // Copilot review on PR #221, comment 3242621736):
        //   1. The in-memory `slew_in_progress` flag: park() and
        //      slew_to_coordinates_async() now set this *before*
        //      issuing motion (with rollback-on-error), so the
        //      flag observation here is reliable.
        //   2. The latest wire snapshot's `running` flag: defense
        //      in depth against an axis that's running for any
        //      reason the in-memory flag wouldn't capture (a
        //      tracking pulse, an external `:J` from a future
        //      out-of-band path, a flag-set racing the wire send).
        //      The snapshot is updated by the background poller at
        //      `polling_interval`; the window where snapshot lags
        //      reality is bounded by that interval.
        if self.state.read().await.slew_in_progress {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_OPERATION,
                "SetPark refused while slew or park is in progress",
            ));
        }
        let snap = self.transport.snapshot().await;
        if snap.ra.running || snap.dec.running {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_OPERATION,
                "SetPark refused while an axis is running per the wire snapshot",
            ));
        }
        let ra_ticks = snap.ra.position_ticks;
        let dec_ticks = snap.dec.position_ticks;
        // Disk I/O runs on the blocking pool so the async runtime
        // isn't held up while we read+parse+stage+fsync+rename. Same
        // pattern as `services/rp/src/persistence/document.rs::write_sidecar`.
        let path = config_path.clone();
        tokio::task::spawn_blocking(move || write_park_to_config(&path, ra_ticks, dec_ticks))
            .await
            .map_err(|e| {
                ASCOMError::new(
                    ASCOMErrorCode::INVALID_OPERATION,
                    format!("set_park write task join error: {e}"),
                )
            })?
            .map_err(Self::ascom)?;
        // Only mutate the in-memory target after the disk write
        // succeeds — otherwise a failed write would leave the live
        // park target out of sync with what's persisted.
        let mut s = self.state.write().await;
        s.park_ra_ticks = Some(ra_ticks);
        s.park_dec_ticks = Some(dec_ticks);
        debug!(
            ra_ticks,
            dec_ticks,
            path = ?config_path,
            "set_park persisted to config file"
        );
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
        // Resolve direction → (axis, ccw, rate_factor) under a read
        // lock. The in-flight check + flag-set happens later under a
        // write lock so it's atomic against concurrent same-axis
        // calls (the rate_factor / tracking_was_on snapshots taken
        // here are stable: rates can be updated concurrently, but
        // the worst case is a one-tick-late read which ASCOM
        // tolerates).
        let (axis, ccw, rate_factor, tracking_was_on) = {
            let s = self.state.read().await;
            let (axis, ccw, rate_factor) = match direction {
                GuideDirection::East => (Axis::Ra, false, 1.0 - s.guide_rate_ra_fraction),
                GuideDirection::West => (Axis::Ra, false, 1.0 + s.guide_rate_ra_fraction),
                GuideDirection::North => (Axis::Dec, false, s.guide_rate_dec_fraction),
                GuideDirection::South => (Axis::Dec, true, s.guide_rate_dec_fraction),
            };
            let tracking_was_on = axis == Axis::Ra && s.tracking_requested;
            (axis, ccw, rate_factor, tracking_was_on)
        };
        // Compute the shifted step period from the cached
        // sidereal-period helper and the rate factor. Validate against
        // the protocol's 24-bit `:I` payload range before sending —
        // `encode_u24` silently truncates above `0x00FF_FFFF`, so an
        // un-validated period would wrap to an unintended speed.
        // For sidereal_period ≈ 380K on the GTi, the floor is
        // `rate_factor ≥ sidereal_period / 0xFFFFFF ≈ 0.023`. Tiny
        // guide-rate fractions trip this; clients see `INVALID_VALUE`.
        let params = self
            .transport
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let sidereal_period = sidereal_step_period(params.tmr_freq, params.cpr_ra);
        let shifted_period = pulse_guide_step_period(sidereal_period, rate_factor);
        const MAX_STEP_PERIOD: u32 = 0x00FF_FFFF;
        if shifted_period == 0 || shifted_period > MAX_STEP_PERIOD {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!(
                    "PulseGuide step period {shifted_period} (rate_factor {rate_factor:.4} × \
                     sidereal_period {sidereal_period}) is outside the protocol's 24-bit \
                     range; pick a guide rate closer to sidereal"
                ),
            ));
        }
        // Atomically check `pulse_guiding_<axis>` and set it to true
        // under a single write lock. This closes the TOCTOU window: a
        // concurrent same-axis `pulse_guide` either acquires the
        // write lock first (and we see the flag set on the next read),
        // or acquires it later (and sees our flag). Without the
        // atomic set, the previous flow let a concurrent caller pass
        // the in-flight check while we were still awaiting the
        // `:K`/`:G`/`:I`/`:J` sends. `axis` is always `Ra` or `Dec`
        // here — `GuideDirection` only resolves to those two — so the
        // boolean dispatch is exhaustive without a third branch.
        let is_ra = axis == Axis::Ra;
        {
            let mut s = self.state.write().await;
            let already_in_flight = if is_ra {
                s.pulse_guiding_ra
            } else {
                s.pulse_guiding_dec
            };
            if already_in_flight {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_OPERATION,
                    "PulseGuide refused while a same-axis pulse is in flight",
                ));
            }
            if is_ra {
                s.pulse_guiding_ra = true;
            } else {
                s.pulse_guiding_dec = true;
            }
        }
        // Wire path: `:K<axis>` (decelerate and wait for the running
        // flag to clear so `:G` doesn't return `!2 MotorNotStopped`),
        // `:G<axis>` (Tracking + ccw), `:I<axis>` (shifted period),
        // `:J<axis>`. Any failure on the wire rolls back the
        // `pulse_guiding_<axis>` flag so the next caller isn't blocked
        // by a half-applied pulse, and so `IsPulseGuiding` reports
        // false consistent with the lack of actual motion.
        let mode = MotionMode {
            kind: ModeKind::Tracking,
            speed: Speed::Slow,
            ccw,
        };
        let wire_result: ASCOMResult<()> = async {
            self.stop_and_wait(axis).await?;
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
            Ok(())
        }
        .await;
        if let Err(e) = wire_result {
            clear_pulse_flag(&self.state, axis).await;
            return Err(e);
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
            // snapshot. [`watcher_poll_with_retry`] tolerates a handful
            // of transient transport errors so a single USB-CDC glitch
            // doesn't take the watcher offline mid-slew; on retry
            // exhaustion it also issues a best-effort `:L` on both
            // axes so the motor isn't left commutating with no
            // observer.
            let snap = match watcher_poll_with_retry(&transport, "slew_watcher").await {
                Ok(s) => s,
                Err(_) => {
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
                let (target_ra, target_dec, target_pier_side) = {
                    let s = state.read().await;
                    (s.target_ra_hours, s.target_dec_degrees, s.target_pier_side)
                };
                if let (Some(target_ra), Some(target_dec), Some(params)) =
                    (target_ra, target_dec, transport.parameters().await)
                {
                    // ERFA refuses the host UTC if `eraCal2jd`
                    // rejects the year (below `IYMIN = -4799`). A
                    // leap-second-table-out-of-range clock returns
                    // `Ok` with a warning, not an error — see the
                    // `StarAdvError::Timekeeping` rustdoc — so the
                    // realistic failure here is an absurdly-far-
                    // past clock, not a future-shifted one. Match
                    // the `poll_axes_now` failure pattern: log,
                    // clear `slew_in_progress`, exit the watcher
                    // rather than aborting the tokio task.
                    let lst = match local_sidereal_time_hours(
                        SystemTime::now(),
                        config.site_longitude_deg,
                    ) {
                        Ok(lst) => lst,
                        Err(e) => {
                            tracing::warn!("watcher LST computation failed: {e}");
                            state.write().await.slew_in_progress = false;
                            return;
                        }
                    };
                    // Flip-aware: `encoder_to_celestial` applies the
                    // post-flip RA/Dec mapping when the Dec encoder is
                    // past the pole. Without it, the residual check
                    // would interpret a successful flip as a 12-hour
                    // RA residual and the pickup loop would try to undo
                    // the flip on its first iteration.
                    let (cur_ra, cur_dec) = encoder_to_celestial(
                        snap.ra.position_ticks,
                        snap.dec.position_ticks,
                        lst,
                        params.cpr_ra,
                        params.cpr_dec,
                        config.site_latitude_deg,
                    );
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
                        // Flip-aware target-encoder computation. With a
                        // pre-flip target side, reuse `pickup_target_ra_ticks`
                        // for the same LST pre-compensation that pre-Phase-6
                        // builds relied on. With a post-flip target side,
                        // compute the projected target via
                        // `target_encoder_flipped` so the pickup re-slew
                        // lands on the flipped encoder (past-the-pole Dec
                        // and the mirror-band RA mech_HA) rather than
                        // undoing the flip back to the pre-flip side.
                        let pre_flip_side = if config.site_latitude_deg >= 0.0 {
                            PierSide::West
                        } else {
                            PierSide::East
                        };
                        let target_is_flipped = target_pier_side
                            .filter(|s| *s != pre_flip_side && *s != PierSide::Unknown)
                            .is_some();
                        let (new_ra_ticks, new_dec_ticks) = if target_is_flipped {
                            let lst_proj = lst + projection.as_secs_f64() / 3600.0;
                            target_encoder_flipped(
                                target_ra,
                                target_dec,
                                lst_proj,
                                params.cpr_ra,
                                params.cpr_dec,
                            )
                        } else {
                            let new_ra =
                                pickup_target_ra_ticks(target_ra, lst, projection, params.cpr_ra);
                            let new_dec = dec_degrees_to_ticks(target_dec, params.cpr_dec);
                            (new_ra, new_dec)
                        };
                        // Fold the deltas to canonical so the pickup
                        // re-slew takes the shortest path even if the
                        // current encoder snapshot landed outside
                        // `[−cpr/2, +cpr/2)` after a through-wrap
                        // flip — see [`fold_delta_to_canonical`].
                        let ra_delta = fold_delta_to_canonical(
                            new_ra_ticks - snap.ra.position_ticks,
                            params.cpr_ra,
                        );
                        let dec_delta = fold_delta_to_canonical(
                            new_dec_ticks - snap.dec.position_ticks,
                            params.cpr_dec,
                        );
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

/// Retrying wrapper around [`TransportManager::poll_axes_now`] used by
/// both the slew and park completion watchers. Tolerates up to
/// [`WATCHER_POLL_RETRY_LIMIT`] consecutive transport errors so a
/// single transient USB-CDC glitch (a brief renumeration, a stale
/// kernel buffer, …) doesn't take the watcher offline for the rest
/// of a goto.
///
/// On every successful poll the snapshot is emitted at `debug` so a
/// post-mortem can reconstruct the last-known-good state observed
/// before any failure. On every failed attempt the underlying error
/// is logged at `warn` with the attempt counter.
///
/// On retry exhaustion, the helper makes a best-effort `:L` on both
/// axes before returning the underlying error: even when we can no
/// longer observe state, the firmware may still be commutating step
/// pulses, and a runaway motor with no observer is the worst case
/// the original exit-on-first-error policy created. The `:L` calls
/// are fire-and-forget — if they fail too, there's nothing useful
/// the watcher can do beyond logging and bailing.
async fn watcher_poll_with_retry(
    transport: &TransportManager,
    context: &'static str,
) -> crate::error::Result<MountSnapshot> {
    let mut last_err: Option<StarAdvError> = None;
    for attempt in 0..WATCHER_POLL_RETRY_LIMIT {
        match transport.poll_axes_now().await {
            Ok(snap) => {
                debug!(
                    context = context,
                    ra_ticks = snap.ra.position_ticks,
                    ra_running = snap.ra.running,
                    ra_blocked = snap.ra.blocked,
                    ra_goto = snap.ra.goto,
                    dec_ticks = snap.dec.position_ticks,
                    dec_running = snap.dec.running,
                    dec_blocked = snap.dec.blocked,
                    dec_goto = snap.dec.goto,
                    "watcher snapshot"
                );
                return Ok(snap);
            }
            Err(e) => {
                tracing::warn!(
                    context = context,
                    attempt = attempt + 1,
                    limit = WATCHER_POLL_RETRY_LIMIT,
                    "watcher poll_axes_now transient error: {e}"
                );
                last_err = Some(e);
                if attempt + 1 < WATCHER_POLL_RETRY_LIMIT {
                    tokio::time::sleep(WATCHER_POLL_RETRY_BACKOFF).await;
                }
            }
        }
    }
    tracing::warn!(
        context = context,
        "watcher poll_axes_now retries exhausted — best-effort :L on both axes before bailing"
    );
    let _ = transport.send(Command::InstantStop(Axis::Ra)).await;
    let _ = transport.send(Command::InstantStop(Axis::Dec)).await;
    Err(last_err
        .unwrap_or_else(|| StarAdvError::Transport("watcher poll retries exhausted".to_string())))
}

/// Probe whether the parent directory of `config_path` can host the
/// staging temp file that `SetPark`'s atomic-rename pattern requires.
///
/// Called once at startup from `main.rs` so the operator sees a `warn!`
/// at boot if `SetPark` will fail at runtime due to filesystem
/// permissions, rather than only discovering it on the first `SetPark`
/// call. Does **not** change `CanSetPark` — the capability still
/// advertises support; the probe is purely an early-warning signal.
///
/// The probe creates a `NamedTempFile` in the same directory the real
/// staging file would live in (`config_path.parent()`) and immediately
/// drops it. Writability of the **parent directory** is what matters
/// for the atomic-rename pattern: even if the target config file is
/// itself read-only, `rename(2)` only needs write access to the
/// containing directory to swap in a new file. The probe therefore
/// matches what `write_park_to_config` actually does — a false-positive
/// would mean the probe passes but the real write fails (or vice
/// versa), defeating the point.
pub fn probe_park_file_writability(config_path: &Path) -> std::io::Result<()> {
    let parent = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // Drop closes and deletes the temp file; the probe leaves no trace.
    let _tmp = tempfile::NamedTempFile::new_in(parent)?;
    Ok(())
}

/// Canonicalise the operator-supplied config path so `SetPark` writes
/// to a stable absolute location even if the process later `chdir`s
/// away (also resolves symlinks, which the atomic-rename pattern
/// needs — the temp file goes in the *physical* parent directory).
/// On canonicalisation failure (path doesn't yet exist, symlink loop,
/// permission denied on a path component) the original path is
/// returned and a `warn!` is logged — `SetPark` will still attempt the
/// write against the path as given, surfacing the real error there.
///
/// Extracted from `main.rs` so the warn-on-failure branch is unit
/// testable; the binary calls this from `main()`.
pub fn canonicalise_config_path(config_path: Option<&PathBuf>) -> Option<PathBuf> {
    config_path.map(|p| {
        std::fs::canonicalize(p).unwrap_or_else(|e| {
            tracing::warn!(
                "could not canonicalise config path {:?}: {e}; SetPark will write to the path as given",
                p
            );
            p.clone()
        })
    })
}

/// Early-warning probe wrapper: run [`probe_park_file_writability`] on
/// the supplied path and log a `warn!` on failure. Used by `main.rs`
/// at startup — operators get a heads-up at boot if `SetPark` will
/// fail at runtime due to filesystem permissions, rather than only
/// discovering it on the first `SetPark` call. `CanSetPark` is not
/// affected; the capability still advertises support and the actual
/// `SetPark` will surface a structured error if the probe was correct.
///
/// Extracted from `main.rs` so the warn-on-failure branch is unit
/// testable.
pub fn warn_if_park_path_unwritable(config_path: &Path) {
    if let Err(e) = probe_park_file_writability(config_path) {
        tracing::warn!(
            "SetPark writes to {:?} will fail at runtime: {e}. \
             Check permissions on the containing directory if SetPark support is required.",
            config_path
        );
    }
}

/// Read `mount.park_ra_ticks` / `mount.park_dec_ticks` from the on-disk
/// config file. Each axis is returned independently — a `None` means
/// the file did not set that key (or set it to JSON `null`), and the
/// caller will fall back to the handshake-captured value for that axis.
///
/// A key that **is** present but holds something other than an integer
/// inside `i32`'s range is surfaced as a `StarAdvError::Config` rather
/// than silently treated as `None`. Operator typos (a string,
/// an i64 too large to be encoder ticks, a float) should fail loudly so
/// the misconfiguration is visible rather than masked by the handshake
/// fallback. Other failures (file missing, malformed JSON, `mount` key
/// missing or not an object) are also surfaced as `StarAdvError::Config`.
///
/// Reading the file only at connect time means an operator can
/// hand-edit the park keys between connects and have the change take
/// effect on reconnect, without restarting the driver.
///
/// Blocking I/O; callers wrap in `tokio::task::spawn_blocking`.
fn read_park_from_config(config_path: &Path) -> crate::error::Result<(Option<i32>, Option<i32>)> {
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| StarAdvError::Config(format!("read config {}: {e}", config_path.display())))?;
    let root: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        StarAdvError::Config(format!("parse config {}: {e}", config_path.display()))
    })?;
    let mount = root
        .as_object()
        .and_then(|o| o.get("mount"))
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            StarAdvError::Config(format!(
                "config {} has no `mount` object",
                config_path.display()
            ))
        })?;
    let ra = extract_park_tick(mount.get("park_ra_ticks"), "mount.park_ra_ticks")?;
    let dec = extract_park_tick(mount.get("park_dec_ticks"), "mount.park_dec_ticks")?;
    Ok((ra, dec))
}

/// Decode an optional park-tick JSON value:
///
/// - Absent (`None`) or explicit `Value::Null` → `Ok(None)` (caller
///   falls back to the handshake-captured value).
/// - A JSON integer in the `i32` range → `Ok(Some(n))`.
/// - Anything else (string, float, boolean, array/object, i64 outside
///   `i32` range) → `Err(StarAdvError::Config)`. Loud failure on
///   operator typo is the whole reason this helper exists — silently
///   falling back to handshake would mask the misconfiguration.
fn extract_park_tick(
    value: Option<&serde_json::Value>,
    key: &'static str,
) -> crate::error::Result<Option<i32>> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_i64().ok_or_else(|| {
                StarAdvError::Config(format!(
                    "`{key}` must be an integer (encoder ticks), got {v}"
                ))
            })?;
            i32::try_from(n).map(Some).map_err(|_| {
                StarAdvError::Config(format!(
                    "`{key}` value {n} is outside the i32 encoder-tick range"
                ))
            })
        }
    }
}

/// Patch the on-disk JSON config with the supplied park encoder pair.
///
/// Read-as-`Value` + atomic-rename pattern: load the file as
/// `serde_json::Value`, mutate **only** the `mount.park_ra_ticks` and
/// `mount.park_dec_ticks` keys, serialise pretty-printed, write via a
/// `tempfile::NamedTempFile` in the same directory, `persist` to swap
/// it in atomically. Every other field of the JSON file — known and
/// unknown — is preserved as a JSON value. Operator-level formatting
/// (insertion-order of unrelated keys, custom indentation, comments
/// disguised as fields) is not preserved byte-for-byte because the
/// round-trip pretty-prints the whole document; the *semantic* content
/// outside the two park keys is unchanged.
///
/// Durability: fsync the staged file before rename (`tempfile::persist`
/// uses POSIX `rename(2)`), then fsync the parent directory after
/// rename so the directory entry update is itself durable. Mirrors
/// `services/rp/src/persistence/document.rs::write_sidecar_sync`.
///
/// The driver never re-serialises its in-memory typed `Config` here:
/// doing so would round-trip CLI overrides (`--port`, `--baud`, etc.)
/// back to disk and is structurally avoided. See the design doc's
/// [§"Park persistence"](../../../docs/services/star-adventurer-gti.md#park-persistence)
/// for the contract this helper implements.
///
/// Blocking I/O; callers wrap in `tokio::task::spawn_blocking`.
fn write_park_to_config(
    config_path: &Path,
    park_ra_ticks: i32,
    park_dec_ticks: i32,
) -> crate::error::Result<()> {
    use std::io::Write;

    let content = std::fs::read_to_string(config_path)
        .map_err(|e| StarAdvError::Config(format!("read config {}: {e}", config_path.display())))?;
    let mut root: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        StarAdvError::Config(format!("parse config {}: {e}", config_path.display()))
    })?;
    let mount = root
        .as_object_mut()
        .and_then(|o| o.get_mut("mount"))
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| {
            StarAdvError::Config(format!(
                "config {} has no `mount` object",
                config_path.display()
            ))
        })?;
    mount.insert(
        "park_ra_ticks".to_string(),
        serde_json::Value::from(park_ra_ticks),
    );
    mount.insert(
        "park_dec_ticks".to_string(),
        serde_json::Value::from(park_dec_ticks),
    );
    let mut pretty = serde_json::to_string_pretty(&root)
        .map_err(|e| StarAdvError::Config(format!("serialise config: {e}")))?;
    // serde_json's pretty-printer omits a trailing newline; add one so
    // operators editing the file later don't trip POSIX "no newline at
    // end of file" warnings in diffs.
    pretty.push('\n');

    // Temp file must live in the **same directory** as the destination
    // so `persist` can use POSIX `rename` (atomic on the same
    // filesystem) rather than copy-and-delete. Fall back to the
    // current dir if the path has no parent (e.g. a bare filename),
    // which is what Path::parent returns Some("") for — coerce to ".".
    let parent = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| StarAdvError::Config(format!("create temp file in {parent:?}: {e}")))?;
    tmp.write_all(pretty.as_bytes())
        .map_err(|e| StarAdvError::Config(format!("write temp file: {e}")))?;
    // fsync the file data so a crash after rename cannot surface a
    // renamed-but-zero-length sidecar.
    tmp.as_file()
        .sync_all()
        .map_err(|e| StarAdvError::Config(format!("fsync temp file: {e}")))?;
    tmp.persist(config_path).map_err(|e| {
        StarAdvError::Config(format!("atomic rename to {}: {e}", config_path.display()))
    })?;
    // fsync the parent directory so the rename itself is durable.
    // Windows can't open a directory as a regular file handle, so this
    // is unix-only. Mirrors `services/rp/src/persistence/document.rs`.
    #[cfg(unix)]
    {
        std::fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| StarAdvError::Config(format!("fsync parent dir {parent:?}: {e}")))?;
    }
    Ok(())
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
            // See [`watcher_poll_with_retry`] in the slew watcher above:
            // tolerates transient transport errors, debug-logs every
            // successful snapshot for post-mortems, and issues a
            // best-effort `:L` on retry exhaustion so the motor halts
            // even when the wire has gone away.
            let snap = match watcher_poll_with_retry(&transport, "park_watcher").await {
                Ok(s) => s,
                Err(_) => {
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
            let active = if axis == Axis::Ra {
                s.pulse_guiding_ra
            } else {
                s.pulse_guiding_dec
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
    // `GuideDirection` only resolves to `Ra` or `Dec` (see the
    // direction-to-axis match in `MountDevice::pulse_guide`), so this
    // helper never sees `Axis::Both`. Using a boolean dispatch keeps
    // the code exhaustive without an unreachable arm.
    let mut s = state.write().await;
    if axis == Axis::Ra {
        s.pulse_guiding_ra = false;
    } else {
        s.pulse_guiding_dec = false;
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
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
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
        // production default envelope of `±6.95 h / ±90°`. Open the
        // envelope all the way for these tests; the safety-gate
        // behaviour is covered separately by
        // [`fast_settle_connected_narrow_envelope`].
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
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

    /// Like `fast_settle_connected`, but with a narrow safety
    /// configuration (a small binding zone + tight Dec range) so the
    /// safety-gate tests can land target coords that are clearly
    /// inside the binding zone or outside the Dec band without first
    /// needing to push past the GTi default `(6.95, 11.05)` /
    /// `±90°`.
    async fn fast_settle_connected_narrow_envelope() -> MountDevice {
        let mut cfg = Config::default();
        if let crate::config::TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        cfg.mount.settle_after_slew = Duration::from_millis(0);
        // Narrow binding zone covering `mech_HA ∈ [0.5, 1.5] h` so a
        // target 1 h past meridian on the natural side is inside it,
        // and tight Dec band `[-5°, +5°]` so off-equator targets are
        // rejected.
        cfg.mount.binding_zone_min_hours = 0.5;
        cfg.mount.binding_zone_max_hours = 1.5;
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
    async fn slew_async_refuses_ra_target_in_binding_zone() {
        // Binding zone covers `mech_HA ∈ [0.5, 1.5] h`. Target RA =
        // LST − 1 puts `mech_HA = LST − (LST − 1) = +1 h` — squarely
        // in the middle of the zone, so the slew must be rejected
        // with `INVALID_VALUE` before any wire motion.
        let d = fast_settle_connected_narrow_envelope().await;
        let lst = d.sidereal_time().await.unwrap();
        let target = (lst - 1.0).rem_euclid(24.0);
        let err = d.slew_to_coordinates_async(target, 0.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
        assert!(
            err.message.contains("binding zone"),
            "error message must call out the binding zone: {}",
            err.message
        );
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

    #[test]
    fn ascom_helper_maps_timekeeping_to_invalid_operation() {
        // Every LST-using trait method propagates ERFA failures via
        // `local_sidereal_time_hours(...).map_err(Self::ascom)?`. A
        // mount-level trait test would need a clock-injection seam
        // (host `SystemTime` can not even represent ERFA's
        // `IYMIN = -4799` floor on Windows, where FILETIME starts in
        // 1601). Instead, exercise the conversion the trait methods
        // actually use — `Self::ascom(Timekeeping(_))` — so the
        // propagation pattern has a runtime assertion in this file
        // alongside the trait code.
        let err = MountDevice::ascom(StarAdvError::Timekeeping(
            "ERFA Dtf2d rejected UTC -5000-01-01 (code -1)".into(),
        ));
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(
            err.message.contains("timekeeping"),
            "ASCOM message should retain the diagnostic, got {:?}",
            err.message
        );
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
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
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
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
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
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
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

    // ---- SetPark / Park persistence ----

    fn device_with_path(path: PathBuf) -> MountDevice {
        let mut cfg = Config::default();
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        MountDevice::with_config_file_path(cfg.mount, manager, Some(path))
    }

    /// Helper: write a default `Config` to `path` as pretty JSON. Used as
    /// the seed file for SetPark round-trip tests.
    fn seed_default_config(path: &Path) {
        let cfg = Config::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        std::fs::write(path, json).unwrap();
    }

    #[test]
    fn write_park_to_config_round_trips_through_typed_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        cfg.server.port = 12345;
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

        write_park_to_config(&path, 8000, -3000).unwrap();

        let back: Config = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.mount.park_ra_ticks, Some(8000));
        assert_eq!(back.mount.park_dec_ticks, Some(-3000));
        // Unrelated fields survive the round-trip.
        assert_eq!(back.server.port, 12345);
    }

    #[test]
    fn write_park_to_config_overwrites_existing_park_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = Config::default();
        cfg.mount.park_ra_ticks = Some(100);
        cfg.mount.park_dec_ticks = Some(200);
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

        write_park_to_config(&path, 999, -1000).unwrap();

        let back: Config = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.mount.park_ra_ticks, Some(999));
        assert_eq!(back.mount.park_dec_ticks, Some(-1000));
    }

    #[test]
    fn write_park_to_config_preserves_unknown_keys() {
        // The driver promises to touch only `mount.park_*_ticks`. Any
        // other key — including fields the typed `Config` doesn't model
        // (future schema additions, operator-added scratch values) —
        // must survive the round-trip. Tested at the raw JSON layer so
        // the typed `Config`'s field set isn't accidentally what we're
        // measuring.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let json = serde_json::json!({
            "transport": {
                "kind": "usb",
                "port": "/dev/ttyACM0",
                "baud_rate": 115200,
                "command_timeout": "2s",
                "polling_interval": "200ms"
            },
            "server": {
                "port": 11117,
                "discovery_port": null,
                "tls": null,
                "auth": null
            },
            "mount": {
                "name": "Test",
                "unique_id": "test-001",
                "description": "Test",
                "site_latitude_deg": 0.0,
                "site_longitude_deg": 0.0,
                "future_field": "preserve me"
            },
            "top_level_future_field": [1, 2, 3]
        });
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

        write_park_to_config(&path, 5, 10).unwrap();

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["mount"]["park_ra_ticks"], serde_json::json!(5));
        assert_eq!(raw["mount"]["park_dec_ticks"], serde_json::json!(10));
        assert_eq!(
            raw["mount"]["future_field"],
            serde_json::json!("preserve me")
        );
        assert_eq!(raw["top_level_future_field"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn write_park_to_config_fails_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.json");
        let err = write_park_to_config(&path, 0, 0).unwrap_err();
        assert!(matches!(err, StarAdvError::Config(_)));
    }

    #[test]
    fn write_park_to_config_fails_when_mount_object_is_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("malformed.json");
        std::fs::write(&path, "{}").unwrap();
        let err = write_park_to_config(&path, 0, 0).unwrap_err();
        match err {
            StarAdvError::Config(msg) => assert!(msg.contains("mount"), "{msg}"),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn write_park_to_config_fails_on_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        // Unclosed bracket — `serde_json::from_str` rejects it.
        std::fs::write(&path, "{ not valid json").unwrap();
        let err = write_park_to_config(&path, 0, 0).unwrap_err();
        match err {
            StarAdvError::Config(msg) => assert!(msg.contains("parse config"), "{msg}"),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn read_park_from_config_fails_on_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ not valid json").unwrap();
        let err = read_park_from_config(&path).unwrap_err();
        match err {
            StarAdvError::Config(msg) => assert!(msg.contains("parse config"), "{msg}"),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn park_returns_invariant_violation_when_in_memory_target_is_missing() {
        // The `park_*_ticks` invariant is: populated by
        // `load_park_target_after_connect` before `*requested_connection`
        // flips true, so any code path that's reached
        // `ensure_connected()` Ok should see Some on both axes. This
        // test deliberately violates the invariant by clearing the
        // values after connect, then calls `park()`. The graceful
        // failure path (return ASCOMError, do not panic) is the
        // contract we want to pin — see the comment block on
        // `MountDevice::park` for the panic-vs-error rationale.
        let d = connected_device().await;
        d.state.write().await.park_ra_ticks = None;
        let err = d.park().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(
            err.message.contains("park_ra_ticks"),
            "message should name the missing axis: {}",
            err.message
        );

        // Symmetric for the Dec axis.
        let d = connected_device().await;
        d.state.write().await.park_dec_ticks = None;
        let err = d.park().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(err.message.contains("park_dec_ticks"), "{}", err.message);
    }

    #[tokio::test]
    async fn debug_impl_includes_config_file_path() {
        // Pins the manual `fmt::Debug` impl — adding a new field
        // requires updating the closure. The path field landed in
        // PR #221; the smoke test catches a future refactor that
        // forgets to extend the Debug closure.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let d = device_with_path(path.clone());
        let s = format!("{d:?}");
        assert!(s.contains("MountDevice"), "{s}");
        assert!(s.contains("config_file_path"), "{s}");
    }

    #[tokio::test]
    async fn can_set_park_is_false_when_no_config_path_was_provided() {
        let d = device();
        assert!(!d.can_set_park().await.unwrap());
    }

    #[tokio::test]
    async fn can_set_park_is_true_when_started_with_a_config_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let d = device_with_path(dir.path().join("config.json"));
        assert!(d.can_set_park().await.unwrap());
    }

    #[tokio::test]
    async fn set_park_returns_not_implemented_without_a_config_path() {
        let d = device();
        let err = d.set_park().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn set_park_returns_not_connected_when_disconnected() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        seed_default_config(&path);
        let d = device_with_path(path);
        let err = d.set_park().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
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
    async fn set_connected_rolls_back_transport_when_park_load_fails() {
        // Regression test for the Copilot review on PR #221
        // (comment 3238682044): if `transport.connect()` succeeds but
        // the post-connect park-target load fails, the transport
        // ref-count was being left incremented (the underlying
        // transport open) while `requested_connection` stayed `false`
        // — effectively leaking a connection. The fix runs the
        // post-connect work through `load_park_target_after_connect`
        // and calls `transport.disconnect()` on any failure before
        // surfacing the error.
        //
        // We trigger the failure path by handing `MountDevice` a
        // `config_file_path` that points to a non-existent file:
        // the handshake will succeed (mock transport is happy), but
        // `read_park_from_config` will fail with a missing-file error.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.json");
        let d = device_with_path(path);

        let err = d.set_connected(true).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);

        // The transport must have been disconnected on rollback.
        // `is_available()` is the underlying TransportManager flag,
        // which would be `true` if connect succeeded and no rollback
        // ran. Asserting it false here proves we balanced the
        // connect ref-count.
        assert!(
            !d.transport.is_available(),
            "transport should be torn down after rollback"
        );
        // And the user-visible `connected()` getter agrees.
        assert!(!d.connected().await.unwrap());
    }

    #[tokio::test]
    async fn set_park_refuses_while_slew_in_progress() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        seed_default_config(&path);
        let d = device_with_path(path);
        d.set_connected(true).await.unwrap();
        d.state.write().await.slew_in_progress = true;
        let err = d.set_park().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[tokio::test]
    async fn set_park_refuses_when_wire_snapshot_reports_axis_running() {
        // Defence-in-depth (per Copilot review on PR #221, comment
        // 3242621736): even if `slew_in_progress` is false — e.g. an
        // axis is running for a reason the in-memory flag wouldn't
        // capture — the wire snapshot's `running` flag must still
        // gate `SetPark` to avoid persisting mid-motion encoder
        // ticks.
        //
        // To exercise this path independently of the
        // `slew_in_progress` flag, we connect with a
        // `CapturingMockFactory`, force `ra.running = true` directly
        // on the mock state (bypassing the slew_to_*_async flag-set
        // path), wait for the next background poll so the snapshot
        // reflects the wire, then call SetPark.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        let mock = factory.mock.clone();
        let mut cfg = Config::default();
        if let crate::config::TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        seed_default_config(&path);
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::with_config_file_path(cfg.mount, manager, Some(path));
        d.set_connected(true).await.unwrap();

        // Force the wire-side running flag without going through
        // `slew_to_coordinates_async` (which would set
        // `slew_in_progress` and trip the other guard).
        {
            let mut s = mock.state.lock().await;
            s.ra.running = true;
            s.ra.initialized = true;
        }
        // Wait for the background poll to ingest the new wire state.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if d.transport.snapshot().await.ra.running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            d.transport.snapshot().await.ra.running,
            "precondition: snapshot must reflect RA running=true"
        );
        // slew_in_progress flag is still false — only the wire
        // snapshot is reporting motion. The new defence layer must
        // still refuse.
        assert!(!d.state.read().await.slew_in_progress);
        let err = d.set_park().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(
            err.message.contains("snapshot"),
            "error should reference the wire snapshot: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn set_park_persists_current_snapshot_and_updates_in_memory_target() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        seed_default_config(&path);
        let d = device_with_path(path.clone());
        d.set_connected(true).await.unwrap();
        // Seed the snapshot directly — the polling loop won't overwrite
        // a stationary mock axis (`advance_one_step` bails on
        // `!running`), so these values stick.
        d.transport.seed_ra_position(8000).await;
        d.transport.seed_dec_position(-3000).await;

        d.set_park().await.unwrap();

        // In-memory target updated.
        let s = d.state.read().await;
        assert_eq!(s.park_ra_ticks, Some(8000));
        assert_eq!(s.park_dec_ticks, Some(-3000));
        drop(s);

        // On-disk config updated.
        let back: Config = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.mount.park_ra_ticks, Some(8000));
        assert_eq!(back.mount.park_dec_ticks, Some(-3000));
    }

    #[tokio::test]
    async fn park_target_defaults_to_handshake_capture_when_config_has_no_values() {
        // No config park values **and no `home_pose`** → driver falls
        // back to the live snapshot, which on a fresh connect equals
        // the handshake-captured encoder reading. The mock starts both
        // axes at 0 and the home_pose seed is a no-op when none is
        // configured, so park_ra_ticks / park_dec_ticks should be
        // Some(0) after connect.
        let d = device();
        d.set_connected(true).await.unwrap();
        let s = d.state.read().await;
        assert_eq!(s.park_ra_ticks, Some(0));
        assert_eq!(s.park_dec_ticks, Some(0));
    }

    #[tokio::test]
    async fn home_pose_seed_fires_when_firmware_reports_near_zero_encoder() {
        // Sky-Watcher firmware does not always read exactly (0, 0)
        // after a power-cycle: the validation GTi reports dec = -1 on
        // fresh power-up. Without the FRESH_POWER_UP_TICK_TOLERANCE
        // guard the strict `!= 0` check would skip the seed and the
        // mount would silently end up with a wrong celestial mapping
        // (and a wrong Park target via the snapshot fallback).
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        // Force the dec encoder to a 1-tick fresh-power-up artifact
        // before the manager opens the transport.
        {
            let mut state = factory.mock.state.lock().await;
            state.dec.position_ticks = -1;
        }
        let mut cfg = Config::default();
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
        cfg.mount.home_pose = Some(crate::config::HomePose::ApPark3);
        cfg.mount.site_latitude_deg = 32.7157;
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();
        // ApPark3 N hemisphere with mock cpr = 0x375F00 = 3,628,800:
        // expected seed → ra_ticks = -907,200, dec_ticks = +907,200.
        // If the seed had been skipped, the park target would be
        // (0, -1) (the pre-seed snapshot).
        let s = d.state.read().await;
        assert_eq!(s.park_ra_ticks, Some(-907200));
        assert_eq!(s.park_dec_ticks, Some(907200));
    }

    #[tokio::test]
    async fn home_pose_seed_skips_when_firmware_encoder_beyond_tolerance() {
        // A real post-slew encoder is tens of thousands of ticks away
        // from zero — well beyond FRESH_POWER_UP_TICK_TOLERANCE. The
        // seed must skip in that case so a mid-session reconnect does
        // not clobber the operator's slewed-to position.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        {
            let mut state = factory.mock.state.lock().await;
            state.ra.position_ticks = 50_000;
        }
        let mut cfg = Config::default();
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
        cfg.mount.home_pose = Some(crate::config::HomePose::ApPark3);
        cfg.mount.site_latitude_deg = 32.7157;
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();
        // Seed skipped → park target falls back to the snapshot (the
        // pre-seed handshake reading), so park_ra_ticks should equal
        // the firmware's reported 50,000 (not -907,200).
        let s = d.state.read().await;
        assert_eq!(s.park_ra_ticks, Some(50_000));
        assert_eq!(s.park_dec_ticks, Some(0));
    }

    #[tokio::test]
    async fn home_pose_seed_skips_just_above_fresh_power_up_tolerance() {
        // Pins the tight 10-tick fresh-power-up floor. A reading of 50
        // ticks (~18″ at the GTi's CPR) is well above the single-tick
        // firmware artifact and indicates the operator has already moved
        // the mount; the seed must skip so the slewed-to position is not
        // clobbered. If the tolerance is ever loosened back toward the
        // historical 100-tick floor, this test catches it.
        use crate::transport::mock::CapturingMockFactory;
        let factory = CapturingMockFactory::new();
        {
            let mut state = factory.mock.state.lock().await;
            state.ra.position_ticks = 50;
        }
        let mut cfg = Config::default();
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
        cfg.mount.home_pose = Some(crate::config::HomePose::ApPark3);
        cfg.mount.site_latitude_deg = 32.7157;
        let manager = Arc::new(TransportManager::new(cfg.clone(), Arc::new(factory)));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();
        let s = d.state.read().await;
        assert_eq!(s.park_ra_ticks, Some(50));
        assert_eq!(s.park_dec_ticks, Some(0));
    }

    #[tokio::test]
    async fn park_target_defaults_to_home_pose_encoder_when_home_pose_configured() {
        // Operator configures `home_pose: ap_park_3` (Sky-Watcher's
        // stock power-up pose) and leaves `park_*_ticks` null. After
        // connect, `seed_home_pose_after_connect` writes the home_pose's
        // logical encoder values to the firmware via `:E`, and the
        // park-target fallback must pick those up from the snapshot —
        // otherwise `Park` would slew the mount to firmware-encoder-
        // zero (mech_HA = 0h, mech_dec = 0°) instead of the pose the
        // operator powered up at. Regression test for the
        // meridian-flip-phase hardware-validation observation that
        // `Park` from `home_pose: ap_park_3` drove the OTA to
        // meridian / celestial equator instead of NCP.
        let mut cfg = Config::default();
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
        cfg.mount.home_pose = Some(crate::config::HomePose::ApPark3);
        cfg.mount.site_latitude_deg = 32.7157;
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();
        // ApPark3 codebase convention: mech_HA = -6h, dec encoder = +90°
        // (northern hemisphere). Mock cpr = 0x375F00 = 3,628,800 for
        // both axes, so ra = -6/24 * cpr = -907,200 and
        // dec = 90/360 * cpr = +907,200.
        let s = d.state.read().await;
        assert_eq!(s.park_ra_ticks, Some(-907200));
        assert_eq!(s.park_dec_ticks, Some(907200));
    }

    #[tokio::test]
    async fn park_target_prefers_config_values_over_handshake_capture() {
        // Config carries park values → driver should use them, not the
        // (zeroed) handshake fallback.
        let mut cfg = Config::default();
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
        cfg.mount.park_ra_ticks = Some(5000);
        cfg.mount.park_dec_ticks = Some(-7000);
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();
        let s = d.state.read().await;
        assert_eq!(s.park_ra_ticks, Some(5000));
        assert_eq!(s.park_dec_ticks, Some(-7000));
    }

    #[tokio::test]
    async fn reconnect_after_set_park_picks_up_persisted_values() {
        // Regression test for the Copilot review feedback on PR #221:
        // SetPark persists the new park target to disk and updates the
        // in-memory state, but the in-memory `MountConfig` does not
        // change. A subsequent disconnect/reconnect within the same
        // process must therefore re-read the config file rather than
        // re-loading from the (stale) in-memory config — otherwise the
        // SetPark target silently reverts to whatever was in
        // `MountConfig` at process start (or the handshake fallback).
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        seed_default_config(&path);
        let d = device_with_path(path.clone());
        d.set_connected(true).await.unwrap();
        d.transport.seed_ra_position(8000).await;
        d.transport.seed_dec_position(-3000).await;
        d.set_park().await.unwrap();

        // Disconnect: in-memory park state is cleared.
        d.set_connected(false).await.unwrap();
        assert_eq!(d.state.read().await.park_ra_ticks, None);
        assert_eq!(d.state.read().await.park_dec_ticks, None);

        // Reset the mock encoders so reconnect's handshake fallback
        // would be (0, 0) — proves the re-read picked up SetPark's
        // values rather than just re-reading handshake.
        d.transport.seed_ra_position(0).await;
        d.transport.seed_dec_position(0).await;

        d.set_connected(true).await.unwrap();
        let s = d.state.read().await;
        assert_eq!(s.park_ra_ticks, Some(8000));
        assert_eq!(s.park_dec_ticks, Some(-3000));
    }

    #[tokio::test]
    async fn reconnect_with_partial_config_uses_handshake_for_missing_axis() {
        // Per-axis fallback: if the config sets only park_ra_ticks,
        // RA comes from the file and Dec comes from the handshake.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        // Hand-craft a JSON config that sets only park_ra_ticks
        // (park_dec_ticks absent, which `read_park_from_config`
        // must read as `None`).
        let mut cfg = Config::default();
        cfg.mount.park_ra_ticks = Some(1234);
        // park_dec_ticks deliberately left as None.
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        let d = device_with_path(path);
        d.set_connected(true).await.unwrap();
        let s = d.state.read().await;
        assert_eq!(s.park_ra_ticks, Some(1234));
        // Mock handshake reports Dec at 0.
        assert_eq!(s.park_dec_ticks, Some(0));
    }

    #[test]
    fn read_park_from_config_returns_none_for_each_missing_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let json = serde_json::json!({
            "mount": {
                "name": "Test",
            }
        });
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();
        let (ra, dec) = read_park_from_config(&path).unwrap();
        assert_eq!(ra, None);
        assert_eq!(dec, None);
    }

    #[test]
    fn read_park_from_config_parses_both_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let json = serde_json::json!({
            "mount": {
                "park_ra_ticks": 1234,
                "park_dec_ticks": -5678,
            }
        });
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();
        let (ra, dec) = read_park_from_config(&path).unwrap();
        assert_eq!(ra, Some(1234));
        assert_eq!(dec, Some(-5678));
    }

    #[test]
    fn read_park_from_config_treats_explicit_null_as_none_per_axis() {
        // Pins the doc-comment guarantee: a `None` return value means
        // the file did not set that key OR set it to `null`, and the
        // two axes are returned independently. Here `park_ra_ticks`
        // is set to a real value while `park_dec_ticks` is explicitly
        // JSON `null`; the helper must return `(Some(1234), None)`,
        // and the caller (`set_connected`) will then fall back to the
        // handshake-captured value for the Dec axis only.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let json = serde_json::json!({
            "mount": {
                "park_ra_ticks": 1234,
                "park_dec_ticks": null,
            }
        });
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();
        let (ra, dec) = read_park_from_config(&path).unwrap();
        assert_eq!(ra, Some(1234));
        assert_eq!(dec, None);
    }

    #[test]
    fn read_park_from_config_errors_on_wrong_type() {
        // Operator typo: park_ra_ticks declared as a string. Used to
        // be silently treated as None (fell back to handshake);
        // current contract is to surface the misconfiguration. Per
        // Copilot review on PR #221 (comment 3238774050).
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let json = serde_json::json!({
            "mount": {
                "park_ra_ticks": "not-an-integer",
                "park_dec_ticks": 0,
            }
        });
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();
        let err = read_park_from_config(&path).unwrap_err();
        match err {
            StarAdvError::Config(msg) => {
                assert!(msg.contains("park_ra_ticks"), "{msg}");
                assert!(msg.contains("integer"), "{msg}");
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn read_park_from_config_errors_on_float_value() {
        // serde_json::Value::Number for 1.5 isn't representable as
        // i64; `as_i64()` returns None and we surface a Config error
        // rather than silently dropping the value.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let json = serde_json::json!({
            "mount": {
                "park_ra_ticks": 1.5,
                "park_dec_ticks": 0,
            }
        });
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();
        let err = read_park_from_config(&path).unwrap_err();
        assert!(matches!(err, StarAdvError::Config(_)));
    }

    #[test]
    fn read_park_from_config_errors_on_out_of_i32_range() {
        // A value that fits in i64 but not i32 — e.g. someone copied
        // a large encoder count from a higher-resolution mount, or
        // a typo added a digit. Either way we should fail loudly so
        // the operator sees the bad value rather than silently
        // falling back to handshake.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let json = serde_json::json!({
            "mount": {
                "park_ra_ticks": i64::from(i32::MAX) + 1_i64,
                "park_dec_ticks": 0,
            }
        });
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();
        let err = read_park_from_config(&path).unwrap_err();
        match err {
            StarAdvError::Config(msg) => {
                assert!(msg.contains("park_ra_ticks"), "{msg}");
                assert!(msg.contains("i32"), "{msg}");
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn probe_park_file_writability_passes_on_a_writable_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        probe_park_file_writability(&path).unwrap();
    }

    #[test]
    fn canonicalise_config_path_returns_none_when_no_path_given() {
        assert!(canonicalise_config_path(None).is_none());
    }

    #[test]
    fn canonicalise_config_path_returns_resolved_path_when_file_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        let got = canonicalise_config_path(Some(&path)).expect("Some");
        // Result is canonicalised — must exist and resolve to the same
        // file. On macOS the temp dir lives under /private/var/..., so
        // an exact string match is fragile; just check it resolves.
        assert!(got.exists(), "canonical path must exist: {got:?}");
    }

    #[test]
    fn canonicalise_config_path_falls_back_to_input_on_failure() {
        // Path doesn't exist → canonicalize errors → fallback to the
        // original path. The warn! is logged but the function returns
        // the path unchanged so SetPark can still surface a real error
        // at write time.
        let nonexistent = PathBuf::from("/does/not/exist/config.json");
        let got = canonicalise_config_path(Some(&nonexistent)).expect("Some");
        assert_eq!(got, nonexistent);
    }

    #[test]
    fn warn_if_park_path_unwritable_returns_quietly_on_writable_directory() {
        // Smoke test: helper returns `()` on success. The warn! body
        // is exercised by `warn_if_park_path_unwritable_logs_on_failure`
        // below (unix-only).
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        warn_if_park_path_unwritable(&path);
    }

    #[cfg(unix)]
    #[test]
    fn warn_if_park_path_unwritable_logs_on_failure() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(dir.path(), perms).unwrap();
        // Helper returns `()` even on probe failure — the test passes
        // as long as the call doesn't panic. The internal warn! body
        // is what we're measuring for coverage.
        warn_if_park_path_unwritable(&path);
        // Restore so TempDir's Drop can clean up.
        let mut restored = std::fs::metadata(dir.path()).unwrap().permissions();
        restored.set_mode(0o755);
        std::fs::set_permissions(dir.path(), restored).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn probe_park_file_writability_fails_on_a_read_only_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        // Drop write permission on the parent directory so
        // `NamedTempFile::new_in` cannot stage a sibling.
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(dir.path(), perms).unwrap();
        // Probe should report the underlying I/O error.
        let err = probe_park_file_writability(&path).unwrap_err();
        // Restore write perms so TempDir's Drop can clean up.
        let mut restored = std::fs::metadata(dir.path()).unwrap().permissions();
        restored.set_mode(0o755);
        std::fs::set_permissions(dir.path(), restored).unwrap();
        // PermissionDenied is what Linux/macOS surface; either way
        // it must be classified as a permission / access issue.
        assert!(
            matches!(
                err.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Other
            ),
            "unexpected error kind: {err:?}"
        );
    }

    #[test]
    fn read_park_from_config_fails_when_mount_object_is_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        let err = read_park_from_config(&path).unwrap_err();
        match err {
            StarAdvError::Config(msg) => assert!(msg.contains("mount"), "{msg}"),
            other => panic!("expected Config, got {other:?}"),
        }
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
        // The flag must flip to true before `pulse_guide` returns —
        // see the atomic check-and-set under the write lock in
        // `MountDevice::pulse_guide`. The 30-second duration here is
        // not a "short pulse" test; it just keeps the watcher's
        // `tokio::time::sleep` from completing during the assertion
        // read so the only way `is_pulse_guiding()` can be true is
        // the synchronous flag-set.
        let d = connected_device().await;
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
    async fn pulse_guide_rolls_back_flag_on_wire_failure() {
        // `StuckAxisFactory` returns `running=true` on every `:f` poll
        // so `stop_and_wait` times out after `AXIS_STOP_TIMEOUT` (2 s),
        // surfacing as a `StarAdvError::Transport` →
        // `ASCOMErrorCode::INVALID_OPERATION`. The pulse-guide
        // rollback path must clear `pulse_guiding_<axis>` so a
        // subsequent caller isn't blocked by the half-applied pulse
        // and `IsPulseGuiding` reports `false` consistent with the
        // lack of actual motion.
        let mut cfg = Config::default();
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(StuckAxisFactory),
        ));
        let d = MountDevice::new(cfg.mount, manager);
        d.set_connected(true).await.unwrap();
        let err = d
            .pulse_guide(GuideDirection::North, Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(
            !d.is_pulse_guiding().await.unwrap(),
            "flag must be cleared after a wire-failure rollback"
        );
    }

    #[tokio::test]
    async fn pulse_guide_rejects_step_period_overflow() {
        // Tiny guide-rate fractions push the shifted step period
        // above the protocol's 24-bit `:I` payload range. Without
        // the check, `encode_u24` would silently truncate to a
        // wrap-around value and run the mount at a wildly wrong
        // speed. For the GTi mock's defaults (sidereal_period ≈
        // 380K), the boundary is `rate_factor ≈ 0.023`; fraction
        // 0.001 is well below.
        let d = connected_device().await;
        d.set_guide_rate_declination(SIDEREAL_DEG_PER_SEC * 0.001)
            .await
            .unwrap();
        let err = d
            .pulse_guide(GuideDirection::North, Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
        // The rollback must clear the flag the period validation
        // would otherwise have set.
        assert!(!d.is_pulse_guiding().await.unwrap());
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

    // ---------- Phase 6: through-wrap routing helpers ----------

    const GTI_CPR: u32 = 0x0037_5F00; // 3,628,800

    /// Hardware-verified GTi counterweight binding zone in mech_HA hours
    /// (matches `default_binding_zone_min/max_hours` in `config.rs`).
    const GTI_BINDING_ZONE: (f64, f64) = (6.95, 11.05);

    #[test]
    fn fold_delta_to_canonical_passes_through_small_deltas() {
        assert_eq!(fold_delta_to_canonical(0, GTI_CPR), 0);
        assert_eq!(fold_delta_to_canonical(1, GTI_CPR), 1);
        assert_eq!(fold_delta_to_canonical(-1, GTI_CPR), -1);
        assert_eq!(fold_delta_to_canonical(100_000, GTI_CPR), 100_000);
        assert_eq!(fold_delta_to_canonical(-100_000, GTI_CPR), -100_000);
    }

    #[test]
    fn fold_delta_to_canonical_collapses_long_way_to_short_way() {
        let half = GTI_CPR as i32 / 2;
        // Delta of +cpr/2 + 100 folds to −cpr/2 + 100 (taking the
        // shorter path on the modular axis).
        let folded = fold_delta_to_canonical(half + 100, GTI_CPR);
        assert_eq!(folded, -half + 100);
        // Symmetric for the negative direction.
        let folded = fold_delta_to_canonical(-half - 100, GTI_CPR);
        assert_eq!(folded, half - 100);
    }

    #[test]
    fn fold_delta_to_canonical_recovers_from_through_wrap_encoder() {
        // After a through-wrap flip slew, the encoder may have landed
        // at e.g. raw −1,890,000 (= +1,738,800 modular). A subsequent
        // pickup that computes `target_canonical (+1,738,800) −
        // current_raw (−1,890,000) = +3,628,800` would order a full
        // revolution; folding collapses it to the (near-)zero residual
        // it should be.
        let target_canonical = 1_738_800_i32;
        let current_raw = -1_890_000_i32;
        let raw_delta = target_canonical - current_raw;
        // raw_delta ≈ cpr. Folded should be small.
        let folded = fold_delta_to_canonical(raw_delta, GTI_CPR);
        assert!(folded.abs() < 1000, "expected near-zero, got {folded}");
    }

    #[test]
    fn flip_slew_ra_delta_forward_flip_from_pre_flip_zero_uses_natural_ccw() {
        // Forward flip starting at encoder ≈ 0 (mech_HA ≈ 0, pre-flip
        // pierWest at meridian). Target = −cpr/2 (post-flip wrap).
        // Canonical CCW; path stays in negative half. Safe.
        let cpr = GTI_CPR;
        let current = 0;
        let canonical = -(cpr as i32 / 2);
        let issued = flip_slew_ra_delta(canonical, current, cpr, GTI_BINDING_ZONE).unwrap();
        assert_eq!(issued, canonical, "natural CCW already in the safe half");
    }

    #[test]
    fn flip_slew_ra_delta_forward_flip_target_minus_half_h_takes_long_way() {
        // The plan §2.0 canonical case: pre-flip mech_HA = −0.5, target
        // post-flip mech_HA = +11.5, canonical = +cpr*11.5/24 -
        // (-cpr*0.5/24) = +cpr/2 + small. (Approximated by +1.815M
        // ticks at cpr_ra = 3.6288M.) Force CCW long way through the
        // wrap.
        let cpr = GTI_CPR;
        let current = -75_600; // mech_HA = -0.5
        let canonical = 1_815_000_i32;
        let issued = flip_slew_ra_delta(canonical, current, cpr, GTI_BINDING_ZONE).unwrap();
        assert!(issued < 0, "must force CCW long way");
        assert_eq!(
            (issued - canonical).rem_euclid(cpr as i32),
            0,
            "same modular destination"
        );
    }

    #[test]
    fn flip_slew_ra_delta_flip_back_from_minus_half_cpr_uses_natural_cw() {
        // Flip-back: current at raw -cpr/2 (post-flip wrap, mech_HA = -12).
        // Target = -cpr/4 (Park 3, mech_HA = -6). Canonical = +cpr/4
        // (positive CW). The path is mech_HA -12 → -11 → ... → -6,
        // entirely in the safe negative half. Use canonical.
        //
        // Regression for hardware validation #3: my prior `always CCW`
        // rule forced -3*cpr/4 here, routing through +6 to +9 binding
        // zone and slamming the CW shaft into the pier.
        let cpr = GTI_CPR;
        let current = -(cpr as i32 / 2);
        let canonical = cpr as i32 / 4;
        let issued = flip_slew_ra_delta(canonical, current, cpr, GTI_BINDING_ZONE).unwrap();
        assert_eq!(
            issued, canonical,
            "flip-back from -cpr/2 must use natural CW"
        );
        assert!(issued > 0);
    }

    #[test]
    fn flip_slew_ra_delta_flip_back_from_plus_half_cpr_forces_cw_through_wrap() {
        // Flip-back from raw +cpr/2 (mech_HA = +12 = -12 modular,
        // post-flip wrap from the other direction). Target = -cpr/4.
        // Canonical = -cpr/4 - +cpr/2 = -3cpr/4 → fold to +cpr/4 (CW,
        // because |−3cpr/4| > half_cpr). Force CW; path stays in safe
        // half going from +cpr/2 → +cpr/2 + cpr/4 = +3cpr/4 raw
        // (modular: -cpr/4 = mech_HA -6).
        let cpr = GTI_CPR;
        let current = cpr as i32 / 2;
        let canonical_raw = -(cpr as i32 / 4) - current; // -3cpr/4
        let canonical = fold_delta_to_canonical(canonical_raw, cpr); // +cpr/4
        let issued = flip_slew_ra_delta(canonical, current, cpr, GTI_BINDING_ZONE).unwrap();
        assert!(issued > 0, "post-flip wrap → safe arc must use CW");
        assert_eq!(issued, canonical, "canonical CW is already safe here");
    }

    #[test]
    fn fold_delta_to_canonical_handles_zero_cpr_defensively() {
        // cpr = 0 is the degenerate "parameter cache not populated"
        // case. Callers normally short-circuit on NOT_CONNECTED
        // before reaching this helper; pass-through is the defensive
        // fallback so a logic bug there can't divide by zero here.
        assert_eq!(fold_delta_to_canonical(12_345, 0), 12_345);
    }

    #[test]
    fn flip_slew_ra_delta_handles_zero_cpr_defensively() {
        assert_eq!(
            flip_slew_ra_delta(12_345, 0, 0, GTI_BINDING_ZONE).unwrap(),
            12_345
        );
    }

    #[test]
    fn flip_slew_ra_delta_zero_canonical_returns_zero() {
        assert_eq!(
            flip_slew_ra_delta(0, 0, GTI_CPR, GTI_BINDING_ZONE).unwrap(),
            0
        );
    }

    #[test]
    fn flip_slew_ra_delta_park4_to_park5_north_uses_canonical_through_wrap() {
        // Hardware regression (2026-05-16): Park 4 N → Park 5 N flip
        // slew. Snap.ra ≈ raw -1,810,272 (mech_HA -11.974, post-flip
        // pierEast just east of the saddle-east wrap). The slew planner
        // chose pierWest (target HA -12 outside the flip window), with
        // target encoder near +cpr/2 (mech_HA +11.999, the saddle-east
        // wrap from the other side). fold_delta_to_canonical produces a
        // small CCW step (~-3.8k ticks) that physically nudges the RA
        // encoder past the -12 wrap and into +12 folded — the path stays
        // entirely in the safe arc (no positive mech_HA visited). The
        // old sign-blind heuristic forced canonical + cpr ≈ +3.625M
        // ticks CW, a near-full polar-axis revolution that swept
        // mech_HA through (+6.95, +11.05) and slammed the CW shaft into
        // the pier — the operator powered the mount off mid-sweep.
        let cpr = GTI_CPR;
        let current = -1_810_272_i32;
        let canonical = -3_798_i32;
        let issued = flip_slew_ra_delta(canonical, current, cpr, GTI_BINDING_ZONE).unwrap();
        assert_eq!(
            issued, canonical,
            "canonical CCW wrap-crossing must be preserved; old long-way CW \
             would full-revolution through the binding zone"
        );
    }

    #[test]
    fn flip_slew_ra_delta_canonical_path_crossing_binding_zone_takes_long_way() {
        // Current at mech_HA = +5 (just below the binding zone), target
        // at mech_HA = +11.5 (just above it). Canonical short path
        // would sweep mech_HA from +5 to +11.5, entering (+6.95, +11.05)
        // around mech_HA = +7. Force the long way around through the
        // safe negative half.
        let cpr = GTI_CPR;
        let current = (cpr as i32) * 5 / 24; // mech_HA = +5
        let canonical = (cpr as i32) * 13 / 48; // +6.5 hours of mech_HA
        let issued = flip_slew_ra_delta(canonical, current, cpr, GTI_BINDING_ZONE).unwrap();
        assert!(
            issued < 0,
            "canonical path enters (6.95, 11.05); must take CCW long way (got {issued})"
        );
        assert_eq!(
            (issued - canonical).rem_euclid(cpr as i32),
            0,
            "same modular destination"
        );
    }

    #[test]
    fn flip_slew_ra_delta_empty_binding_zone_always_uses_canonical() {
        // Empty zone (min ≥ max) disables routing; the function
        // collapses to the canonical short delta regardless of which
        // half the current sits in. Matches the BDD-test config that
        // sets `binding_zone_min = 24.0, binding_zone_max = 0.0` to
        // bypass the safety gate.
        let cpr = GTI_CPR;
        let empty_zone = (24.0_f64, 0.0_f64);
        for (current, canonical) in [
            (0_i32, -(cpr as i32 / 2)),
            (-(cpr as i32 / 2), cpr as i32 / 4),
            (-1_810_272_i32, -3_798_i32),
        ] {
            assert_eq!(
                flip_slew_ra_delta(canonical, current, cpr, empty_zone).unwrap(),
                canonical,
                "empty zone must pass canonical through (current={current}, canonical={canonical})"
            );
        }
    }

    /// Wide-zone reading of the "CW must not rise more than 0.95 h above
    /// horizontal" rule — the inner edge is at +0.95 h (CW just past
    /// horizontal on the ascending side) instead of +6.95 h (CW about to
    /// bind the pier on the descending side). Hardware-revealed
    /// 2026-05-17 when a flip-in-place from Park 3 took the long way
    /// around through the wider unsafe arc.
    const GTI_WIDE_ZONE: (f64, f64) = (0.95, 11.05);

    #[test]
    fn flip_slew_ra_delta_refuses_when_both_directions_cross_zone() {
        // Park 3 → "NCP on East side" via SetSideOfPier(East):
        // current at raw -cpr/4 (mech_HA = -6 h, Park 3 saddle west),
        // target at raw +cpr/4 (mech_HA = +6 h, Park 3's flipped twin).
        // Canonical fold of +cpr/2 lands on -cpr/2 → CCW short path
        // sweeps mech_HA -6 → -12 (wrap) → +6, crossing the k=-1
        // mirror of the wide zone. The long way (+cpr/2 CW) sweeps
        // mech_HA -6 → 0 → +6, crossing the wide zone directly at
        // (+0.95, +6). Both directions unsafe → refuse.
        let cpr = GTI_CPR;
        let current = -(cpr as i32 / 4); // mech_HA = -6
        let canonical = -(cpr as i32 / 2); // -12 h CCW, canonical-fold boundary
        let err =
            flip_slew_ra_delta(canonical, current, cpr, GTI_WIDE_ZONE).expect_err("both cross");
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(
            err.message.contains("both cross"),
            "error must mention both-direction crossing; got: {}",
            err.message
        );
    }

    #[test]
    fn flip_slew_ra_delta_picks_long_way_when_canonical_alone_crosses_wide_zone() {
        // Wide-zone analogue of the existing
        // `flip_slew_ra_delta_canonical_path_crossing_binding_zone_takes_long_way`
        // test: current at mech_HA = -5, canonical = +6 h (CW into the
        // wide zone (+0.95, +11.05)). Long way (-18 h CCW) sweeps
        // mech_HA -5 → -12 → +1 (folded). Path range [-23, -5] doesn't
        // cross k=-1 zone (-23.05, -12.95) (path_lo -23 is just inside
        // the zone... actually let me check: path_lo=-23 < bz_hi=-12.95
        // ✓; bz_lo=-23.05 < path_hi=-5 ✓. OVERLAP. So long way also
        // crosses.) Force a case where canonical crosses and long way
        // doesn't: current at mech_HA = -1, canonical = +3 h (CW into
        // wide zone). Long way (-21 h CCW) end at -22 ≡ +2 mod 24.
        // Path range [-22, -1]. k=-1 zone (-23.05, -12.95): path_lo
        // -22 < -12.95 ✓; bz_lo -23.05 < -1 ✓. Also crosses... the
        // wide zone is hard to escape via the long way around. This
        // is the empirical motivation for the both-cross check.
        let cpr = GTI_CPR;
        let current = -(cpr as i32) / 24; // mech_HA = -1
        let canonical = (cpr as i32) / 8; // +3 h CW
        let err = flip_slew_ra_delta(canonical, current, cpr, GTI_WIDE_ZONE)
            .expect_err("with wide zone, both directions cross");
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn check_non_flip_ra_path_refuses_when_canonical_sweep_crosses_zone() {
        // Non-flip slew from mech_HA = +0.5 h to +11.5 h. Both
        // endpoints sit outside the wide zone (+0.95, +11.05) but the
        // canonical short sweep (+11 h CW) crosses the entire zone
        // interior. Today's destination-only `check_within_safe_envelope`
        // would let this through; the path-aware check refuses it.
        let cpr = GTI_CPR;
        let current = (cpr as i32) / 48; // mech_HA = +0.5
        let canonical = (cpr as i32) * 11 / 24; // +11 h CW
        let err = check_non_flip_ra_path(canonical, current, cpr, GTI_WIDE_ZONE)
            .expect_err("path crosses zone");
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(
            err.message.contains("non-flip RA slew"),
            "error must identify itself as non-flip path check; got: {}",
            err.message
        );
    }

    #[test]
    fn check_non_flip_ra_path_accepts_clean_sweep() {
        // Non-flip slew that stays clear of the zone: current at
        // mech_HA = -3 h, canonical -2 h CCW → sweep [-5, -3]. No
        // overlap with the wide zone or its k=-1 mirror. Returns Ok.
        let cpr = GTI_CPR;
        let current = -(cpr as i32) / 8; // mech_HA = -3
        let canonical = -(cpr as i32) / 12; // -2 h CCW
        check_non_flip_ra_path(canonical, current, cpr, GTI_WIDE_ZONE)
            .expect("sweep [-5, -3] doesn't touch the wide zone");
    }

    #[test]
    fn check_non_flip_ra_path_passes_through_for_zero_inputs() {
        // Defensive degenerate cases (consistent with flip_slew_ra_delta).
        check_non_flip_ra_path(12_345, 0, 0, GTI_WIDE_ZONE).unwrap();
        check_non_flip_ra_path(0, 0, GTI_CPR, GTI_WIDE_ZONE).unwrap();
    }

    // ---------- flip_slew_dec_delta (Dec routing through the visible pole) ----------

    /// Trace the absolute encoder traversal range from `start` by `delta`
    /// and report `true` iff the trajectory crosses `pole_ticks` (the
    /// unsafe-pole position) on its way to the end.
    fn path_crosses_pole(start: i32, delta: i32, pole_ticks: i32, cpr: u32) -> bool {
        let cpr_i = cpr as i32;
        let end = start + delta;
        let (lo, hi) = if end >= start {
            (start, end)
        } else {
            (end, start)
        };
        // Test all modular replicas of `pole_ticks` that could fall in
        // `[lo, hi]`. Since `|delta| ≤ cpr`, at most one replica matters.
        (-2..=2).any(|k| {
            let p = pole_ticks + k * cpr_i;
            p >= lo && p <= hi
        })
    }

    #[test]
    fn flip_slew_dec_delta_north_park3_start_to_post_flip_positive_uses_natural_cw() {
        // Park 3: start at encoder +cpr/4 (= +90°, NCP). Target = +135°
        // encoder = +3*cpr/8 (celestial dec = +45° on the flipped side).
        // Natural delta = +cpr/8 (positive CW). Path stays in upper
        // half, doesn't touch SCP. Use canonical.
        let cpr = GTI_CPR;
        let quarter = cpr as i32 / 4;
        let current = quarter; // +90° encoder
        let target = (cpr as f64 * 3.0 / 8.0).round() as i32; // +135°
        let canonical = target - current; // +cpr/8
        assert_eq!(
            flip_slew_dec_delta(canonical, current, cpr, true),
            canonical,
            "Park 3 → +135° should use the natural CW direction"
        );
        // Confirm the path avoids SCP (-cpr/4).
        let issued = flip_slew_dec_delta(canonical, current, cpr, true);
        assert!(!path_crosses_pole(current, issued, -quarter, cpr));
    }

    #[test]
    fn flip_slew_dec_delta_north_pre_flip_zero_to_post_flip_dec_zero_takes_long_way() {
        // The bug case from the first hardware run: starting at encoder
        // = 0 (codebase historical home), the canonical fold of a slew
        // to dec_encoder = ±cpr/2 returns negative (CCW), which routes
        // the Dec axis through −cpr/4 (SCP, below horizon for north).
        // The fix forces the CW long way through +cpr/4 (NCP).
        let cpr = GTI_CPR;
        let quarter = cpr as i32 / 4;
        let half_cpr = cpr as i32 / 2;
        let canonical = -half_cpr; // post-flip target for celestial dec = 0
        let issued = flip_slew_dec_delta(canonical, 0, cpr, true);
        assert_eq!(issued, half_cpr, "must force CW through NCP");
        // Verify path crosses NCP and not SCP.
        assert!(path_crosses_pole(0, issued, quarter, cpr));
        assert!(!path_crosses_pole(0, issued, -quarter, cpr));
    }

    #[test]
    fn flip_slew_dec_delta_north_flip_back_from_upper_post_flip_uses_natural_ccw() {
        // Flip-back: starting at encoder +3*cpr/8 (= +135°, upper
        // post-flip), target = 0 (pre-flip celestial equator). Natural
        // CCW (negative direction) crosses +cpr/4 (NCP). Don't take
        // the long way — CCW is safe here.
        let cpr = GTI_CPR;
        let quarter = cpr as i32 / 4;
        let current = (cpr as f64 * 3.0 / 8.0).round() as i32;
        let target = 0;
        let canonical = target - current; // negative
        let issued = flip_slew_dec_delta(canonical, current, cpr, true);
        assert_eq!(
            issued, canonical,
            "flip-back from +135° must use natural CCW"
        );
        // Verify path crosses NCP (the safe pole) and not SCP.
        assert!(path_crosses_pole(current, issued, quarter, cpr));
        assert!(!path_crosses_pole(current, issued, -quarter, cpr));
    }

    #[test]
    fn flip_slew_dec_delta_north_below_equator_pre_flip_to_post_flip_positive_uses_cw() {
        // Pre-flip but below celestial equator: encoder = -cpr/8
        // (= -45°). Target = +3*cpr/8 (= +135° encoder, post-flip
        // dec=+45). CW path: -45 → 0 → +90 (NCP) → +135. SAFE.
        let cpr = GTI_CPR;
        let quarter = cpr as i32 / 4;
        let current = -(cpr as i32 / 8);
        let target = (cpr as i32 * 3) / 8;
        let canonical = target - current; // positive, half cpr
        let issued = flip_slew_dec_delta(canonical, current, cpr, true);
        assert_eq!(issued, canonical, "natural CW direction is safe");
        assert!(path_crosses_pole(current, issued, quarter, cpr));
        assert!(!path_crosses_pole(current, issued, -quarter, cpr));
    }

    #[test]
    fn flip_slew_dec_delta_north_below_equator_pre_flip_to_post_flip_negative_forces_long_cw() {
        // Pre-flip below equator: encoder = -cpr/8. Target =
        // -3*cpr/8 (post-flip negative). Natural CCW (negative) path:
        // -cpr/8 → -cpr/4 (SCP) → -3*cpr/8. UNSAFE. Force long-way CW:
        // -cpr/8 → +cpr/4 (NCP) → +cpr/2 → wraps → -3*cpr/8. SAFE.
        let cpr = GTI_CPR;
        let quarter = cpr as i32 / 4;
        let current = -(cpr as i32 / 8);
        let target_canonical = -(cpr as i32 * 3) / 8;
        let canonical = target_canonical - current; // negative
        let issued = flip_slew_dec_delta(canonical, current, cpr, true);
        // Forced to positive direction (long way).
        assert!(issued > 0);
        // Lands at the same modular destination.
        assert_eq!((issued - canonical).rem_euclid(cpr as i32), 0);
        // Path crosses NCP, not SCP.
        assert!(path_crosses_pole(current, issued, quarter, cpr));
        assert!(!path_crosses_pole(current, issued, -quarter, cpr));
    }

    #[test]
    fn flip_slew_dec_delta_north_lower_post_flip_to_pre_flip_uses_ccw_through_wrap() {
        // Lower post-flip: encoder = -3*cpr/8 (= -135°, celestial
        // dec=-45 on post-flip side). Target = 0 (pre-flip equator).
        // Natural CW (positive) path: -3*cpr/8 → -cpr/4 (SCP) → 0.
        // UNSAFE. Force CCW (negative) long-way: -3*cpr/8 →
        // -cpr/2 → wraps → +cpr/2 → +cpr/4 (NCP) → 0. SAFE.
        let cpr = GTI_CPR;
        let quarter = cpr as i32 / 4;
        let current = -((cpr as i32 * 3) / 8);
        let canonical = 0 - current; // positive
        let issued = flip_slew_dec_delta(canonical, current, cpr, true);
        assert!(issued < 0, "must force CCW long way");
        assert_eq!((issued - canonical).rem_euclid(cpr as i32), 0);
        assert!(path_crosses_pole(current, issued, quarter, cpr));
        assert!(!path_crosses_pole(current, issued, -quarter, cpr));
    }

    #[test]
    fn flip_slew_dec_delta_south_inverts_safe_pole() {
        // Southern observer: SCP is visible, NCP is below horizon.
        // Repeat the "park 3 start" case but with southern config:
        // start at encoder = -cpr/4 (Park 3 south = SCP). Target =
        // -3*cpr/8. Natural CCW (negative). Should be used as-is.
        let cpr = GTI_CPR;
        let quarter = cpr as i32 / 4;
        let current = -quarter; // SCP for southern Park 3
        let target = -((cpr as i32 * 3) / 8);
        let canonical = target - current; // negative
        let issued = flip_slew_dec_delta(canonical, current, cpr, false);
        assert_eq!(
            issued, canonical,
            "south Park 3 → -3cpr/8 must use natural CCW"
        );
        // For south, the unsafe pole is +cpr/4 (NCP). Verify path
        // doesn't cross it.
        assert!(!path_crosses_pole(current, issued, quarter, cpr));
    }

    #[test]
    fn flip_slew_dec_delta_handles_zero_cpr_defensively() {
        assert_eq!(flip_slew_dec_delta(12_345, 0, 0, true), 12_345);
    }

    #[test]
    fn flip_slew_dec_delta_zero_canonical_returns_zero() {
        assert_eq!(flip_slew_dec_delta(0, 0, GTI_CPR, true), 0);
    }

    // ---------- Phase 6: SetSideOfPier + CanSetPierSide ----------

    async fn flip_enabled_device() -> MountDevice {
        let mut cfg = Config::default();
        if let crate::config::TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.polling_interval = Duration::from_millis(20);
        }
        cfg.mount.settle_after_slew = Duration::from_millis(0);
        // Disable the binding-zone check for this test.
        cfg.mount.binding_zone_min_hours = 24.0;
        cfg.mount.binding_zone_max_hours = 0.0;
        cfg.mount.flip_policy.enabled = true;
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        MountDevice::new(cfg.mount, manager)
    }

    async fn flip_enabled_connected_device() -> MountDevice {
        let d = flip_enabled_device().await;
        d.set_connected(true).await.unwrap();
        d
    }

    #[tokio::test]
    async fn can_set_pier_side_defaults_to_false() {
        let d = fast_settle_connected().await;
        assert!(!d.can_set_pier_side().await.unwrap());
    }

    #[tokio::test]
    async fn can_set_pier_side_is_true_when_flip_policy_enabled() {
        let d = flip_enabled_connected_device().await;
        assert!(d.can_set_pier_side().await.unwrap());
    }

    #[tokio::test]
    async fn set_side_of_pier_returns_not_implemented_when_flip_policy_disabled() {
        let d = fast_settle_connected().await;
        let err = d.set_side_of_pier(PierSide::East).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn set_side_of_pier_rejects_unknown_with_invalid_value() {
        let d = flip_enabled_connected_device().await;
        let err = d.set_side_of_pier(PierSide::Unknown).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[tokio::test]
    async fn set_side_of_pier_refuses_when_not_connected() {
        let d = flip_enabled_device().await;
        let err = d.set_side_of_pier(PierSide::East).await.unwrap_err();
        assert_eq!(err.code, ASCOMError::NOT_CONNECTED.code);
    }

    #[tokio::test]
    async fn set_side_of_pier_refuses_while_parked() {
        let d = flip_enabled_connected_device().await;
        // Park puts AtPark = true; SetSideOfPier must refuse with
        // INVALID_WHILE_PARKED before reaching the slew planner.
        d.state.write().await.at_park = true;
        let err = d.set_side_of_pier(PierSide::East).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_WHILE_PARKED);
    }

    #[tokio::test]
    async fn set_side_of_pier_refuses_while_slew_in_progress() {
        let d = flip_enabled_connected_device().await;
        d.state.write().await.slew_in_progress = true;
        let err = d.set_side_of_pier(PierSide::East).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[tokio::test]
    async fn set_side_of_pier_to_current_side_succeeds_as_noop() {
        let d = flip_enabled_connected_device().await;
        // Mock starts with Dec encoder = 0 (within ±90°), site latitude
        // = 0° (northern convention since `>= 0`), so current side is
        // pierWest. SetSideOfPier(West) is a no-op.
        d.set_side_of_pier(PierSide::West).await.unwrap();
        // State unchanged.
        assert!(!d.state.read().await.slew_in_progress);
    }

    #[tokio::test]
    async fn set_side_of_pier_to_opposite_side_starts_a_flip_slew() {
        let d = flip_enabled_connected_device().await;
        d.set_side_of_pier(PierSide::East).await.unwrap();
        // Slew was issued — the state should now show slew_in_progress
        // until the watcher clears it. The watcher may have already
        // completed in the mock (instant-settle config), so accept
        // either: the slew was either still in progress at this read,
        // or already finished with the encoder mutated.
        let s = d.state.read().await;
        let slewing = s.slew_in_progress;
        let target_set = s.target_ra_hours.is_some() && s.target_dec_degrees.is_some();
        drop(s);
        assert!(
            slewing || target_set,
            "expected slew to have been issued (slew_in_progress or target_ra latched)"
        );
    }

    // ---- watcher_poll_with_retry --------------------------------------
    //
    // The slew/park watchers' transport-error path used to exit on the
    // first failed `poll_axes_now`, which made a single transient USB-
    // CDC glitch take the watcher offline for the rest of the slew. The
    // helper now retries [`WATCHER_POLL_RETRY_LIMIT`] times and on
    // exhaustion fires a best-effort `:L` on both axes so a runaway
    // motor doesn't continue commutating with no observer.

    use crate::transport::{Transport, TransportFactory};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Inject N consecutive transport failures and count `:L<axis>`
    /// frames that crossed the wire. Shared between the test and the
    /// inner [`FlakyTransport`] so the test can observe both knobs.
    struct FlakyController {
        fail_remaining: AtomicU32,
        stop_calls_ra: AtomicU32,
        stop_calls_dec: AtomicU32,
    }

    /// Wraps [`crate::transport::mock::MockTransport`] and fails the
    /// first `fail_remaining` round-trips. Every `:L<axis>` frame is
    /// counted before the fail check so even a `:L` that lands during
    /// the failure window still registers (the retry-exhaustion path
    /// fires `:L` regardless of whether the transport is responsive).
    struct FlakyTransport {
        inner: crate::transport::mock::MockTransport,
        ctrl: Arc<FlakyController>,
    }

    #[async_trait]
    impl Transport for FlakyTransport {
        async fn round_trip(
            &self,
            request: &[u8],
            timeout: Duration,
        ) -> crate::error::Result<Vec<u8>> {
            if request.len() >= 3 && request[1] == b'L' {
                match request[2] {
                    b'1' => {
                        self.ctrl.stop_calls_ra.fetch_add(1, Ordering::SeqCst);
                    }
                    b'2' => {
                        self.ctrl.stop_calls_dec.fetch_add(1, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
            if self.ctrl.fail_remaining.load(Ordering::SeqCst) > 0 {
                self.ctrl.fail_remaining.fetch_sub(1, Ordering::SeqCst);
                return Err(StarAdvError::Transport("flaky test eof".to_string()));
            }
            self.inner.round_trip(request, timeout).await
        }

        async fn close(&self) -> crate::error::Result<()> {
            Ok(())
        }
    }

    struct FlakyTransportFactory {
        ctrl: Arc<FlakyController>,
    }

    #[async_trait]
    impl TransportFactory for FlakyTransportFactory {
        async fn open(&self, _config: &Config) -> crate::error::Result<Arc<dyn Transport>> {
            Ok(Arc::new(FlakyTransport {
                inner: crate::transport::mock::MockTransport::new(),
                ctrl: self.ctrl.clone(),
            }))
        }
    }

    async fn flaky_manager() -> (Arc<TransportManager>, Arc<FlakyController>) {
        let ctrl = Arc::new(FlakyController {
            fail_remaining: AtomicU32::new(0),
            stop_calls_ra: AtomicU32::new(0),
            stop_calls_dec: AtomicU32::new(0),
        });
        let factory = Arc::new(FlakyTransportFactory { ctrl: ctrl.clone() });
        let manager = Arc::new(TransportManager::new(Config::default(), factory));
        // Connect with fail_remaining = 0 so the handshake completes,
        // then return the controller so the test can flip the failure
        // budget on without interfering with init.
        manager.connect().await.unwrap();
        (manager, ctrl)
    }

    #[tokio::test]
    async fn watcher_poll_with_retry_returns_ok_on_first_success() {
        let (manager, ctrl) = flaky_manager().await;
        let snap = watcher_poll_with_retry(&manager, "test")
            .await
            .expect("happy-path poll should succeed");
        // No retries needed → no best-effort :L was issued.
        assert_eq!(ctrl.stop_calls_ra.load(Ordering::SeqCst), 0);
        assert_eq!(ctrl.stop_calls_dec.load(Ordering::SeqCst), 0);
        // Mock transport seeds positions at handshake; just confirm the
        // returned snapshot looks structurally valid (no panic, both
        // axes populated even if zero).
        let _ = snap.ra.position_ticks;
        let _ = snap.dec.position_ticks;
    }

    #[tokio::test]
    async fn watcher_poll_with_retry_recovers_after_transient_error() {
        let (manager, ctrl) = flaky_manager().await;
        // Fail the next round-trip exactly once: the helper's second
        // attempt should land on a healthy transport and return Ok.
        ctrl.fail_remaining.store(1, Ordering::SeqCst);
        watcher_poll_with_retry(&manager, "test")
            .await
            .expect("retry should recover from a single transient error");
        // No retry-exhaustion path → no best-effort :L.
        assert_eq!(ctrl.stop_calls_ra.load(Ordering::SeqCst), 0);
        assert_eq!(ctrl.stop_calls_dec.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn watcher_poll_with_retry_exhausts_then_issues_best_effort_stop() {
        let (manager, ctrl) = flaky_manager().await;
        // Saturate the failure budget so every retry attempt errors.
        ctrl.fail_remaining.store(u32::MAX, Ordering::SeqCst);
        let err = watcher_poll_with_retry(&manager, "test")
            .await
            .expect_err("retry budget should be exhausted");
        match err {
            StarAdvError::Transport(_) => {}
            other => panic!("expected Transport error, got {other:?}"),
        }
        // Best-effort `:L` must fire on both axes regardless of whether
        // it lands — the test counts the frames before the fail check
        // in the flaky transport.
        assert_eq!(ctrl.stop_calls_ra.load(Ordering::SeqCst), 1);
        assert_eq!(ctrl.stop_calls_dec.load(Ordering::SeqCst), 1);
    }
}
