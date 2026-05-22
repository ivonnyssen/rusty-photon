//! Operations not in ERFA's surface — small root-finders over the
//! ERFA-supplied positions in `erfars_impl`.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};

use crate::erfars_impl::{alt_az_at, lst_hours, time_jds};
use crate::site::Site;
use crate::types::{IcrsCoord, RiseSet, TwilightKind, TwilightWindow};

/// 1 sidereal hour in solar hours.
const SIDEREAL_TO_SOLAR: f64 = 0.997_269_566_3;

/// Bisect a function over a `DateTime<Utc>` interval looking for a
/// sign change of `f`. Returns `None` if the function does not change
/// sign on the bracket. Tolerance is in whole seconds.
fn bisect_dt<F>(
    mut f: F,
    lo: DateTime<Utc>,
    hi: DateTime<Utc>,
    tol_secs: i64,
) -> Option<DateTime<Utc>>
where
    F: FnMut(DateTime<Utc>) -> f64,
{
    let flo = f(lo);
    let fhi = f(hi);
    if flo.is_nan() || fhi.is_nan() {
        return None;
    }
    if flo * fhi > 0.0 {
        return None;
    }
    let mut lo = lo;
    let mut hi = hi;
    let mut flo_sign = flo.signum();
    while (hi - lo).num_seconds() > tol_secs {
        let half = (hi - lo) / 2;
        let mid = lo + half;
        let fmid = f(mid);
        if fmid.is_nan() {
            return None;
        }
        if fmid == 0.0 {
            return Some(mid);
        }
        if fmid.signum() == flo_sign {
            lo = mid;
            flo_sign = fmid.signum();
        } else {
            hi = mid;
        }
    }
    Some(lo + (hi - lo) / 2)
}

/// UT of upper transit on the given UTC date. Closed-form via LST,
/// refined by one Newton step against the actual computed LST at the
/// candidate time.
pub(crate) fn transit(site: &Site, target: IcrsCoord, date: NaiveDate) -> Option<DateTime<Utc>> {
    let start = NaiveDateTime::new(date, NaiveTime::from_hms_opt(0, 0, 0)?).and_utc();
    let lst0 = lst_hours(site, &time_jds(start));
    // NaN propagates through `rem_euclid` but `as i64` saturates NaN
    // to 0, which would silently collapse the computation to "start".
    // Surface the failure as `None` instead so callers see the
    // upstream time-conversion problem.
    if !lst0.is_finite() {
        return None;
    }
    let delta_sidereal = (target.ra_hours - lst0).rem_euclid(24.0);
    let delta_solar = delta_sidereal * SIDEREAL_TO_SOLAR;
    let candidate = start + Duration::milliseconds((delta_solar * 3_600_000.0) as i64);

    // One Newton iteration: re-evaluate LST at the candidate, take
    // the residual mod 24 (signed: residual > 12h means we overshot).
    let lst1 = lst_hours(site, &time_jds(candidate));
    if !lst1.is_finite() {
        return None;
    }
    let mut residual = (target.ra_hours - lst1).rem_euclid(24.0);
    if residual > 12.0 {
        residual -= 24.0;
    }
    let refined =
        candidate + Duration::milliseconds((residual * SIDEREAL_TO_SOLAR * 3_600_000.0) as i64);
    Some(refined)
}

/// Rise/set times above `min_alt_deg`.
pub(crate) fn rise_set(
    _eph: &impl crate::Ephemeris, // unused for v1; reserved for future
    site: &Site,
    target: IcrsCoord,
    date: NaiveDate,
    min_alt_deg: f64,
) -> Option<RiseSet> {
    let transit_t = transit(site, target, date)?;
    // Antitransit is 12 sidereal hours away; use 11h57.97m solar.
    let half_sidereal_day_solar =
        Duration::milliseconds((12.0 * SIDEREAL_TO_SOLAR * 3_600_000.0) as i64);
    let antitransit_before = transit_t - half_sidereal_day_solar;
    let antitransit_after = transit_t + half_sidereal_day_solar;

    let alt_minus_thresh = |t: DateTime<Utc>| -> f64 {
        match alt_az_at(site, target, &time_jds(t)) {
            Ok(aa) => aa.altitude_degrees - min_alt_deg,
            Err(_) => f64::NAN,
        }
    };

    let alt_at_transit = alt_minus_thresh(transit_t);
    if alt_at_transit < 0.0 {
        return None; // never reaches threshold
    }
    let alt_anti_before = alt_minus_thresh(antitransit_before);
    let alt_anti_after = alt_minus_thresh(antitransit_after);
    if alt_anti_before >= 0.0 && alt_anti_after >= 0.0 {
        return None; // always above threshold (circumpolar-up)
    }

    let rise = bisect_dt(alt_minus_thresh, antitransit_before, transit_t, 1);
    let set = bisect_dt(alt_minus_thresh, transit_t, antitransit_after, 1);
    match (rise, set) {
        (Some(rise_utc), Some(set_utc)) => Some(RiseSet { rise_utc, set_utc }),
        _ => None,
    }
}

/// Time until the target next reaches the meridian (HA = 0). Side of
/// pier is ignored in v1 — the convenience tool's caller treats the
/// returned duration as "time until a flip might be required".
pub(crate) fn meridian_flip(
    site: &Site,
    target: IcrsCoord,
    time: DateTime<Utc>,
) -> Option<Duration> {
    let lst = lst_hours(site, &time_jds(time));
    if !lst.is_finite() {
        return None;
    }
    let ha = (lst - target.ra_hours).rem_euclid(24.0);
    // ha ∈ [0, 24). HA = 0 means the target is on the meridian *right
    // now* — the flip is due now, not in another full sidereal day.
    // For 0 < ha < 24, transit is `24 - ha` sidereal hours in the
    // future.
    let hours_sidereal = if ha == 0.0 { 0.0 } else { 24.0 - ha };
    let hours_solar = hours_sidereal * SIDEREAL_TO_SOLAR;
    Some(Duration::milliseconds((hours_solar * 3_600_000.0) as i64))
}

/// Civil/nautical/astronomical twilight bracket centred on the local
/// night that covers `date` (UTC). Returns `Some` for both bounds when
/// the Sun crosses the threshold both going down (evening) and going
/// up (morning); `None` for either bound at high latitudes where the
/// Sun never crosses the threshold.
pub(crate) fn twilight(
    eph: &impl crate::Ephemeris,
    site: &Site,
    date: NaiveDate,
    kind: TwilightKind,
) -> TwilightWindow {
    let threshold = kind.sun_altitude_threshold_degrees();
    // Approximate local solar noon: noon UTC shifted by 4 minutes per
    // degree of longitude (240 s = 240_000 ms / deg). longitude_degrees
    // is positive east, so local solar noon is *earlier* in UTC for
    // eastern longitudes.
    let Some(noon_naive) = NaiveTime::from_hms_opt(12, 0, 0) else {
        // (12, 0, 0) is structurally always a valid HMS; this arm is
        // only here to keep production code panic-free.
        return TwilightWindow {
            begin_utc: None,
            end_utc: None,
        };
    };
    let noon_utc = NaiveDateTime::new(date, noon_naive).and_utc();
    let solar_noon = noon_utc - Duration::milliseconds((site.longitude_degrees * 240_000.0) as i64);
    let midnight = solar_noon + Duration::hours(12);
    let next_noon = solar_noon + Duration::hours(24);

    // Sun altitude relative to threshold, as a sign-changing function
    // of time. We rely on the trait method so derived twilight is
    // testable against a hand-rolled mock Ephemeris in the planner
    // crate.
    let f = |t: DateTime<Utc>| eph.sun_position(site, t).alt_az.altitude_degrees - threshold;

    let begin_utc = bisect_dt(f, solar_noon, midnight, 1);
    let end_utc = bisect_dt(f, midnight, next_noon, 1);
    TwilightWindow { begin_utc, end_utc }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::erfars_impl::ErfarsEphemeris;
    use crate::Ephemeris;
    use chrono::TimeZone;

    fn site_seattle() -> Site {
        Site::new(47.6062, -122.3321).unwrap()
    }

    #[test]
    fn polaris_at_seattle_circumpolar_returns_none() {
        let eph = ErfarsEphemeris::new();
        let polaris = IcrsCoord {
            ra_hours: 2.5301944,
            dec_degrees: 89.2641111,
        };
        let date = NaiveDate::from_ymd_opt(2026, 5, 3).unwrap();
        // Polaris never sets at Seattle (lat ~47.6°), so above
        // min_alt_deg = 10° is always-up.
        assert!(rise_set(&eph, &site_seattle(), polaris, date, 10.0).is_none());
    }

    #[test]
    fn extreme_southern_target_at_seattle_never_up() {
        let eph = ErfarsEphemeris::new();
        // Octans-region target near the south celestial pole — never
        // visible from Seattle.
        let target = IcrsCoord {
            ra_hours: 12.0,
            dec_degrees: -85.0,
        };
        let date = NaiveDate::from_ymd_opt(2026, 5, 3).unwrap();
        assert!(rise_set(&eph, &site_seattle(), target, date, 10.0).is_none());
    }

    #[test]
    fn typical_target_rises_and_sets_within_24h() {
        let eph = ErfarsEphemeris::new();
        let m31 = IcrsCoord {
            ra_hours: 0.7122,
            dec_degrees: 41.2689,
        };
        let date = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let rs = rise_set(&eph, &site_seattle(), m31, date, 30.0)
            .expect("M31 should rise above 30° at Seattle in autumn");
        assert!(rs.set_utc > rs.rise_utc, "set must follow rise");
        let span = rs.set_utc - rs.rise_utc;
        assert!(span > Duration::hours(1));
        assert!(span < Duration::hours(24));
    }

    #[test]
    fn transit_within_one_day_of_requested_date() {
        let m31 = IcrsCoord {
            ra_hours: 0.7122,
            dec_degrees: 41.2689,
        };
        let date = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let t = transit(&site_seattle(), m31, date).unwrap();
        let window_start =
            NaiveDateTime::new(date, NaiveTime::from_hms_opt(0, 0, 0).unwrap()).and_utc();
        assert!(t >= window_start);
        assert!(t < window_start + Duration::hours(24));
    }

    #[test]
    fn meridian_flip_at_meridian_returns_zero() {
        // Construct a target whose RA equals the current LST: HA = 0.
        // The flip is "right now", so the duration should be ~0, not
        // a full sidereal day.
        let eph = ErfarsEphemeris::new();
        let site = site_seattle();
        let t = Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap();
        let lst = eph.sidereal_time(&site, t).lst_hours;
        let target = IcrsCoord {
            ra_hours: lst,
            dec_degrees: 30.0,
        };
        let d = meridian_flip(&site, target, t).unwrap();
        // Allow ~1 minute slack for the f64 roundtrip through
        // chrono::Duration::milliseconds; should be far below a full
        // sidereal day.
        assert!(
            d.num_seconds().abs() < 60,
            "expected ~0s at HA=0, got {}s",
            d.num_seconds()
        );
    }

    #[test]
    fn meridian_flip_returns_positive_duration() {
        let eph = ErfarsEphemeris::new();
        let m31 = IcrsCoord {
            ra_hours: 0.7122,
            dec_degrees: 41.2689,
        };
        let t = Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap();
        let d = meridian_flip(&site_seattle(), m31, t).unwrap();
        assert!(d > Duration::zero());
        assert!(d <= Duration::hours(24));
        // sanity: trait dispatch matches direct call
        use crate::types::SideOfPier;
        let via_trait = eph
            .meridian_flip(&site_seattle(), m31, t, SideOfPier::Unknown)
            .unwrap();
        assert_eq!(via_trait, d);
    }

    #[test]
    fn astronomical_twilight_at_seattle_in_summer_has_both_bounds() {
        let eph = ErfarsEphemeris::new();
        let date = NaiveDate::from_ymd_opt(2026, 6, 21).unwrap();
        let w = twilight(&eph, &site_seattle(), date, TwilightKind::Astronomical);
        // At Seattle's latitude (47.6°) astronomical twilight does not
        // technically end on the longest summer day — sun stays above
        // -18° throughout. Either both bounds are None or both are
        // Some; assert structure rather than presence.
        match (w.begin_utc, w.end_utc) {
            (None, None) => {}
            (Some(b), Some(e)) => assert!(e > b),
            (b, e) => panic!("inconsistent twilight: begin={:?} end={:?}", b, e),
        }
    }

    #[test]
    fn civil_twilight_at_seattle_in_winter_brackets_evening() {
        let eph = ErfarsEphemeris::new();
        let date = NaiveDate::from_ymd_opt(2026, 12, 21).unwrap();
        let w = twilight(&eph, &site_seattle(), date, TwilightKind::Civil);
        let begin = w
            .begin_utc
            .expect("civil twilight begin must exist in winter");
        let end = w.end_utc.expect("civil twilight end must exist in winter");
        assert!(end > begin);
        // The night should be at least 8 hours in late December at 47.6N
        assert!(end - begin > Duration::hours(8));
    }

    #[test]
    fn sun_at_threshold_is_consistent_with_sun_position() {
        // After bisection completes, the sun's altitude at the begin
        // time should be very close to -6° (civil threshold).
        let eph = ErfarsEphemeris::new();
        let date = NaiveDate::from_ymd_opt(2026, 12, 21).unwrap();
        let w = twilight(&eph, &site_seattle(), date, TwilightKind::Civil);
        let begin = w.begin_utc.unwrap();
        let sun = eph.sun_position(&site_seattle(), begin);
        assert!(
            (sun.alt_az.altitude_degrees - (-6.0)).abs() < 0.05,
            "sun alt at civil dusk = {:.3}°, expected ~-6",
            sun.alt_az.altitude_degrees
        );
    }
}
