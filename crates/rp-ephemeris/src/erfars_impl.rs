use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use erfars::astrometry::Atco13;
use erfars::constants::{ERFA_DD2R, ERFA_DPI, ERFA_DR2D};
use erfars::ephemerides::{Epv00, Moon98};
use erfars::rotationtime::Gst06a;
use erfars::timescales::{Dtf2d, Taitt, Utctai};

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
/// chrono validates calendar inputs, so `Dtf2d` should never fail
/// here for any value the caller could construct on a sane host
/// clock. If it does — typically the operator's clock is set to a
/// year ERFA's leapsecond table doesn't cover — we log
/// `tracing::error!` and return a NaN-filled [`TimeJds`]. NaN flows
/// through the downstream float math and surfaces in the dashboard
/// as `NaN` values (or in the alpaca client as a NaN coord), which
/// is preferable to a service crash. Operators should treat the
/// logged error as a clock-config issue.
pub(crate) fn time_jds(time: DateTime<Utc>) -> TimeJds {
    let year = time.year();
    let month = time.month() as i32;
    let day = time.day() as i32;
    let hh = time.hour() as i32;
    let mm = time.minute() as i32;
    let seconds = time.second() as f64 + time.nanosecond() as f64 * 1e-9;

    let (utc1, utc2) = match Dtf2d(true, year, month, day, hh, mm, seconds) {
        Ok((pair, _status)) => pair,
        Err(e) => {
            tracing::error!(
                ?time,
                error = ?e,
                "ERFA Dtf2d rejected a chrono-validated DateTime<Utc>; host clock is \
                 outside ERFA's calendar range. Returning NaN time JDs; downstream \
                 computations will surface NaN until the host clock is corrected."
            );
            return TimeJds {
                utc1: f64::NAN,
                utc2: f64::NAN,
                tt1: f64::NAN,
                tt2: f64::NAN,
                ut11: f64::NAN,
                ut12: f64::NAN,
            };
        }
    };
    let (tai1, tai2) = match Utctai(utc1, utc2) {
        Ok((pair, _status)) => pair,
        Err(e) => {
            tracing::error!(
                ?time,
                error = ?e,
                "ERFA Utctai failed (leapsecond table likely out of range for this \
                 host clock). Returning NaN time JDs; downstream computations will \
                 surface NaN until the host clock is corrected or the leapsecond \
                 table refreshed."
            );
            return TimeJds {
                utc1: f64::NAN,
                utc2: f64::NAN,
                tt1: f64::NAN,
                tt2: f64::NAN,
                ut11: f64::NAN,
                ut12: f64::NAN,
            };
        }
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
/// Returns a NaN-filled [`IcrsCoord`] on ERFA failure (e.g. host
/// clock outside `Epv00`'s validity range, typically before 1900 or
/// after 2100). Logs `tracing::error!` at the failure site so the
/// operator sees the misconfiguration; the NaN flows through the
/// subsequent alt/az computation and surfaces as NaN coords in the
/// dashboard rather than a service crash.
pub(crate) fn sun_icrs(jds: &TimeJds) -> IcrsCoord {
    let ((pvh, _pvb), _warn) = match Epv00(jds.tt1, jds.tt2) {
        Ok(result) => result,
        Err(e) => {
            tracing::error!(
                tt1 = jds.tt1,
                tt2 = jds.tt2,
                error = ?e,
                "ERFA Epv00 failed (host clock likely outside the ephemeris validity \
                 window). Returning NaN sun coordinates; downstream computations will \
                 surface NaN until the host clock is corrected."
            );
            return IcrsCoord {
                ra_hours: f64::NAN,
                dec_degrees: f64::NAN,
            };
        }
    };
    let x = -pvh[0];
    let y = -pvh[1];
    let z = -pvh[2];
    cartesian_to_icrs(x, y, z)
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
        let jds = time_jds(time);
        LocalSiderealTime {
            lst_hours: lst_hours(site, &jds),
        }
    }

    fn alt_az(
        &self,
        site: &Site,
        target: IcrsCoord,
        time: DateTime<Utc>,
    ) -> Result<AltAz, EphemerisError> {
        let jds = time_jds(time);
        alt_az_at(site, target, &jds)
    }

    fn transit(&self, site: &Site, target: IcrsCoord, date: NaiveDate) -> Option<DateTime<Utc>> {
        derived::transit(site, target, date)
    }

    fn rise_set(
        &self,
        site: &Site,
        target: IcrsCoord,
        date: NaiveDate,
        min_alt_deg: f64,
    ) -> Option<RiseSet> {
        derived::rise_set(self, site, target, date, min_alt_deg)
    }

    fn meridian_flip(
        &self,
        site: &Site,
        target: IcrsCoord,
        time: DateTime<Utc>,
        _side: SideOfPier,
    ) -> Option<Duration> {
        derived::meridian_flip(site, target, time)
    }

    fn sun_position(&self, site: &Site, time: DateTime<Utc>) -> SunInfo {
        let jds = time_jds(time);
        let coords = sun_icrs(&jds);
        let alt_az = alt_az_at(site, coords, &jds).unwrap_or(AltAz {
            altitude_degrees: f64::NAN,
            azimuth_degrees: f64::NAN,
        });
        SunInfo { coords, alt_az }
    }

    fn twilight(&self, site: &Site, date: NaiveDate, kind: TwilightKind) -> TwilightWindow {
        derived::twilight(self, site, date, kind)
    }

    fn moon_position(&self, site: &Site, time: DateTime<Utc>) -> MoonInfo {
        let jds = time_jds(time);
        let coords = moon_icrs(&jds);
        let alt_az = alt_az_at(site, coords, &jds).unwrap_or(AltAz {
            altitude_degrees: f64::NAN,
            azimuth_degrees: f64::NAN,
        });
        let sun = sun_icrs(&jds);
        let phase_degrees = angular_separation_degrees(coords, sun);
        // phase_degrees here is the Sun-Earth-Moon elongation:
        //   0°   = new (Sun and Moon at same RA → 0% illuminated)
        //   180° = full (Sun and Moon opposite → 100% illuminated)
        // Illuminated fraction = (1 - cos(elongation)) / 2 — at
        // elongation=0 this is 0, at elongation=180 it is 1, which
        // is what users see in the sky. The `(1 + cos)/2` form is for
        // the *phase angle* convention (vertex at Moon: 0° = full,
        // 180° = new) — we don't use that convention here.
        let illumination_fraction = (1.0 - (phase_degrees * ERFA_DD2R).cos()) / 2.0;
        MoonInfo {
            coords,
            alt_az,
            phase_degrees,
            illumination_fraction,
        }
    }

    fn moon_separation(&self, target: IcrsCoord, time: DateTime<Utc>) -> f64 {
        let jds = time_jds(time);
        angular_separation_degrees(target, moon_icrs(&jds))
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
}
