//! Automatic J2000-vs-JNow inference for received GoTo coordinates.
//!
//! The client GoTos a bright cataloged object; we compare the received
//! coordinates against that object's J2000 position and against its
//! apparent-of-date position (ERFA `Atci13`: precession, nutation, annual
//! aberration). ~26 years past J2000 the two differ by ~20 arcminutes, so
//! whichever frame the received value lands on is unambiguous. Objects far
//! from every probe (arbitrary points, stars, planets) yield no verdict.

use chrono::{DateTime, Datelike, Timelike, Utc};
use erfars::astrometry::Atci13;
use erfars::timescales::{Dtf2d, Taitt, Utctai};
use tracing::{info, warn};

/// A separation below this (degrees) reads as "on the probe" …
const NEAR_DEGREES: f64 = 2.0 / 60.0;
/// … and above this as "clearly off it". Between the two: ambiguous.
const FAR_DEGREES: f64 = 10.0 / 60.0;

/// Bright, large-separation objects present in the embedded rp-catalog.
/// A GoTo to any of these gives a clean epoch verdict.
const DEFAULT_PROBES: &[&str] = &[
    "M 31", "M 42", "M 13", "M 51", "M 81", "M 101", "M 104", "M 8", "M 27", "M 57", "NGC 7000",
    "NGC 253",
];

#[derive(Debug, Clone)]
pub struct Probe {
    pub name: String,
    pub ra_hours: f64,
    pub dec_degrees: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochVerdict {
    /// Received coordinates match a probe's J2000 position.
    LooksJ2000,
    /// Received coordinates match a probe's apparent-of-date position.
    LooksApparentOfDate,
    /// No probe close enough in either frame (arbitrary point, star,
    /// planet, or an object not in the probe list).
    NoVerdict,
}

#[derive(Debug, Clone)]
pub struct EpochReport {
    pub verdict: EpochVerdict,
    /// Nearest probe by J2000 separation: (name, separation degrees).
    pub nearest_j2000: Option<(String, f64)>,
    /// Nearest probe by apparent-of-date separation.
    pub nearest_apparent: Option<(String, f64)>,
}

pub struct EpochInference {
    probes: Vec<Probe>,
}

impl EpochInference {
    /// Resolve the built-in probe list plus any extra names against the
    /// embedded catalog; unresolvable names are warned about and skipped.
    pub fn with_default_and_extra_probes(extra: &[String]) -> Self {
        let catalog = rp_catalog::Catalog::embedded();
        let mut probes = Vec::new();
        for name in DEFAULT_PROBES
            .iter()
            .map(|s| (*s).to_owned())
            .chain(extra.iter().cloned())
        {
            match catalog.resolve(&name) {
                Some(resolved) => probes.push(Probe {
                    name: resolved.name.clone(),
                    ra_hours: resolved.coord.ra_hours(),
                    dec_degrees: resolved.coord.dec_degrees(),
                }),
                None => warn!("epoch probe {name:?} not in the embedded catalog; skipped"),
            }
        }
        info!(
            "epoch inference armed with {} probes: {}",
            probes.len(),
            probes
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        Self { probes }
    }

    #[cfg(test)]
    fn from_probes(probes: Vec<Probe>) -> Self {
        Self { probes }
    }

    /// Compare a received GoTo position against every probe in both frames.
    pub fn analyze(&self, ra_hours: f64, dec_degrees: f64, now: DateTime<Utc>) -> EpochReport {
        let mut nearest_j2000: Option<(String, f64)> = None;
        let mut nearest_apparent: Option<(String, f64)> = None;
        for probe in &self.probes {
            let sep_j2000 =
                separation_degrees(ra_hours, dec_degrees, probe.ra_hours, probe.dec_degrees);
            if nearest_j2000.as_ref().is_none_or(|(_, d)| sep_j2000 < *d) {
                nearest_j2000 = Some((probe.name.clone(), sep_j2000));
            }
            if let Some((app_ra, app_dec)) =
                apparent_of_date(probe.ra_hours, probe.dec_degrees, now)
            {
                let sep_app = separation_degrees(ra_hours, dec_degrees, app_ra, app_dec);
                if nearest_apparent.as_ref().is_none_or(|(_, d)| sep_app < *d) {
                    nearest_apparent = Some((probe.name.clone(), sep_app));
                }
            }
        }

        let j2000_sep = nearest_j2000.as_ref().map_or(f64::INFINITY, |(_, d)| *d);
        let apparent_sep = nearest_apparent.as_ref().map_or(f64::INFINITY, |(_, d)| *d);
        let verdict = if j2000_sep < NEAR_DEGREES && apparent_sep > FAR_DEGREES {
            EpochVerdict::LooksJ2000
        } else if apparent_sep < NEAR_DEGREES && j2000_sep > FAR_DEGREES {
            EpochVerdict::LooksApparentOfDate
        } else {
            EpochVerdict::NoVerdict
        };
        EpochReport {
            verdict,
            nearest_j2000,
            nearest_apparent,
        }
    }
}

/// Convert a J2000/ICRS position to apparent place of date (equinox-based
/// RA/Dec: CIRS RA minus the equation of the origins). `None` when the
/// host clock is outside ERFA's calendar range.
pub fn apparent_of_date(ra_hours: f64, dec_degrees: f64, now: DateTime<Utc>) -> Option<(f64, f64)> {
    let (tt1, tt2) = tt_jd(now)?;
    let rc = (ra_hours * 15.0).to_radians();
    let dc = dec_degrees.to_radians();
    let (ri, di, eo) = Atci13(rc, dc, 0.0, 0.0, 0.0, 0.0, tt1, tt2);
    let ra_app_hours = ((ri - eo).to_degrees() / 15.0).rem_euclid(24.0);
    Some((ra_app_hours, di.to_degrees()))
}

/// TT Julian date pair for a UTC instant, per the rp-ephemeris idiom
/// (ΔUT1 ignored; irrelevant at arcminute scale).
fn tt_jd(now: DateTime<Utc>) -> Option<(f64, f64)> {
    let seconds = f64::from(now.second()) + f64::from(now.nanosecond()) * 1e-9;
    let (utc1, utc2) = match Dtf2d(
        true,
        now.year(),
        now.month() as i32,
        now.day() as i32,
        now.hour() as i32,
        now.minute() as i32,
        seconds,
    ) {
        Ok((pair, _status)) => pair,
        Err(e) => {
            warn!("ERFA Dtf2d failed for {now}: {e:?}; no epoch verdict");
            return None;
        }
    };
    let (tai1, tai2) = match Utctai(utc1, utc2) {
        Ok((pair, _status)) => pair,
        Err(e) => {
            warn!("ERFA Utctai failed for {now}: {e:?}; no epoch verdict");
            return None;
        }
    };
    Some(Taitt(tai1, tai2))
}

/// Great-circle separation between two equatorial positions, in degrees.
pub fn separation_degrees(ra1_hours: f64, dec1: f64, ra2_hours: f64, dec2: f64) -> f64 {
    let ra1 = (ra1_hours * 15.0).to_radians();
    let ra2 = (ra2_hours * 15.0).to_radians();
    let (d1, d2) = (dec1.to_radians(), dec2.to_radians());
    let sin_half_dra = ((ra1 - ra2) / 2.0).sin();
    let sin_half_ddec = ((d1 - d2) / 2.0).sin();
    let h = sin_half_ddec * sin_half_ddec + d1.cos() * d2.cos() * sin_half_dra * sin_half_dra;
    2.0 * h.sqrt().asin().to_degrees()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn m31() -> Probe {
        let resolved = rp_catalog::Catalog::embedded().resolve("M 31").unwrap();
        Probe {
            name: resolved.name.clone(),
            ra_hours: resolved.coord.ra_hours(),
            dec_degrees: resolved.coord.dec_degrees(),
        }
    }

    fn mid_2026() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 28, 6, 0, 0).unwrap()
    }

    #[test]
    fn apparent_of_date_moves_m31_by_tens_of_arcminutes_in_2026() {
        let probe = m31();
        let (app_ra, app_dec) = apparent_of_date(probe.ra_hours, probe.dec_degrees, mid_2026())
            .expect("2026 is inside ERFA's calendar range");
        let sep = separation_degrees(probe.ra_hours, probe.dec_degrees, app_ra, app_dec);
        assert!(
            (5.0 / 60.0..40.0 / 60.0).contains(&sep),
            "expected 5′..40′ of precession since J2000, got {:.2}′",
            sep * 60.0
        );
    }

    #[test]
    fn j2000_coordinates_yield_a_j2000_verdict() {
        let probe = m31();
        let inference = EpochInference::from_probes(vec![probe.clone()]);
        let report = inference.analyze(probe.ra_hours, probe.dec_degrees, mid_2026());
        assert_eq!(report.verdict, EpochVerdict::LooksJ2000);
    }

    #[test]
    fn apparent_coordinates_yield_a_jnow_verdict() {
        let probe = m31();
        let (app_ra, app_dec) =
            apparent_of_date(probe.ra_hours, probe.dec_degrees, mid_2026()).unwrap();
        let inference = EpochInference::from_probes(vec![probe]);
        let report = inference.analyze(app_ra, app_dec, mid_2026());
        assert_eq!(report.verdict, EpochVerdict::LooksApparentOfDate);
    }

    #[test]
    fn a_point_far_from_every_probe_yields_no_verdict() {
        let inference = EpochInference::from_probes(vec![m31()]);
        let report = inference.analyze(12.0, -60.0, mid_2026());
        assert_eq!(report.verdict, EpochVerdict::NoVerdict);
    }
}
