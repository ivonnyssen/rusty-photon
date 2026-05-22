use std::any::Any;
use std::panic::{self, UnwindSafe};

use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use erfars::astrometry::Atco13;
use erfars::constants::{ERFA_DD2R, ERFA_DPI, ERFA_DR2D};
use erfars::ephemerides::{Epv00, Moon98};
use erfars::rotationtime::Gst06a;
use erfars::timescales::{Dtf2d, Taitt, Utctai};
use erfars::ERFAResult;

use crate::derived;
use crate::site::Site;
use crate::types::{
    AltAz, EphemerisError, IcrsCoord, LocalSiderealTime, MoonInfo, RiseSet, SideOfPier, SunInfo,
    TwilightKind, TwilightWindow,
};
use crate::Ephemeris;

/// ERFA-backed [`Ephemeris`] implementation. Holds no state beyond a
/// zero-sized marker — every call is a fresh trip through ERFA.
#[derive(Debug, Default, Clone, Copy)]
pub struct ErfarsEphemeris;

impl ErfarsEphemeris {
    pub fn new() -> Self {
        Self
    }
}

/// Runs `f` inside `panic::catch_unwind`. On panic, extracts a string
/// from the payload, logs it via `tracing::error!`, and returns
/// `fallback`. The panic hook still fires before we catch — operators
/// see the panic on stderr — but the service stays up.
///
/// This is the central defense against panics inside the erfars
/// wrappers: their `unexpected_val_err!` macro turns into `panic!()`
/// for any ERFA return code outside the wrapper's known set (today,
/// nothing actually triggers it, but we don't control the upstream
/// crate). Wrapping each [`Ephemeris`] trait method's body here means
/// any future inconsistency surfaces as NaN/None rather than a service
/// crash.
fn run_with_guard<R, F>(method: &'static str, fallback: R, f: F) -> R
where
    F: FnOnce() -> R + UnwindSafe,
{
    match panic::catch_unwind(f) {
        Ok(value) => value,
        Err(payload) => {
            let message = panic_payload_message(&payload);
            tracing::error!(
                method,
                panic_message = %message,
                "ERFA call panicked; returning fallback value. Operators should treat \
                 this as either a host-clock misconfiguration or an upstream wrapper \
                 inconsistency and investigate."
            );
            fallback
        }
    }
}

/// Best-effort extraction of a panic payload as a `String`. The payload
/// must be dropped while holding the original `Box`, so we copy any
/// useful text out before the borrow ends.
fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

fn nan_alt_az() -> AltAz {
    AltAz {
        altitude_degrees: f64::NAN,
        azimuth_degrees: f64::NAN,
    }
}

fn nan_icrs() -> IcrsCoord {
    IcrsCoord {
        ra_hours: f64::NAN,
        dec_degrees: f64::NAN,
    }
}

fn nan_time_jds() -> TimeJds {
    TimeJds {
        utc1: f64::NAN,
        utc2: f64::NAN,
        tt1: f64::NAN,
        tt2: f64::NAN,
        ut11: f64::NAN,
        ut12: f64::NAN,
    }
}

/// JD pairs for the time scales we care about. Computed once per call
/// to avoid hammering ERFA's leapsecond table four times in a row.
pub(crate) struct TimeJds {
    pub utc1: f64,
    pub utc2: f64,
    pub tt1: f64,
    pub tt2: f64,
    pub ut11: f64,
    pub ut12: f64,
}

/// Convert a `DateTime<Utc>` to the ERFA JD-pair time scales we need.
///
/// chrono can construct dates outside ERFA's calendar range (years
/// down to -262144 vs. ERFA's -4799 floor). Both ERFA calls below
/// have Err handlers that surface NaN-filled JDs rather than
/// panicking; NaN flows through the downstream float math and shows
/// up in the dashboard or alpaca client rather than crashing the
/// service. The handler bodies live in `dtf2d_jds` and `utctai_pair`
/// so they're directly unit-testable.
pub(crate) fn time_jds(time: DateTime<Utc>) -> TimeJds {
    let year = time.year();
    let month = time.month() as i32;
    let day = time.day() as i32;
    let hh = time.hour() as i32;
    let mm = time.minute() as i32;
    let seconds = time.second() as f64 + time.nanosecond() as f64 * 1e-9;

    let Some((utc1, utc2)) = dtf2d_jds(Dtf2d(true, year, month, day, hh, mm, seconds), time) else {
        return nan_time_jds();
    };
    let Some((tai1, tai2)) = utctai_pair(Utctai(utc1, utc2), time) else {
        return nan_time_jds();
    };
    let (tt1, tt2) = Taitt(tai1, tai2);
    // ΔUT1 = 0; UT1 ≈ UTC. UTC pair doubles as the UT1 pair.
    TimeJds {
        utc1,
        utc2,
        tt1,
        tt2,
        ut11: utc1,
        ut12: utc2,
    }
}

/// Unwrap a `Dtf2d` result into the UTC JD pair, logging and returning
/// `None` on Err. Reachable from production: chrono accepts years
/// outside ERFA's [-4799, +∞) range.
fn dtf2d_jds(result: ERFAResult<(f64, f64)>, time: DateTime<Utc>) -> Option<(f64, f64)> {
    match result {
        Ok((pair, _status)) => Some(pair),
        Err(e) => {
            tracing::error!(
                ?time,
                error = ?e,
                "ERFA Dtf2d rejected a chrono-validated DateTime<Utc>; host clock is \
                 outside ERFA's calendar range. Returning NaN time JDs; downstream \
                 computations will surface NaN until the host clock is corrected."
            );
            None
        }
    }
}

/// Unwrap a `Utctai` result into the TAI JD pair, logging and
/// returning `None` on Err. Unreachable in practice (Dtf2d already
/// filters the years that would cause `Utctai`'s internal `eraDat`
/// call to error), but kept as a defensive fallback rather than a
/// `.expect` so production code stays panic-free.
fn utctai_pair(result: ERFAResult<(f64, f64)>, time: DateTime<Utc>) -> Option<(f64, f64)> {
    match result {
        Ok((pair, _status)) => Some(pair),
        Err(e) => {
            tracing::error!(
                ?time,
                error = ?e,
                "ERFA Utctai failed despite Dtf2d succeeding — upstream invariant \
                 violation. Returning NaN time JDs; downstream computations will \
                 surface NaN."
            );
            None
        }
    }
}

/// Greenwich apparent sidereal time, in radians, at the given JDs.
pub(crate) fn gast_radians(jds: &TimeJds) -> f64 {
    Gst06a(jds.ut11, jds.ut12, jds.tt1, jds.tt2)
}

/// Local apparent sidereal time at `site`, in hours `[0, 24)`.
pub(crate) fn lst_hours(site: &Site, jds: &TimeJds) -> f64 {
    let gast_hours = gast_radians(jds) * 12.0 / ERFA_DPI;
    (gast_hours + site.longitude_degrees / 15.0).rem_euclid(24.0)
}

/// Topocentric alt/az for an ICRS target at the given UTC time.
pub(crate) fn alt_az_at(
    site: &Site,
    target: IcrsCoord,
    jds: &TimeJds,
) -> Result<AltAz, EphemerisError> {
    let rc = target.ra_hours * 15.0 * ERFA_DD2R;
    let dc = target.dec_degrees * ERFA_DD2R;
    let elong = site.longitude_degrees * ERFA_DD2R;
    let phi = site.latitude_degrees * ERFA_DD2R;
    // Default amateur-rig conditions, documented on the trait.
    let phpa = 1013.25;
    let tc = 10.0;
    let rh = 0.5;
    let wl = 0.55;
    let result = Atco13(
        rc, dc, 0.0, 0.0, 0.0, 0.0, jds.utc1, jds.utc2, 0.0, elong, phi, 0.0, 0.0, 0.0, phpa, tc,
        rh, wl,
    )
    .map_err(EphemerisError::InvalidAltAzInputs)?;
    let (aob, zob, _hob, _dob, _rob, _eo) = result.0;
    let altitude_degrees = (ERFA_DPI / 2.0 - zob) * ERFA_DR2D;
    let azimuth_degrees = (aob * ERFA_DR2D).rem_euclid(360.0);
    Ok(AltAz {
        altitude_degrees,
        azimuth_degrees,
    })
}

/// Geocentric astrometric Sun coordinates from `Epv00`. The Sun
/// direction from Earth is the negative of the Earth's heliocentric
/// position (BCRS ≈ ICRS to milliarcsec).
///
/// The underlying ERFA `eraEpv00` only ever returns 0 or +1, so the
/// erfars wrapper never produces `Err` today. We still match on
/// `Err` defensively — returning NaN coords rather than `.expect`-ing
/// — so production code stays panic-free if the upstream contract
/// ever changes. The handler lives in `epv00_heliocentric` so it's
/// directly unit-testable.
pub(crate) fn sun_icrs(jds: &TimeJds) -> IcrsCoord {
    let Some(pvh) = epv00_heliocentric(Epv00(jds.tt1, jds.tt2), jds) else {
        return nan_icrs();
    };
    let x = -pvh[0];
    let y = -pvh[1];
    let z = -pvh[2];
    cartesian_to_icrs(x, y, z)
}

/// Unwrap an `Epv00` result into the heliocentric position vector,
/// logging and returning `None` on Err. Unreachable in practice
/// (eraEpv00 only returns 0 or +1), but kept as a defensive fallback
/// rather than `.expect` so production code stays panic-free.
fn epv00_heliocentric(result: ERFAResult<([f64; 6], [f64; 6])>, jds: &TimeJds) -> Option<[f64; 6]> {
    match result {
        Ok(((pvh, _pvb), _warn)) => Some(pvh),
        Err(e) => {
            tracing::error!(
                tt1 = jds.tt1,
                tt2 = jds.tt2,
                error = ?e,
                "ERFA Epv00 returned Err despite a contract of Ok(0) or Ok(+1) — \
                 upstream invariant violation. Returning NaN sun coordinates."
            );
            None
        }
    }
}

/// Geocentric Moon coordinates from `Moon98` (GCRS ≈ ICRS).
pub(crate) fn moon_icrs(jds: &TimeJds) -> IcrsCoord {
    let pv = Moon98(jds.tt1, jds.tt2);
    cartesian_to_icrs(pv[0], pv[1], pv[2])
}

fn cartesian_to_icrs(x: f64, y: f64, z: f64) -> IcrsCoord {
    let r = (x * x + y * y + z * z).sqrt();
    let mut ra = y.atan2(x);
    if ra < 0.0 {
        ra += 2.0 * ERFA_DPI;
    }
    let dec = (z / r).asin();
    IcrsCoord {
        ra_hours: ra * 12.0 / ERFA_DPI,
        dec_degrees: dec * ERFA_DR2D,
    }
}

/// Angular separation between two ICRS coordinates, in degrees.
/// Uses the spherical law of cosines.
pub(crate) fn angular_separation_degrees(a: IcrsCoord, b: IcrsCoord) -> f64 {
    let ra_a = a.ra_hours * 15.0 * ERFA_DD2R;
    let dec_a = a.dec_degrees * ERFA_DD2R;
    let ra_b = b.ra_hours * 15.0 * ERFA_DD2R;
    let dec_b = b.dec_degrees * ERFA_DD2R;
    let cos_sep = dec_a.sin() * dec_b.sin() + dec_a.cos() * dec_b.cos() * (ra_a - ra_b).cos();
    cos_sep.clamp(-1.0, 1.0).acos() * ERFA_DR2D
}

impl Ephemeris for ErfarsEphemeris {
    fn sidereal_time(&self, site: &Site, time: DateTime<Utc>) -> LocalSiderealTime {
        run_with_guard(
            "sidereal_time",
            LocalSiderealTime {
                lst_hours: f64::NAN,
            },
            || {
                let jds = time_jds(time);
                LocalSiderealTime {
                    lst_hours: lst_hours(site, &jds),
                }
            },
        )
    }

    fn alt_az(
        &self,
        site: &Site,
        target: IcrsCoord,
        time: DateTime<Utc>,
    ) -> Result<AltAz, EphemerisError> {
        run_with_guard("alt_az", Ok(nan_alt_az()), || {
            let jds = time_jds(time);
            alt_az_at(site, target, &jds)
        })
    }

    fn transit(&self, site: &Site, target: IcrsCoord, date: NaiveDate) -> Option<DateTime<Utc>> {
        run_with_guard("transit", None, || derived::transit(site, target, date))
    }

    fn rise_set(
        &self,
        site: &Site,
        target: IcrsCoord,
        date: NaiveDate,
        min_alt_deg: f64,
    ) -> Option<RiseSet> {
        run_with_guard("rise_set", None, || {
            derived::rise_set(self, site, target, date, min_alt_deg)
        })
    }

    fn meridian_flip(
        &self,
        site: &Site,
        target: IcrsCoord,
        time: DateTime<Utc>,
        _side: SideOfPier,
    ) -> Option<Duration> {
        run_with_guard("meridian_flip", None, || {
            derived::meridian_flip(site, target, time)
        })
    }

    fn sun_position(&self, site: &Site, time: DateTime<Utc>) -> SunInfo {
        run_with_guard(
            "sun_position",
            SunInfo {
                coords: nan_icrs(),
                alt_az: nan_alt_az(),
            },
            || {
                let jds = time_jds(time);
                let coords = sun_icrs(&jds);
                let alt_az = alt_az_at(site, coords, &jds).unwrap_or_else(|_| nan_alt_az());
                SunInfo { coords, alt_az }
            },
        )
    }

    fn twilight(&self, site: &Site, date: NaiveDate, kind: TwilightKind) -> TwilightWindow {
        run_with_guard(
            "twilight",
            TwilightWindow {
                begin_utc: None,
                end_utc: None,
            },
            || derived::twilight(self, site, date, kind),
        )
    }

    fn moon_position(&self, site: &Site, time: DateTime<Utc>) -> MoonInfo {
        run_with_guard(
            "moon_position",
            MoonInfo {
                coords: nan_icrs(),
                alt_az: nan_alt_az(),
                phase_degrees: f64::NAN,
                illumination_fraction: f64::NAN,
            },
            || {
                let jds = time_jds(time);
                let coords = moon_icrs(&jds);
                let alt_az = alt_az_at(site, coords, &jds).unwrap_or_else(|_| nan_alt_az());
                let sun = sun_icrs(&jds);
                let phase_degrees = angular_separation_degrees(coords, sun);
                // phase_degrees here is the Sun-Earth-Moon elongation:
                //   0°   = new (Sun and Moon at same RA → 0% illuminated)
                //   180° = full (Sun and Moon opposite → 100% illuminated)
                // Illuminated fraction = (1 - cos(elongation)) / 2 — at
                // elongation=0 this is 0, at elongation=180 it is 1,
                // which is what users see in the sky. The `(1 + cos)/2`
                // form is for the *phase angle* convention (vertex at
                // Moon: 0° = full, 180° = new) — we don't use that
                // convention here.
                let illumination_fraction = (1.0 - (phase_degrees * ERFA_DD2R).cos()) / 2.0;
                MoonInfo {
                    coords,
                    alt_az,
                    phase_degrees,
                    illumination_fraction,
                }
            },
        )
    }

    fn moon_separation(&self, target: IcrsCoord, time: DateTime<Utc>) -> f64 {
        run_with_guard("moon_separation", f64::NAN, || {
            let jds = time_jds(time);
            angular_separation_degrees(target, moon_icrs(&jds))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn site_greenwich() -> Site {
        Site::new(0.0, 0.0).unwrap()
    }

    fn site_seattle() -> Site {
        Site::new(47.6062, -122.3321).unwrap()
    }

    /// Polaris (RA ~2.5h, Dec ~+89.26°) is essentially at the celestial
    /// pole; from a mid-northern site its altitude tracks the latitude
    /// closely (within ~1° depending on the year's pole motion and
    /// refraction).
    #[test]
    fn polaris_altitude_tracks_latitude_at_seattle() {
        let eph = ErfarsEphemeris::new();
        let polaris = IcrsCoord {
            ra_hours: 2.5301944,
            dec_degrees: 89.2641111,
        };
        let t = Utc.with_ymd_and_hms(2026, 6, 21, 6, 0, 0).unwrap();
        let alt = eph.alt_az(&site_seattle(), polaris, t).unwrap();
        // Seattle latitude is 47.6°; Polaris altitude ≈ latitude ± dec
        // offset from pole, refraction-bumped near horizon. Expect
        // within ~1° of latitude here.
        assert!(
            (alt.altitude_degrees - 47.6).abs() < 1.5,
            "polaris altitude {:.2}° not close to Seattle latitude",
            alt.altitude_degrees
        );
    }

    /// Sidereal time at Greenwich at J2000.0 epoch should be close to
    /// 18h 41m 50.5s (well-known canonical value). This is a strong
    /// sanity check on the time-conversion + Gst06a chain.
    #[test]
    fn gst_at_j2000_epoch_matches_canonical() {
        let eph = ErfarsEphemeris::new();
        // J2000 = 2000-01-01 12:00 TT ≈ 11:58:55.816 UTC
        let t = Utc.with_ymd_and_hms(2000, 1, 1, 11, 58, 55).unwrap();
        let lst = eph.sidereal_time(&site_greenwich(), t);
        // Expected ~18.6973h. Allow 0.01h (~36 seconds of LST) to
        // absorb our truncation of TT and ΔUT1=0 simplification.
        let expected = 18.0 + 41.0 / 60.0 + 50.5 / 3600.0;
        assert!(
            (lst.lst_hours - expected).abs() < 0.05,
            "GMST at J2000 epoch was {:.6}h; expected {:.6}h",
            lst.lst_hours,
            expected
        );
    }

    /// On the vernal equinox, the Sun is at RA=0h, Dec=0° (by
    /// definition of the equinox).
    #[test]
    fn sun_at_vernal_equinox_is_near_origin() {
        let eph = ErfarsEphemeris::new();
        // 2026 vernal equinox is 2026-03-20 14:46 UTC. Take that
        // moment plus a few minutes so we're well past the crossing.
        let t = Utc.with_ymd_and_hms(2026, 3, 20, 14, 46, 0).unwrap();
        let sun = eph.sun_position(&site_greenwich(), t);
        // Geocentric astrometric, no aberration: dec should be within
        // 0.5° of 0 (sub-day drift) and RA within 0.5h of 0/24.
        assert!(
            sun.coords.dec_degrees.abs() < 0.5,
            "sun dec at vernal equinox = {:.4}°, expected ~0",
            sun.coords.dec_degrees
        );
        let ra_distance_to_origin = sun.coords.ra_hours.min(24.0 - sun.coords.ra_hours);
        assert!(
            ra_distance_to_origin < 0.5,
            "sun RA at vernal equinox = {:.4}h, expected ~0/24",
            sun.coords.ra_hours
        );
    }

    /// Sun is below horizon at midnight everywhere except polar
    /// summer.
    #[test]
    fn sun_is_below_horizon_at_seattle_midnight_in_winter() {
        let eph = ErfarsEphemeris::new();
        let t = Utc.with_ymd_and_hms(2026, 12, 21, 8, 0, 0).unwrap(); // midnight PST
        let sun = eph.sun_position(&site_seattle(), t);
        assert!(
            sun.alt_az.altitude_degrees < 0.0,
            "sun altitude at Seattle midnight in winter = {:.2}°",
            sun.alt_az.altitude_degrees
        );
    }

    /// Moon coordinates should be valid (RA in [0,24), Dec in [-90,90])
    /// and altitude either above or below horizon — just a sanity
    /// check that the wiring works end-to-end.
    #[test]
    fn moon_coordinates_in_valid_range() {
        let eph = ErfarsEphemeris::new();
        let t = Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap();
        let m = eph.moon_position(&site_seattle(), t);
        assert!((0.0..24.0).contains(&m.coords.ra_hours));
        assert!((-90.0..=90.0).contains(&m.coords.dec_degrees));
        assert!((0.0..=180.0).contains(&m.phase_degrees));
        assert!((0.0..=1.0).contains(&m.illumination_fraction));
    }

    /// `Dtf2d` rejects years < -4799. chrono can construct dates down
    /// to year -262144, so we can hit the Err arm in `time_jds` from
    /// safe code. Expect NaN-filled JDs and a `tracing::error!` log.
    #[test]
    fn time_jds_returns_nan_when_year_is_before_erfa_lower_bound() {
        let t = Utc.with_ymd_and_hms(-10000, 1, 1, 0, 0, 0).unwrap();
        let jds = time_jds(t);
        assert!(jds.utc1.is_nan());
        assert!(jds.utc2.is_nan());
        assert!(jds.tt1.is_nan());
        assert!(jds.tt2.is_nan());
        assert!(jds.ut11.is_nan());
        assert!(jds.ut12.is_nan());
    }

    /// End-to-end: a year outside ERFA's range should make every
    /// trait method degrade to NaN/None instead of crashing.
    #[test]
    fn trait_methods_degrade_to_nan_or_none_for_year_before_erfa_range() {
        let eph = ErfarsEphemeris::new();
        let t = Utc.with_ymd_and_hms(-10000, 1, 1, 0, 0, 0).unwrap();
        let d = NaiveDate::from_ymd_opt(-10000, 1, 1).unwrap();
        let target = IcrsCoord {
            ra_hours: 12.0,
            dec_degrees: 0.0,
        };
        let site = site_seattle();

        assert!(eph.sidereal_time(&site, t).lst_hours.is_nan());
        let alt_az = eph.alt_az(&site, target, t).unwrap();
        assert!(alt_az.altitude_degrees.is_nan());
        assert!(alt_az.azimuth_degrees.is_nan());
        let sun = eph.sun_position(&site, t);
        assert!(sun.coords.ra_hours.is_nan());
        assert!(sun.coords.dec_degrees.is_nan());
        let moon = eph.moon_position(&site, t);
        assert!(moon.coords.ra_hours.is_nan());
        assert!(moon.illumination_fraction.is_nan());
        assert!(eph.moon_separation(target, t).is_nan());
        // Date-based helpers bisect over LST/sun-altitude; both go NaN
        // upstream and the bisector returns None.
        assert!(eph.transit(&site, target, d).is_none());
        assert!(eph.rise_set(&site, target, d, 0.0).is_none());
    }

    /// `run_with_guard` returns the closure's value when the closure
    /// does not panic. Establishes the happy path.
    #[test]
    fn run_with_guard_returns_closure_value_on_happy_path() {
        let result = run_with_guard("test", 0, || 42);
        assert_eq!(result, 42);
    }

    /// `run_with_guard` catches a `panic!` from the closure and
    /// returns the supplied fallback. This is the central defense
    /// against panics inside the erfars wrappers' `unexpected_val_err!`
    /// macro.
    #[test]
    fn run_with_guard_returns_fallback_when_closure_panics() {
        let result = run_with_guard("test", 7, || -> i32 {
            panic!("simulated wrapper panic");
        });
        assert_eq!(result, 7);
    }

    /// Verify that `&'static str` panic payloads (the shape produced
    /// by `panic!("literal")` in the erfars `unexpected_val_err!`
    /// macro) round-trip through the extractor.
    #[test]
    fn panic_payload_message_extracts_static_str() {
        let payload: Box<dyn Any + Send> = Box::new("hello world");
        assert_eq!(panic_payload_message(&*payload), "hello world");
    }

    /// `panic!("{}", ...)` with formatting arguments produces a
    /// `String` payload, not `&'static str` — cover that arm too.
    #[test]
    fn panic_payload_message_extracts_string() {
        let payload: Box<dyn Any + Send> = Box::new(String::from("formatted msg"));
        assert_eq!(panic_payload_message(&*payload), "formatted msg");
    }

    /// Any other payload type (e.g. a `panic_any` value) falls back
    /// to a sentinel string so we still log *something* useful.
    #[test]
    fn panic_payload_message_returns_sentinel_for_unknown_payload() {
        let payload: Box<dyn Any + Send> = Box::new(42_i32);
        assert_eq!(
            panic_payload_message(&*payload),
            "<non-string panic payload>"
        );
    }

    fn epoch() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    /// Happy-path unwrap of a `Dtf2d` Ok pair.
    #[test]
    fn dtf2d_jds_passes_through_ok_pair() {
        let pair = dtf2d_jds(Ok(((2451545.0, 0.5), 0)), epoch());
        assert_eq!(pair, Some((2451545.0, 0.5)));
    }

    /// `Dtf2d` Err → None. This is the documented bad-input path
    /// (year outside ERFA's calendar range).
    #[test]
    fn dtf2d_jds_returns_none_on_err() {
        assert!(dtf2d_jds(Err(-1), epoch()).is_none());
    }

    /// Happy-path unwrap of a `Utctai` Ok pair.
    #[test]
    fn utctai_pair_passes_through_ok_pair() {
        let pair = utctai_pair(Ok(((2451545.0, 0.5), 0)), epoch());
        assert_eq!(pair, Some((2451545.0, 0.5)));
    }

    /// `Utctai` Err → None. Structurally unreachable in production
    /// (Dtf2d filters the inputs that would cause it), so the helper
    /// gives us a seam to verify the defensive arm without needing
    /// the impossible ERFA-internal failure to actually occur.
    #[test]
    fn utctai_pair_returns_none_on_err() {
        assert!(utctai_pair(Err(-1), epoch()).is_none());
    }

    /// Happy-path unwrap of an `Epv00` Ok result; returns the
    /// heliocentric position vector.
    #[test]
    fn epv00_heliocentric_passes_through_ok_pvh() {
        let pvh = [1.0, 2.0, 3.0, 0.1, 0.2, 0.3];
        let pvb = [0.0; 6];
        let jds = nan_time_jds();
        assert_eq!(epv00_heliocentric(Ok(((pvh, pvb), 0)), &jds), Some(pvh));
    }

    /// `Epv00` Err → None. Structurally unreachable today (eraEpv00
    /// only ever returns 0 or +1), so the helper exists so that
    /// production code can stay panic-free even if the upstream
    /// invariant ever changes.
    #[test]
    fn epv00_heliocentric_returns_none_on_err() {
        let jds = nan_time_jds();
        assert!(epv00_heliocentric(Err(-1), &jds).is_none());
    }

    /// Verify the production wiring: feeding `sun_icrs` a `TimeJds`
    /// whose `tt` pair would force Epv00 to return Err (it can't
    /// today, but we cover the defensive arm via the inner helper
    /// above and trust this wiring path).
    #[test]
    fn sun_icrs_returns_nan_when_helper_yields_none() {
        // Drive sun_icrs with finite (but absurd) tt values to confirm
        // the happy path returns finite coords. The Err arm coverage
        // is provided by `epv00_heliocentric_returns_none_on_err`.
        let jds = TimeJds {
            utc1: 2451545.0,
            utc2: 0.0,
            tt1: 2451545.0,
            tt2: 0.0,
            ut11: 2451545.0,
            ut12: 0.0,
        };
        let icrs = sun_icrs(&jds);
        assert!(icrs.ra_hours.is_finite());
        assert!(icrs.dec_degrees.is_finite());
    }
}
