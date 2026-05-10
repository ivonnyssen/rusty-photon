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

/// Side-of-pier classification derived from the RA-axis mechanical hour
/// angle and site latitude.
///
/// In the northern hemisphere, mechanical HA in `[-6, +6)` is the East
/// side (`PierSide::East`); the rest is West. Southern hemisphere
/// inverts (per the Sky-Watcher hand-control spec §"Get Mount Pointing
/// State").
pub fn side_of_pier(mech_ha: f64, site_latitude_deg: f64) -> PierSide {
    let ha = fold_to_signed(mech_ha, 24.0);
    let northern = site_latitude_deg >= 0.0;
    let east_in_north = (-6.0..6.0).contains(&ha);
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
    fn side_of_pier_north_meridian_is_east() {
        // Mechanical HA = 0 (object at meridian) on a northern site →
        // East side. Same convention as the design doc.
        assert_eq!(side_of_pier(0.0, 47.6), PierSide::East);
    }

    #[test]
    fn side_of_pier_north_boundary_is_half_open() {
        // Per the design doc: north [-6, +6) → East. -6 is included
        // (East); +6 is excluded (West, the start of the other half).
        assert_eq!(side_of_pier(-6.0, 47.6), PierSide::East);
        assert_eq!(side_of_pier(6.0, 47.6), PierSide::West);
        // Just inside / just outside the +6 boundary.
        assert_eq!(side_of_pier(5.999, 47.6), PierSide::East);
        assert_eq!(side_of_pier(6.001, 47.6), PierSide::West);
    }

    #[test]
    fn side_of_pier_north_three_hours_east_is_east() {
        assert_eq!(side_of_pier(-3.0, 47.6), PierSide::East);
        assert_eq!(side_of_pier(3.0, 47.6), PierSide::East);
    }

    #[test]
    fn side_of_pier_north_west_arc_is_west() {
        // HA in [+6, +12) maps to the West side.
        assert_eq!(side_of_pier(7.0, 47.6), PierSide::West);
        assert_eq!(side_of_pier(11.999, 47.6), PierSide::West);
    }

    #[test]
    fn side_of_pier_southern_hemisphere_inverts() {
        // Mirror of the northern half-open boundary.
        assert_eq!(side_of_pier(0.0, -33.9), PierSide::West);
        assert_eq!(side_of_pier(-6.0, -33.9), PierSide::West);
        assert_eq!(side_of_pier(6.0, -33.9), PierSide::East);
        assert_eq!(side_of_pier(-3.0, -33.9), PierSide::West);
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
}
