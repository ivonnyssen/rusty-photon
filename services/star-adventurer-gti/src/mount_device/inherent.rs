//! Inherent methods on [`MountDevice`] — the helpers the `Device` and
//! `Telescope` trait impls compose to expose the ASCOM surface.
//!
//! Grouped here so the trait-impl files (`device.rs`, `telescope.rs`)
//! stay focused on protocol dispatch. The methods fall into a few
//! buckets:
//!
//! - **Error mapping**: [`MountDevice::ascom`].
//! - **Validation**: [`MountDevice::validate_coordinates`],
//!   [`MountDevice::check_within_safe_envelope`], and the free
//!   [`validate_guide_rate`] used by `set_guide_rate_*`.
//! - **Preconditions**: [`MountDevice::ensure_connected`],
//!   [`MountDevice::ensure_unparked`].
//! - **Motion control wrappers**: [`MountDevice::stop_and_wait`]
//!   (ASCOM-mapped wrapper over [`super::slew::stop_axis_and_wait`]),
//!   [`MountDevice::await_slew_complete`] (synchronous-slew polling).
//! - **Post-connect lifecycle**:
//!   [`MountDevice::seed_home_pose_after_connect`],
//!   [`MountDevice::load_park_target_after_connect`].
//! - **Slew planner**:
//!   [`MountDevice::execute_slew_with_explicit_side`] — the shared
//!   body for `SlewToCoordinatesAsync` and `SetSideOfPier`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ascom_alpaca::api::telescope::{PierSide, Telescope};
use ascom_alpaca::api::Device;
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use rusty_photon_shared_transport::{Session, SessionError, TransportError};
use skywatcher_motor_protocol::{Axis, Command};
use tracing::{debug, info};

use crate::codec::SkywatcherCodec;
use crate::coordinates::{
    dec_degrees_to_ticks, fold_ha, fold_to_canonical_band, local_sidereal_time_hours,
    mechanical_ha_to_ra_ticks, ra_to_mechanical_ha, side_of_pier as side_of_pier_calc,
    target_encoder_flipped, target_encoder_normal, SIDEREAL_DEG_PER_SEC,
};
use crate::error::StarAdvError;

use super::park_persistence::read_park_from_config;
use super::slew::{
    check_non_flip_ra_path, flip_slew_dec_delta, flip_slew_ra_delta, issue_slew_axis,
    stop_axis_and_wait, AXIS_STOP_TIMEOUT,
};
use super::watchers::spawn_slew_completion_watcher;
use super::{pre_flip_side_for_latitude, MountDevice};

/// Upper bound on how long the synchronous `SlewToCoordinates` /
/// `SlewToTarget` will wait for the watcher to clear `slew_in_progress`.
/// 5 minutes — far longer than any realistic slew (a worst-case full
/// half-revolution at high-speed slew rate is well under a minute on
/// the GTi) but finite enough that a stuck driver cannot wedge an
/// Alpaca request indefinitely.
const SYNC_SLEW_TIMEOUT: Duration = Duration::from_secs(300);

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
pub(super) fn validate_guide_rate(deg_per_sec: f64) -> ASCOMResult<f64> {
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

impl MountDevice {
    /// Map a [`StarAdvError`] to its ASCOM equivalent. Used by every
    /// trait method that hits the transport / coordinate layer.
    pub(super) fn ascom(e: StarAdvError) -> ASCOMError {
        e.to_ascom_error()
    }

    /// Map a `SessionError<SkywatcherCodecError>` (from
    /// `SharedTransport::acquire`) into the closest ASCOM error.
    pub(super) fn ascom_session_err(
        err: SessionError<crate::codec::SkywatcherCodecError>,
    ) -> ASCOMError {
        StarAdvError::from(err).to_ascom_error()
    }

    /// Map a `TransportError` (from `Session::close`) into the closest
    /// ASCOM error. The shared-transport teardown is best-effort —
    /// any failure here surfaces to the ASCOM caller rather than
    /// being swallowed by a `tracing::warn!` (the pre-migration
    /// pattern, removed by the Phase E migration).
    pub(super) fn ascom_transport_err(err: TransportError) -> ASCOMError {
        StarAdvError::from(SessionError::<crate::codec::SkywatcherCodecError>::Transport(err))
            .to_ascom_error()
    }

    /// Validate an RA value (hours, [0, 24)) and a Dec value (degrees,
    /// [-90, +90]), returning `INVALID_VALUE` when either is out of range.
    pub(super) fn validate_coordinates(ra_hours: f64, dec_degrees: f64) -> ASCOMResult<()> {
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
    /// would land inside the CW exclusion zone.
    ///
    /// **Why:** the Star Adventurer GTi has a mechanical safety
    /// constraint — the counterweights must not rise more than 0.95 h
    /// above horizontal at any point. The arc where the CW exceeds
    /// that threshold is the configured CW exclusion zone (defaults
    /// `(0.95, 11.05)` h of mech_HA). Slewing the OTA past it stalls
    /// the motor against a hard stop while the encoder counter
    /// continues to advance — on a real-hardware ConformU run that
    /// drove the mount into the counterweight-up region we heard the
    /// motor whine and saw the axis stop physically for several
    /// seconds at a time. The 2026-05-17 San Diego session
    /// additionally demonstrated OTA-vs-tripod contact when the
    /// previously-narrower zone let the CW sweep through the
    /// ascending half.
    ///
    /// The check is in **encoder `mech_HA` space** (signed hours
    /// folded to `[−12, +12)`). For a target on the natural pier
    /// side, `target_mech_HA = celestial_HA`. For a flipped target,
    /// `target_mech_HA = celestial_HA + 12 h` folded. The interval
    /// `(binding_zone_min_hours, binding_zone_max_hours)` is **open**
    /// — a target landing exactly on a zone boundary is permitted (and
    /// matches the open-interval convention [`super::slew::check_non_flip_ra_path`]
    /// uses for path checks). An empty zone (`min >= max`) disables
    /// the check, matching the same path-check convention.
    ///
    /// Note: this is the *destination-only* leg of the safety model.
    /// Path crossings are handled separately by
    /// [`super::slew::check_non_flip_ra_path`] (non-flip slews) and
    /// [`super::slew::flip_slew_ra_delta`] (flip slews); a target
    /// outside the zone with a sweep that crosses the zone is caught
    /// there, not here. The combination — destination check plus path
    /// check — is the safety floor.
    ///
    /// `flip_policy.flip_range_hours` is **not** consulted here — that
    /// rule lives in `select_pier_side_for_target` for pier-side
    /// preference only. Park 1 / Park 5 (anti-meridian poses with
    /// `mech_HA = ±12` on the chosen pier) are reachable via slew
    /// because their mech_HA is outside the CW exclusion zone.
    ///
    /// Both axes are validated together so a partial-failure slew
    /// can't issue motion on RA before discovering Dec is out of
    /// range.
    pub(super) fn check_within_safe_envelope(
        &self,
        ra_hours: f64,
        dec_degrees: f64,
        lst_hours: f64,
        target_is_flipped: bool,
    ) -> ASCOMResult<()> {
        let target_mech_ha = {
            let normal = ra_to_mechanical_ha(ra_hours, lst_hours);
            if target_is_flipped {
                fold_ha(normal + 12.0)
            } else {
                normal
            }
        };
        let zone_min = self.config.binding_zone_min_hours;
        let zone_max = self.config.binding_zone_max_hours;
        // Open interval: target landing exactly on a boundary is OK.
        // Disable on `min >= max` (empty zone), matching the path
        // check's convention so destination and path checks agree on
        // which configurations are "disabled."
        if zone_min < zone_max && target_mech_ha > zone_min && target_mech_ha < zone_max {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!(
                    "target mech_HA {target_mech_ha:.3} h is inside the CW exclusion zone \
                     ({zone_min}, {zone_max}) h"
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
    pub(super) async fn ensure_unparked(&self) -> ASCOMResult<()> {
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
    pub(super) async fn ensure_connected(&self) -> ASCOMResult<()> {
        if !self.connected().await? {
            Err(ASCOMError::NOT_CONNECTED)
        } else {
            Ok(())
        }
    }

    /// `ASCOMResult` wrapper over the free-function
    /// [`super::slew::stop_axis_and_wait`] — issues `:K<axis>` and
    /// polls `:f<axis>` until the running flag clears. Used by every
    /// `MountDevice` caller that needs ASCOM error mapping:
    /// `set_tracking(true)`, `park`, and the per-axis stop preceding
    /// each `issue_slew_axis` in `slew_to_coordinates_async`. The
    /// slew-completion and park watchers run inside spawned tasks and
    /// call `stop_axis_and_wait` directly (no `MountDevice` to wrap
    /// the error).
    pub(super) async fn stop_and_wait(&self, axis: Axis) -> ASCOMResult<()> {
        let guard = self.session.read().await;
        let session = guard
            .as_ref()
            .ok_or_else(|| Self::ascom(StarAdvError::NotConnected))?;
        stop_axis_and_wait(&self.manager, session, axis, AXIS_STOP_TIMEOUT)
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
    pub(super) async fn await_slew_complete(&self) -> ASCOMResult<()> {
        let poll = self.manager.polling_interval_for_watcher();
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
    ///    `seed_home_pose_after_connect` runs first in `set_connected`
    ///    so the snapshot already reflects the home_pose's logical
    ///    encoder values on a fresh power-up; a mid-session reconnect
    ///    (firmware encoder non-zero) leaves the snapshot at the
    ///    handshake reading, which is the "park where the OTA already
    ///    is" semantic operators expect from a reconnect.
    ///
    /// Extracted from `set_connected` so a failure here (file missing,
    /// malformed JSON, lost transport mid-load) can be rolled back by the
    /// caller without leaking the connection ref-count. See the design
    /// doc's §"Park lifecycle" for the resolution rules.
    pub(super) async fn load_park_target_after_connect(
        &self,
        _session: &Session<SkywatcherCodec>,
    ) -> ASCOMResult<()> {
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
        let snap = self.manager.snapshot().await;
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

    /// Post-connect encoder seed for the operator's configured `HomePose`.
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
    pub(super) async fn seed_home_pose_after_connect(
        &self,
        session: &Session<SkywatcherCodec>,
    ) -> ASCOMResult<()> {
        let Some(home_pose) = self.config.home_pose else {
            // No pose configured — trust the firmware encoder as-is.
            // This is the codebase's historical (pre-Phase-6) behaviour
            // and what existing pre-`home_pose` config files expect.
            return Ok(());
        };
        let params = self
            .manager
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let snap = self.manager.snapshot().await;
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
        self.manager
            .send(
                session,
                Command::SetPosition {
                    axis: Axis::Ra,
                    ticks: ra_ticks,
                },
            )
            .await
            .map_err(Self::ascom)?;
        self.manager.seed_ra_position(ra_ticks).await;
        self.manager
            .send(
                session,
                Command::SetPosition {
                    axis: Axis::Dec,
                    ticks: dec_ticks,
                },
            )
            .await
            .map_err(Self::ascom)?;
        self.manager.seed_dec_position(dec_ticks).await;
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
    ///   3. Computes deltas with `fold_to_canonical_band` (handles a
    ///      post-through-wrap raw encoder) and applies
    ///      `flip_slew_ra_delta` on the RA axis for flip slews
    ///      (forces CCW direction through the safe negative-mech_HA
    ///      half).
    ///   4. Issues the INDI wire sequence per axis and hands off to
    ///      the slew-completion watcher.
    pub(super) async fn execute_slew_with_explicit_side(
        &self,
        ra: f64,
        dec: f64,
        chosen_side: PierSide,
    ) -> ASCOMResult<()> {
        let params = self
            .manager
            .parameters()
            .await
            .ok_or(ASCOMError::NOT_CONNECTED)?;
        let lst = local_sidereal_time_hours(SystemTime::now(), self.config.site_longitude_deg)
            .map_err(Self::ascom)?;
        let pre_flip_side = pre_flip_side_for_latitude(self.config.site_latitude_deg);
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
            let snap = self.manager.snapshot().await;
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
                fold_to_canonical_band(ra_ticks - snap.ra.position_ticks, params.cpr_ra);
            let binding_zone = (
                self.config.binding_zone_min_hours,
                self.config.binding_zone_max_hours,
            );
            let ra_delta = if is_flip_slew {
                // Flip slews steer the polar-axis sweep out of the
                // CW exclusion zone — see
                // [`super::slew::flip_slew_ra_delta`] and the design doc's
                // [§"Through-wrap slew routing"](../../../../docs/services/star-adventurer-gti.md#through-wrap-slew-routing).
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
                // pier side. Refuse if that sweep enters the CW
                // exclusion zone (e.g. cur mech_HA = +0.5 h → target +11.5 h
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
                fold_to_canonical_band(dec_ticks - snap.dec.position_ticks, params.cpr_dec);
            let dec_delta = if is_flip_slew {
                // Flip slews force the Dec axis to traverse the
                // visible celestial pole (NCP for north, SCP for
                // south) rather than the below-horizon pole — see
                // [`super::slew::flip_slew_dec_delta`].
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
            let guard = self.session.read().await;
            let session = guard
                .as_ref()
                .ok_or_else(|| Self::ascom(StarAdvError::NotConnected))?;
            self.stop_and_wait(Axis::Ra).await?;
            self.state.write().await.tracking_requested = false;
            issue_slew_axis(&self.manager, session, Axis::Ra, ra_delta)
                .await
                .map_err(Self::ascom)?;
            self.stop_and_wait(Axis::Dec).await?;
            issue_slew_axis(&self.manager, session, Axis::Dec, dec_delta)
                .await
                .map_err(Self::ascom)?;
            Ok(())
        }
        .await;
        if let Err(e) = result {
            self.state.write().await.slew_in_progress = false;
            return Err(e);
        }

        // Hand off to the completion watcher. The watcher acquires its
        // own session so the user's disconnect path doesn't have to
        // wait for slews to finish — see `spawn_slew_completion_watcher`.
        let settle = {
            let s = self.state.read().await;
            s.slew_settle_time.unwrap_or(self.config.settle_after_slew)
        };
        spawn_slew_completion_watcher(
            Arc::clone(&self.state),
            Arc::clone(&self.manager),
            Arc::clone(&self.session),
            self.config.clone(),
            self.manager.polling_interval_for_watcher(),
            settle,
            tracking_was_on,
        )
        .await
        .map_err(Self::ascom)?;
        Ok(())
    }
}
