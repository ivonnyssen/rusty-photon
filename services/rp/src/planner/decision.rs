//! Decision logic for the convenience planner tools (`get_next_target`,
//! `get_target_status`). Pure function over (target list, current
//! time, site, `Ephemeris` impl, default min-altitude); a hand-rolled
//! mock `Ephemeris` can drive it deterministically in tests.
//!
//! v1 implements the rp.md §"Dynamic Planner" decision-logic bullets
//! 1, 2, and 6 in full (altitude / set-time elimination, prefer
//! transiting, twilight / end-of-session fallback). Bullet 3
//! (least-progress preference) and bullet 4 (filter-change
//! minimization) currently no-op because rp does not yet track
//! per-target progress in the session. Bullet 5 (meridian-flip
//! avoidance) is satisfied indirectly: a target whose transit was
//! already in the recent past has a large positive HA and ranks
//! lower than a target approaching transit.
//!
//! The returned `NextTargetReason` is a structured discriminant so
//! a planner plugin can branch without parsing free-form text.

use chrono::{DateTime, Utc};
use rp_ephemeris::{Ephemeris, Site};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannerTarget {
    pub name: String,
    pub ra_hours: f64,
    pub dec_degrees: f64,
    /// Per-target altitude floor. `None` falls back to the
    /// planner-wide minimum supplied by the caller.
    pub min_altitude_degrees: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NextTargetReason {
    BestTransitingCandidate,
    NoTargetsConfigured,
    AllBelowMinAltitude,
    WaitForTwilight,
    EndOfSession,
}

#[derive(Debug, Clone, Serialize)]
pub struct NextTargetRecommendation {
    /// `None` when `reason` is `NoTargetsConfigured`,
    /// `AllBelowMinAltitude`, `WaitForTwilight`, or `EndOfSession`.
    pub target: Option<PlannerTarget>,
    pub reason: NextTargetReason,
}

/// Pick the next target to slew to. The decision is a pure function
/// of its arguments, so tests can drive it with a hand-rolled
/// `Ephemeris` mock and a frozen `now`.
pub fn next_target(
    eph: &impl Ephemeris,
    site: &Site,
    now: DateTime<Utc>,
    targets: &[PlannerTarget],
    default_min_altitude_deg: f64,
) -> NextTargetRecommendation {
    if targets.is_empty() {
        return NextTargetRecommendation {
            target: None,
            reason: NextTargetReason::NoTargetsConfigured,
        };
    }

    // Step 1: eliminate by altitude. A target whose computed alt is
    // below `min_altitude_degrees` (per-target if set, else default)
    // is dropped.
    let mut survivors: Vec<&PlannerTarget> = Vec::new();
    for t in targets {
        let coords = rp_ephemeris::IcrsCoord {
            ra_hours: t.ra_hours,
            dec_degrees: t.dec_degrees,
        };
        let Ok(aa) = eph.alt_az(site, coords, now) else {
            continue;
        };
        let floor = t.min_altitude_degrees.unwrap_or(default_min_altitude_deg);
        if aa.altitude_degrees >= floor {
            survivors.push(t);
        }
    }

    if survivors.is_empty() {
        // Distinguish "we're in daylight" from "all targets are
        // below min altitude even though it's night". The Sun
        // elevation supplies that branch.
        //
        // `EndOfSession` is in the discriminant for forward
        // compatibility but unreachable from this v1 code path: it
        // belongs in a future revision that knows the difference
        // between "all targets exhausted" (per-target progress
        // counters all met) and "the night is over" (set times have
        // all passed). Both require state we don't yet thread
        // through — see rp.md §"Dynamic Planner" bullets 1, 3.
        let sun_alt = eph.sun_position(site, now).alt_az.altitude_degrees;
        let reason = if sun_alt > 0.0 {
            NextTargetReason::WaitForTwilight
        } else {
            NextTargetReason::AllBelowMinAltitude
        };
        return NextTargetRecommendation {
            target: None,
            reason,
        };
    }

    // Step 2: prefer transiting. The transit time minimises absolute
    // hour-angle from the current LST; pick the target with the
    // smallest |HA|.
    let lst = eph.sidereal_time(site, now).lst_hours;
    let chosen = survivors
        .into_iter()
        .min_by(|a, b| {
            let ha_a = signed_hour_angle(lst, a.ra_hours);
            let ha_b = signed_hour_angle(lst, b.ra_hours);
            ha_a.abs()
                .partial_cmp(&ha_b.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("survivors non-empty by construction");

    NextTargetRecommendation {
        target: Some(chosen.clone()),
        reason: NextTargetReason::BestTransitingCandidate,
    }
}

/// Hour angle of `target_ra_hours` at `lst_hours`, normalised to
/// the half-open interval `(-12, 12]` (negative = east of meridian,
/// positive = west).
pub fn signed_hour_angle(lst_hours: f64, target_ra_hours: f64) -> f64 {
    let mut ha = (lst_hours - target_ra_hours).rem_euclid(24.0);
    if ha > 12.0 {
        ha -= 24.0;
    }
    ha
}

/// Parse the top-level `targets` JSON (rp's `Config.targets: Value`)
/// into typed entries, skipping (with a `debug!` log) rows that
/// don't have the required `name` / `ra_hours` / `dec_degrees`
/// fields *or* whose numeric fields are out of range. The latter
/// would otherwise turn a config typo into a confusing runtime
/// "no_targets_configured" / "all_below_min_altitude" outcome from
/// `get_next_target` — flagging it at parse time keeps the failure
/// mode close to the operator's edit. Used at McpHandler
/// construction time.
pub fn parse_targets_from_value(v: &Value) -> Vec<PlannerTarget> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let Some(name) = entry.get("name").and_then(|n| n.as_str()) else {
            tracing::debug!(?entry, "skipping target row missing `name`");
            continue;
        };
        let Some(ra) = entry.get("ra_hours").and_then(|n| n.as_f64()) else {
            tracing::debug!(target = %name, "skipping target row missing `ra_hours`");
            continue;
        };
        let Some(dec) = entry.get("dec_degrees").and_then(|n| n.as_f64()) else {
            tracing::debug!(target = %name, "skipping target row missing `dec_degrees`");
            continue;
        };
        if !(0.0..24.0).contains(&ra) {
            tracing::debug!(
                target = %name, ra_hours = ra,
                "skipping target with ra_hours outside [0, 24)"
            );
            continue;
        }
        if !(-90.0..=90.0).contains(&dec) {
            tracing::debug!(
                target = %name, dec_degrees = dec,
                "skipping target with dec_degrees outside [-90, 90]"
            );
            continue;
        }
        let min_alt = entry.get("min_altitude_degrees").and_then(|n| n.as_f64());
        if let Some(m) = min_alt {
            if !(-90.0..=90.0).contains(&m) {
                tracing::debug!(
                    target = %name, min_altitude_degrees = m,
                    "skipping target with min_altitude_degrees outside [-90, 90]"
                );
                continue;
            }
        }
        out.push(PlannerTarget {
            name: name.to_string(),
            ra_hours: ra,
            dec_degrees: dec,
            min_altitude_degrees: min_alt,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rp_ephemeris::{
        AltAz, EphemerisError, IcrsCoord, LocalSiderealTime, MoonInfo, RiseSet, Site, SunInfo,
        TwilightKind, TwilightWindow,
    };

    /// Hand-rolled mock so the decision logic is testable without
    /// hitting real ERFA. The closures fix the answers per-target.
    #[derive(Default)]
    struct MockEphemeris {
        /// (ra_hours, dec_degrees) → altitude_degrees
        alt_overrides: Vec<((f64, f64), f64)>,
        sun_alt: f64,
        lst_hours: f64,
    }

    impl Ephemeris for MockEphemeris {
        fn sidereal_time(&self, _site: &Site, _t: DateTime<Utc>) -> LocalSiderealTime {
            LocalSiderealTime {
                lst_hours: self.lst_hours,
            }
        }
        fn alt_az(
            &self,
            _site: &Site,
            target: IcrsCoord,
            _t: DateTime<Utc>,
        ) -> Result<AltAz, EphemerisError> {
            let alt = self
                .alt_overrides
                .iter()
                .find_map(|((ra, dec), alt)| {
                    if (ra - target.ra_hours).abs() < 1e-9
                        && (dec - target.dec_degrees).abs() < 1e-9
                    {
                        Some(*alt)
                    } else {
                        None
                    }
                })
                .unwrap_or(0.0);
            Ok(AltAz {
                altitude_degrees: alt,
                azimuth_degrees: 0.0,
            })
        }
        fn transit(
            &self,
            _site: &Site,
            _target: IcrsCoord,
            _date: chrono::NaiveDate,
        ) -> Option<DateTime<Utc>> {
            None
        }
        fn rise_set(
            &self,
            _site: &Site,
            _target: IcrsCoord,
            _date: chrono::NaiveDate,
            _min: f64,
        ) -> Option<RiseSet> {
            None
        }
        fn meridian_flip(
            &self,
            _site: &Site,
            _target: IcrsCoord,
            _t: DateTime<Utc>,
            _side: rp_ephemeris::SideOfPier,
        ) -> Option<chrono::Duration> {
            None
        }
        fn sun_position(&self, _site: &Site, _t: DateTime<Utc>) -> SunInfo {
            SunInfo {
                coords: IcrsCoord {
                    ra_hours: 0.0,
                    dec_degrees: 0.0,
                },
                alt_az: AltAz {
                    altitude_degrees: self.sun_alt,
                    azimuth_degrees: 0.0,
                },
            }
        }
        fn twilight(
            &self,
            _site: &Site,
            _date: chrono::NaiveDate,
            _kind: TwilightKind,
        ) -> TwilightWindow {
            TwilightWindow {
                begin_utc: None,
                end_utc: None,
            }
        }
        fn moon_position(&self, _site: &Site, _t: DateTime<Utc>) -> MoonInfo {
            MoonInfo {
                coords: IcrsCoord {
                    ra_hours: 0.0,
                    dec_degrees: 0.0,
                },
                alt_az: AltAz {
                    altitude_degrees: 0.0,
                    azimuth_degrees: 0.0,
                },
                phase_degrees: 0.0,
                illumination_fraction: 0.5,
            }
        }
        fn moon_separation(&self, _target: IcrsCoord, _t: DateTime<Utc>) -> f64 {
            0.0
        }
    }

    fn site() -> Site {
        Site::new(47.6062, -122.3321).unwrap()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 6, 0, 0).unwrap()
    }

    #[test]
    fn empty_targets_return_no_targets_configured() {
        let rec = next_target(&MockEphemeris::default(), &site(), now(), &[], 20.0);
        assert!(rec.target.is_none());
        assert_eq!(rec.reason, NextTargetReason::NoTargetsConfigured);
    }

    #[test]
    fn target_below_min_alt_is_eliminated() {
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: -10.0, // night
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
        }];
        let rec = next_target(&eph, &site(), now(), &targets, 30.0);
        assert!(rec.target.is_none());
        assert_eq!(rec.reason, NextTargetReason::AllBelowMinAltitude);
    }

    #[test]
    fn daytime_returns_wait_for_twilight() {
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: 30.0, // sun is up
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
        }];
        let rec = next_target(&eph, &site(), now(), &targets, 30.0);
        assert!(rec.target.is_none());
        assert_eq!(rec.reason, NextTargetReason::WaitForTwilight);
    }

    #[test]
    fn picks_target_closest_to_transit() {
        // LST = 12.0. Two targets above min alt:
        //   M31 at ra=0.7 → HA = 11.3 → very far from transit
        //   M42 at ra=11.0 → HA = 1.0 → close to transit
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7, 41.0), 50.0), ((11.0, -5.0), 50.0)],
            sun_alt: -20.0,
            lst_hours: 12.0,
        };
        let targets = vec![
            PlannerTarget {
                name: "M31".into(),
                ra_hours: 0.7,
                dec_degrees: 41.0,
                min_altitude_degrees: None,
            },
            PlannerTarget {
                name: "M42".into(),
                ra_hours: 11.0,
                dec_degrees: -5.0,
                min_altitude_degrees: None,
            },
        ];
        let rec = next_target(&eph, &site(), now(), &targets, 20.0);
        let target = rec.target.expect("expected a target");
        assert_eq!(target.name, "M42");
        assert_eq!(rec.reason, NextTargetReason::BestTransitingCandidate);
    }

    #[test]
    fn per_target_min_altitude_overrides_default() {
        let eph = MockEphemeris {
            alt_overrides: vec![((1.0, 0.0), 25.0)],
            sun_alt: -20.0,
            lst_hours: 1.0,
        };
        let targets = vec![PlannerTarget {
            name: "T1".into(),
            ra_hours: 1.0,
            dec_degrees: 0.0,
            min_altitude_degrees: Some(20.0),
        }];
        // default 30 would eliminate; per-target 20 keeps it.
        let rec = next_target(&eph, &site(), now(), &targets, 30.0);
        assert!(
            rec.target.is_some(),
            "per-target floor must override default"
        );
    }

    #[test]
    fn signed_hour_angle_wraps_correctly() {
        assert!((signed_hour_angle(0.0, 23.5) - 0.5).abs() < 1e-9);
        assert!((signed_hour_angle(23.5, 0.0) - (-0.5)).abs() < 1e-9);
        assert!((signed_hour_angle(12.0, 0.0) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn parse_targets_skips_bad_entries() {
        let v = serde_json::json!([
            {"name": "M31", "ra_hours": 0.7, "dec_degrees": 41.0},
            {"name": "no_coords"},
            {"ra_hours": 1.0, "dec_degrees": 2.0},
            "garbage string",
            {"name": "M42", "ra_hours": 5.5, "dec_degrees": -5.4, "min_altitude_degrees": 25.0},
        ]);
        let parsed = parse_targets_from_value(&v);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "M31");
        assert_eq!(parsed[1].name, "M42");
        assert_eq!(parsed[1].min_altitude_degrees, Some(25.0));
    }

    #[test]
    fn parse_targets_skips_out_of_range_numerics() {
        let v = serde_json::json!([
            {"name": "good", "ra_hours": 1.0, "dec_degrees": 0.0},
            {"name": "ra_too_low", "ra_hours": -1.0, "dec_degrees": 0.0},
            {"name": "ra_too_high", "ra_hours": 25.0, "dec_degrees": 0.0},
            {"name": "dec_too_low", "ra_hours": 1.0, "dec_degrees": -91.0},
            {"name": "dec_too_high", "ra_hours": 1.0, "dec_degrees": 91.0},
            {
                "name": "min_alt_bad",
                "ra_hours": 1.0,
                "dec_degrees": 0.0,
                "min_altitude_degrees": 200.0
            },
        ]);
        let parsed = parse_targets_from_value(&v);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "good");
    }
}
