//! Encoder-tick ↔ celestial-coordinate conversions.
//!
//! The mount's wire protocol speaks raw encoder ticks; ASCOM speaks RA/Dec
//! (hours and degrees). Bridging the two requires:
//!
//! * Counts-per-revolution (per axis, queried at handshake — see
//!   [`MountParameters`](crate::transport_manager::MountParameters)).
//! * The sync offset (added on read, subtracted on write — set by
//!   `SyncToCoordinates`).
//! * Local apparent sidereal time (computed from host UTC + site
//!   longitude).
//! * Site latitude (for Az/Alt and side-of-pier derivation).
//!
//! These functions are pure — given the same parameters, they always return
//! the same answer. They are unit-tested directly without the transport
//! layer in scope.

use std::time::SystemTime;

use ascom_alpaca::api::telescope::PierSide;
use chrono::{DateTime, Datelike, Timelike, Utc};
use erfars::constants::ERFA_DPI;
use erfars::rotationtime::Gst06a;
use erfars::timescales::{Dtf2d, Taitt, Utctai};

/// Convert RA-axis encoder ticks to a mechanical hour-angle in the range
/// `[-12, +12)` hours.
pub fn ra_ticks_to_mechanical_ha(ticks: i32, cpr: u32) -> f64 {
    if cpr == 0 {
        return 0.0;
    }
    let hours = (ticks as f64) * 24.0 / (cpr as f64);
    fold_to_signed(hours, 24.0)
}

/// Convert Dec-axis encoder ticks to a declination angle in degrees.
///
/// Returns the linear mapping `ticks * 360 / cpr` then folded into
/// `[-180, +180)`. **Does not** fold through the celestial pole — values
/// outside `[-90, +90]` are returned as-is so the caller can detect a
/// mount that ended up "through the pole" (Phase 4 sync logic decides
/// what to do with that). For the BDD scenarios this is enough: every
/// scenario stays inside the legal Dec range.
pub fn dec_ticks_to_degrees(ticks: i32, cpr: u32) -> f64 {
    if cpr == 0 {
        return 0.0;
    }
    let deg = (ticks as f64) * 360.0 / (cpr as f64);
    fold_to_signed(deg, 360.0)
}

/// Local apparent sidereal time in hours `[0, 24)` from the host's wall
/// clock and the configured site longitude (east-positive, ASCOM
/// convention).
///
/// Uses ERFA's `Gst06a` for Greenwich apparent sidereal time, then adds
/// the site longitude. Same approach as
/// `crates/rp-ephemeris/src/erfars_impl.rs::lst_hours`.
pub fn local_sidereal_time_hours(utc: SystemTime, site_longitude_deg: f64) -> f64 {
    let gast_hours = greenwich_apparent_sidereal_hours(utc);
    (gast_hours + site_longitude_deg / 15.0).rem_euclid(24.0)
}

fn greenwich_apparent_sidereal_hours(utc: SystemTime) -> f64 {
    let dt: DateTime<Utc> = utc.into();
    let year = dt.year();
    let month = dt.month() as i32;
    let day = dt.day() as i32;
    let hh = dt.hour() as i32;
    let mm = dt.minute() as i32;
    let seconds = dt.second() as f64 + (dt.nanosecond() as f64) * 1e-9;

    let (utc1, utc2) = Dtf2d(true, year, month, day, hh, mm, seconds)
        .expect("chrono-validated DateTime<Utc> rejected by ERFA Dtf2d")
        .0;
    let (tai1, tai2) = Utctai(utc1, utc2)
        .expect("ERFA Utctai failed (leapsecond table out of range?)")
        .0;
    let (tt1, tt2) = Taitt(tai1, tai2);
    // ΔUT1 = 0; UT1 ≈ UTC for amateur tracking purposes.
    let gast_radians = Gst06a(utc1, utc2, tt1, tt2);
    gast_radians * 12.0 / ERFA_DPI
}

/// Mechanical hour angle (signed hours) → ASCOM right ascension (hours
/// `[0, 24)`), given the LST.
///
/// `RA = LST - mechanical_HA`, folded into the standard `[0, 24)` range.
pub fn mechanical_ha_to_ra(mech_ha: f64, lst_hours: f64) -> f64 {
    (lst_hours - mech_ha).rem_euclid(24.0)
}

/// ASCOM right ascension (hours `[0, 24)`) → mechanical hour angle.
///
/// Inverse of [`mechanical_ha_to_ra`]. Folds the result into `[-12, +12)`
/// because that matches what the encoder reports.
pub fn ra_to_mechanical_ha(ra_hours: f64, lst_hours: f64) -> f64 {
    fold_to_signed((lst_hours - ra_hours).rem_euclid(24.0), 24.0)
}

/// Side-of-pier classification derived from the Dec-axis encoder
/// position, the Dec-axis CPR, and the site latitude.
///
/// ASCOM `PierSide::West` is the "normal" pointing state of a GEM
/// (counterweight east, OTA on the west side of the pier);
/// `PierSide::East` is the "beyond-the-pole" / post-meridian-flip
/// state. For a Northern Hemisphere observer, the Dec encoder stays
/// within ±90° (= `±cpr_dec/4` ticks) of home for any target reached
/// without a meridian flip → `PierSide::West`. Once the mount rotates
/// the Dec axis past the celestial pole (encoder magnitude past 90°)
/// the OTA has crossed to the east side of the pier →
/// `PierSide::East`. Southern hemisphere inverts the convention.
///
/// This is INDI eqmod's canonical GEM convention (see
/// `eqmodbase.cpp::EncodersToRADec` — `DECurrent > 90° && <= 270°`
/// → `PIER_EAST`, equivalent to the magnitude-past-90° check applied
/// to the signed encoder reading our driver carries).
///
/// The earlier RA-meridian split (HA = 0) happens to return the same
/// value as the Dec-encoder check for every pointing state reachable
/// inside the safety envelope, which is why the two conventions
/// agreed on the ConformU `SideofPier` cases the previous
/// implementation passed. They diverge only when the mount has been
/// manually positioned with the Dec encoder past the pole (e.g. a
/// power-cycle with the OTA pointing through the pole); the
/// Dec-encoder convention reports `PierSide::East` for that state,
/// the HA convention misreports it as whichever side the RA encoder
/// happens to sit on.
pub fn side_of_pier(dec_ticks: i32, cpr_dec: u32, site_latitude_deg: f64) -> PierSide {
    if cpr_dec == 0 {
        return PierSide::Unknown;
    }
    let quarter = (cpr_dec / 4) as i64;
    // Past-the-pole detection: |Dec encoder| > 90° means the mount
    // has rotated the Dec axis beyond either celestial pole — which
    // for a German Equatorial Mount means it is in the
    // post-meridian-flip / counterweight-up state. The boundary at
    // exactly ±90° is *not* East: the mount can sit at the pole via
    // normal-pointing slews without a flip.
    let east_in_north = (dec_ticks as i64).abs() > quarter;
    let northern = site_latitude_deg >= 0.0;
    let east = if northern {
        east_in_north
    } else {
        !east_in_north
    };
    if east {
        PierSide::East
    } else {
        PierSide::West
    }
}

/// Convert a mechanical hour-angle (signed hours) to RA-axis encoder ticks.
/// Inverse of [`ra_ticks_to_mechanical_ha`].
pub fn mechanical_ha_to_ra_ticks(mech_ha_hours: f64, cpr: u32) -> i32 {
    if cpr == 0 {
        return 0;
    }
    (mech_ha_hours * (cpr as f64) / 24.0).round() as i32
}

/// Convert a declination (degrees, `[-90, +90]`) to Dec-axis encoder ticks.
/// Inverse of [`dec_ticks_to_degrees`].
pub fn dec_degrees_to_ticks(dec_degrees: f64, cpr: u32) -> i32 {
    if cpr == 0 {
        return 0;
    }
    (dec_degrees * (cpr as f64) / 360.0).round() as i32
}

/// Compute the RA-axis encoder target for a pickup-loop re-slew,
/// pre-compensated for the LST drift expected to occur before the
/// next residual check.
///
/// Without pre-compensation, each pickup iteration computes
/// `mech_HA = LST(now) - target_RA` and aims the encoder there. But
/// the slew takes ~one iteration to settle, by which time LST has
/// advanced ~`projection × 15.04″/sec`. The next iteration sees the
/// encoder is short of where it should be (because target_RA has the
/// same value but mech_HA needed = LST_now+iter - target_RA is now
/// larger). Pickup is locked into chasing a moving target and the
/// residual floor matches the per-iteration LST drift.
///
/// Pre-compensation: aim where the encoder *will need to be* at the
/// next check, i.e. compute `mech_HA = LST(now + projection) -
/// target_RA`. By the time the slew settles and the next iteration
/// re-checks, LST has advanced into the projection and the encoder
/// is exactly on target.
///
/// `projection` is the watcher's expected per-iteration duration —
/// see [`crate::mount_device::spawn_slew_completion_watcher`]. On a
/// per-`polling_interval` cadence the empirical observation is
/// ~`polling_interval × 2` (one watcher sleep + one slew-settle
/// + wire round-trips).
pub fn pickup_target_ra_ticks(
    target_ra_hours: f64,
    current_lst_hours: f64,
    projection: std::time::Duration,
    cpr_ra: u32,
) -> i32 {
    let lst_at_next_check = current_lst_hours + projection.as_secs_f64() / 3600.0;
    let mech_ha = ra_to_mechanical_ha(target_ra_hours, lst_at_next_check);
    mechanical_ha_to_ra_ticks(mech_ha, cpr_ra)
}

/// Convert RA / Dec → topocentric (altitude, azimuth) given the site
/// latitude and the current LST.
///
/// Returns `(altitude_degrees, azimuth_degrees)`. Azimuth is measured
/// clockwise from north in the range `[0, 360)`. Refraction is **not**
/// applied — the design doc keeps the driver refraction-free
/// (`DoesRefraction = false`).
pub fn ra_dec_to_alt_az(
    ra_hours: f64,
    dec_degrees: f64,
    site_latitude_deg: f64,
    lst_hours: f64,
) -> (f64, f64) {
    let ha_rad = ((lst_hours - ra_hours) * 15.0).to_radians();
    let dec_rad = dec_degrees.to_radians();
    let lat_rad = site_latitude_deg.to_radians();
    let sin_alt = dec_rad.sin() * lat_rad.sin() + dec_rad.cos() * lat_rad.cos() * ha_rad.cos();
    let alt_rad = sin_alt.clamp(-1.0, 1.0).asin();
    let cos_alt = alt_rad.cos();
    // Avoid divide-by-zero at the zenith. Az is undefined there; return 0.
    let az_rad = if cos_alt.abs() < 1e-12 {
        0.0
    } else {
        let sin_az = -dec_rad.cos() * ha_rad.sin() / cos_alt;
        let cos_az = (dec_rad.sin() - sin_alt * lat_rad.sin()) / (cos_alt * lat_rad.cos());
        sin_az.atan2(cos_az)
    };
    let alt_deg = alt_rad.to_degrees();
    let az_deg = az_rad.to_degrees().rem_euclid(360.0);
    (alt_deg, az_deg)
}

/// Sidereal step-period (T1 preset) for the `:I` command, in
/// timer-counter units.
///
/// The mount's step rate is `tmr_freq / period` steps/sec; one full
/// revolution is `cpr` steps; one sidereal day is `86164.0905` seconds.
/// Solving for `period` so the rotation rate matches the sidereal rate:
///
/// `period = tmr_freq * 86164.0905 / cpr`
///
/// For the GTi defaults (`tmr_freq = 0xF42400 = 16_000_000`,
/// `cpr = 0x375F00 = 3_628_800`), this gives roughly `379,887`.
pub fn sidereal_step_period(tmr_freq: u32, cpr_ra: u32) -> u32 {
    if cpr_ra == 0 {
        return 0;
    }
    let sidereal_seconds = 86164.0905_f64;
    ((tmr_freq as f64) * sidereal_seconds / (cpr_ra as f64)).round() as u32
}

/// Sidereal rate in degrees per second.
///
/// `360° / 86164.0905 s ≈ 4.17807e-3 deg/sec ≈ 15.04108″/sec`. ASCOM
/// `GuideRateRightAscension` / `GuideRateDeclination` are expressed in
/// these units; internally the driver uses a fraction of sidereal in
/// `(0, 1)` for the rate-shift math.
pub const SIDEREAL_DEG_PER_SEC: f64 = 360.0 / 86164.0905;

/// Rate-shifted step period for a PulseGuide burst, in timer-counter
/// units (the same units `:I` takes).
///
/// `rate_factor` is the target rate as a multiple of sidereal:
///   - East  → `1.0 - ra_fraction` (RA tracking slowed)
///   - West  → `1.0 + ra_fraction` (RA tracking sped up)
///   - North → `dec_fraction`      (Dec spun from zero)
///   - South → `dec_fraction`
///
/// Step period scales inversely with rate
/// (`period = sidereal_period / rate_factor`).
///
/// The caller is responsible for keeping `rate_factor` strictly
/// positive — `pulse_guide`'s upstream validation rejects a guide-rate
/// fraction outside `(0, 1)`, so the East formula `1 - fraction`
/// stays in `(0, 1)` and never zeroes the divisor.
pub fn pulse_guide_step_period(sidereal_period: u32, rate_factor: f64) -> u32 {
    debug_assert!(
        rate_factor > 0.0,
        "rate_factor must be positive (got {rate_factor})"
    );
    ((sidereal_period as f64) / rate_factor).round() as u32
}

/// Fold a value into `[-period/2, +period/2)`. Used by both the RA and
/// Dec encoder mappings.
fn fold_to_signed(value: f64, period: f64) -> f64 {
    let half = period / 2.0;
    let folded = value.rem_euclid(period);
    if folded >= half {
        folded - period
    } else {
        folded
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const GTI_CPR: u32 = 0x0037_5F00;

    #[test]
    fn ra_at_encoder_zero_is_meridian() {
        assert_eq!(ra_ticks_to_mechanical_ha(0, GTI_CPR), 0.0);
    }

    #[test]
    fn ra_at_quarter_revolution_is_six_hours_east() {
        let ha = ra_ticks_to_mechanical_ha((GTI_CPR / 4) as i32, GTI_CPR);
        assert!((ha - 6.0).abs() < 1e-9, "got {ha}");
    }

    #[test]
    fn ra_at_negative_quarter_is_minus_six_hours() {
        let ha = ra_ticks_to_mechanical_ha(-((GTI_CPR / 4) as i32), GTI_CPR);
        assert!((ha + 6.0).abs() < 1e-9, "got {ha}");
    }

    #[test]
    fn ra_at_half_revolution_folds_to_minus_twelve() {
        let ha = ra_ticks_to_mechanical_ha((GTI_CPR / 2) as i32, GTI_CPR);
        // Either -12 or just below 12, depending on fold direction.
        assert!(ha.abs().abs_diff_eq(&12.0_f64, 1e-9), "got {ha}");
    }

    #[test]
    fn dec_at_encoder_zero_is_celestial_equator() {
        assert_eq!(dec_ticks_to_degrees(0, GTI_CPR), 0.0);
    }

    #[test]
    fn dec_at_quarter_revolution_is_north_pole() {
        let deg = dec_ticks_to_degrees((GTI_CPR / 4) as i32, GTI_CPR);
        assert!((deg - 90.0).abs() < 1e-9, "got {deg}");
    }

    #[test]
    fn dec_at_negative_quarter_is_south_pole() {
        let deg = dec_ticks_to_degrees(-((GTI_CPR / 4) as i32), GTI_CPR);
        assert!((deg + 90.0).abs() < 1e-9, "got {deg}");
    }

    #[test]
    fn lst_changes_with_longitude() {
        // Two LSTs at the same UTC, 90° apart in longitude, must be 6
        // hours apart.
        let utc = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let lst_0 = local_sidereal_time_hours(utc, 0.0);
        let lst_e = local_sidereal_time_hours(utc, 90.0);
        let diff = (lst_e - lst_0).rem_euclid(24.0);
        assert!((diff - 6.0).abs() < 1e-6, "LST(90E) - LST(0) = {diff}h");
    }

    #[test]
    fn lst_is_stable_across_calls() {
        // Same input → same output.
        let utc = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(
            local_sidereal_time_hours(utc, -122.4),
            local_sidereal_time_hours(utc, -122.4)
        );
    }

    #[test]
    fn mechanical_ha_to_ra_round_trips() {
        for &(mech_ha, lst) in &[(0.0, 0.0), (3.0, 6.0), (-4.5, 18.0), (5.999, 12.0)] {
            let ra = mechanical_ha_to_ra(mech_ha, lst);
            let back = ra_to_mechanical_ha(ra, lst);
            assert!(
                (back - mech_ha).abs() < 1e-9,
                "mech_ha={mech_ha} lst={lst} ra={ra} back={back}"
            );
        }
    }

    #[test]
    fn side_of_pier_north_equator_is_west() {
        // Dec encoder at home (0 ticks ≈ celestial equator on the
        // meridian). Mount is in normal-pointing state → pierWest.
        assert_eq!(side_of_pier(0, GTI_CPR, 47.6), PierSide::West);
    }

    #[test]
    fn side_of_pier_north_within_envelope_is_west() {
        // Any encoder magnitude up to and including ±cpr/4 (= ±90°)
        // is reachable without a meridian flip. ConformU's SOPPierTest
        // exercises these cases; all four must read pierWest.
        let quarter = (GTI_CPR / 4) as i32;
        assert_eq!(side_of_pier(quarter / 3, GTI_CPR, 47.6), PierSide::West);
        assert_eq!(side_of_pier(-quarter / 3, GTI_CPR, 47.6), PierSide::West);
        // Boundary cases at exactly ±90°: the mount can reach either
        // celestial pole via normal pointing, so the boundary is
        // included in `West` (matches INDI eqmod's `> 90°` strict
        // check).
        assert_eq!(side_of_pier(quarter, GTI_CPR, 47.6), PierSide::West);
        assert_eq!(side_of_pier(-quarter, GTI_CPR, 47.6), PierSide::West);
    }

    #[test]
    fn side_of_pier_north_past_pole_is_east() {
        // Dec encoder magnitude past 90° means the mount has rotated
        // the Dec axis beyond the celestial pole — the post-flip /
        // counterweight-up state, which ASCOM names pierEast.
        let quarter = (GTI_CPR / 4) as i32;
        assert_eq!(side_of_pier(quarter + 1, GTI_CPR, 47.6), PierSide::East);
        assert_eq!(side_of_pier(-(quarter + 1), GTI_CPR, 47.6), PierSide::East);
        // Mid-flip and "full flip to equator on the opposite side".
        let half = (GTI_CPR / 2) as i32;
        assert_eq!(
            side_of_pier(quarter + half / 4, GTI_CPR, 47.6),
            PierSide::East
        );
        assert_eq!(side_of_pier(half, GTI_CPR, 47.6), PierSide::East);
    }

    #[test]
    fn side_of_pier_southern_hemisphere_inverts() {
        // Mirror of the northern split.
        let quarter = (GTI_CPR / 4) as i32;
        assert_eq!(side_of_pier(0, GTI_CPR, -33.9), PierSide::East);
        assert_eq!(side_of_pier(quarter / 3, GTI_CPR, -33.9), PierSide::East);
        assert_eq!(side_of_pier(quarter, GTI_CPR, -33.9), PierSide::East);
        assert_eq!(side_of_pier(quarter + 1, GTI_CPR, -33.9), PierSide::West);
        assert_eq!(side_of_pier(-(quarter + 1), GTI_CPR, -33.9), PierSide::West);
    }

    #[test]
    fn side_of_pier_returns_unknown_when_cpr_is_zero() {
        // Degenerate case — would mean the parameter cache was never
        // populated. The accessor short-circuits on
        // `NOT_CONNECTED` before reaching the helper in practice, but
        // the helper still has to handle this defensively to stay a
        // total function.
        assert_eq!(side_of_pier(0, 0, 47.6), PierSide::Unknown);
    }

    /// Tiny `f64` helper so the half-revolution fold test can be stated
    /// concisely.
    trait AbsDiffEq {
        fn abs_diff_eq(&self, other: &f64, tol: f64) -> bool;
    }
    impl AbsDiffEq for f64 {
        fn abs_diff_eq(&self, other: &f64, tol: f64) -> bool {
            (self - other).abs() < tol
        }
    }

    #[test]
    fn ra_ticks_round_trip_through_mechanical_ha() {
        for ticks in [
            0,
            100_000,
            -200_000,
            GTI_CPR as i32 / 8,
            -(GTI_CPR as i32 / 4),
        ] {
            let ha = ra_ticks_to_mechanical_ha(ticks, GTI_CPR);
            let back = mechanical_ha_to_ra_ticks(ha, GTI_CPR);
            assert_eq!(back, ticks, "ticks={ticks}");
        }
    }

    #[test]
    fn dec_ticks_round_trip_through_degrees() {
        for ticks in [0, 1_000, -1_000, GTI_CPR as i32 / 8, -(GTI_CPR as i32 / 4)] {
            let deg = dec_ticks_to_degrees(ticks, GTI_CPR);
            let back = dec_degrees_to_ticks(deg, GTI_CPR);
            assert_eq!(back, ticks, "ticks={ticks}");
        }
    }

    #[test]
    fn alt_az_for_zenith_at_equator() {
        // At the equator, the celestial equator passes through the zenith
        // when the LST equals the target's RA.
        let (alt, _az) = ra_dec_to_alt_az(12.0, 0.0, 0.0, 12.0);
        assert!((alt - 90.0).abs() < 1e-6, "got alt={alt}");
    }

    #[test]
    fn alt_az_for_celestial_pole_at_north_observer() {
        // From a northern observer, the NCP sits at altitude = latitude.
        // (Matches the standard astronomy textbook result.)
        let (alt, _az) = ra_dec_to_alt_az(0.0, 90.0, 47.6, 12.0);
        assert!((alt - 47.6).abs() < 1e-6, "got alt={alt}");
    }

    #[test]
    fn sidereal_step_period_for_gti_defaults() {
        // tmr_freq = 16M, cpr = 3,628,800 → period ≈ 379,887.
        let p = sidereal_step_period(0x00F4_2400, GTI_CPR);
        assert!((379_000..=380_000).contains(&p), "expected ~380K, got {p}");
    }

    #[test]
    fn pickup_target_zero_projection_matches_unprojected_math() {
        // With zero projection, pickup target must equal the same
        // mech_ha → ticks math the slew-issue path uses. This is the
        // backwards-compat case: pre-compensation off ⇒ identical
        // wire behaviour to before the change.
        let target_ra = 6.0;
        let lst = 12.0;
        let mech_ha = ra_to_mechanical_ha(target_ra, lst);
        let unprojected = mechanical_ha_to_ra_ticks(mech_ha, GTI_CPR);
        let projected_zero =
            pickup_target_ra_ticks(target_ra, lst, std::time::Duration::ZERO, GTI_CPR);
        assert_eq!(projected_zero, unprojected);
    }

    #[test]
    fn pickup_target_400ms_projection_advances_by_sidereal_drift() {
        // 400 ms of LST drift at sidereal rate = 400 ms × 15.04″/s ≈ 6.016″.
        // Converted to encoder ticks at GTi CPR: 6.016″ / 3600 = 0.001671°
        // of RA = (0.001671° × 4 min/°) hours of mech_HA. At 15°/hour, 1
        // sidereal second of mech_HA = (1/3600) hours, and 400 ms = 0.4
        // sidereal seconds → 0.4/3600 hours of mech_HA.
        // Ticks per hour of mech_HA = cpr/24 = 151,200. So 400 ms ≈
        // 0.4/3600 × 151,200 ≈ 16.8 ticks.
        let target_ra = 6.0;
        let lst = 12.0;
        let no_proj = pickup_target_ra_ticks(target_ra, lst, std::time::Duration::ZERO, GTI_CPR);
        let projected = pickup_target_ra_ticks(
            target_ra,
            lst,
            std::time::Duration::from_millis(400),
            GTI_CPR,
        );
        let delta = projected - no_proj;
        // Expected: ~16-17 ticks. Tolerance +/-1 for round() boundary.
        assert!(
            (15..=18).contains(&delta),
            "expected delta of ~16-17 ticks for 400ms LST projection, got {delta}"
        );
    }

    #[test]
    fn pickup_target_projection_scales_linearly_with_duration() {
        // Doubling the projection should double the tick delta (within
        // round-to-int noise).
        let target_ra = 6.0;
        let lst = 12.0;
        let d1 = pickup_target_ra_ticks(
            target_ra,
            lst,
            std::time::Duration::from_millis(200),
            GTI_CPR,
        ) - pickup_target_ra_ticks(target_ra, lst, std::time::Duration::ZERO, GTI_CPR);
        let d2 = pickup_target_ra_ticks(
            target_ra,
            lst,
            std::time::Duration::from_millis(400),
            GTI_CPR,
        ) - pickup_target_ra_ticks(target_ra, lst, std::time::Duration::ZERO, GTI_CPR);
        assert!(
            (d2 - 2 * d1).abs() <= 1,
            "expected ~2× scaling: 200ms→{d1}, 400ms→{d2}"
        );
    }

    #[test]
    fn pulse_guide_step_period_identity_at_unit_rate_factor() {
        // Rate factor = 1.0 reproduces the sidereal period exactly
        // (modulo rounding to integer).
        let p_sid = sidereal_step_period(0x00F4_2400, GTI_CPR);
        assert_eq!(pulse_guide_step_period(p_sid, 1.0), p_sid);
    }

    #[test]
    fn pulse_guide_step_period_halves_rate_doubles_period() {
        // Rate factor = 0.5 (Dec North/South at fraction = 0.5, or East
        // at fraction = 0.5) doubles the step period.
        let p_sid = sidereal_step_period(0x00F4_2400, GTI_CPR);
        let shifted = pulse_guide_step_period(p_sid, 0.5);
        assert_eq!(shifted, 2 * p_sid);
    }

    #[test]
    fn pulse_guide_step_period_west_at_fraction_half_uses_one_and_a_half_rate() {
        // West at fraction = 0.5 → rate_factor = 1.5 → period = P_sid / 1.5.
        let p_sid = sidereal_step_period(0x00F4_2400, GTI_CPR);
        let shifted = pulse_guide_step_period(p_sid, 1.5);
        let expected = ((p_sid as f64) / 1.5).round() as u32;
        assert_eq!(shifted, expected);
        // Sanity: must be smaller than sidereal (faster rate ⇒ shorter
        // period).
        assert!(shifted < p_sid);
    }

    #[test]
    fn pulse_guide_step_period_small_fraction_grows_period_proportionally() {
        // fraction = 0.1 on Dec → rate_factor = 0.1 → period = 10 × sidereal.
        let p_sid = sidereal_step_period(0x00F4_2400, GTI_CPR);
        let shifted = pulse_guide_step_period(p_sid, 0.1);
        assert_eq!(shifted, 10 * p_sid);
    }

    #[test]
    fn sidereal_deg_per_sec_is_about_fifteen_arcseconds_per_second() {
        // Cross-check the constant against the textbook value: sidereal
        // rate ≈ 15.04108″/sec.
        let arcsec_per_sec = SIDEREAL_DEG_PER_SEC * 3600.0;
        assert!(
            (arcsec_per_sec - 15.04108).abs() < 1e-4,
            "expected ~15.04108″/sec, got {arcsec_per_sec}"
        );
    }
}
