//! Decision logic for the convenience planner tools (`get_next_target`,
//! `get_target_status`). Pure function over (target list, current
//! time, site, `Ephemeris` impl, default min-altitude, progress
//! counters); a hand-rolled mock `Ephemeris` can drive it
//! deterministically in tests.
//!
//! v1 implements five of the rp.md §"Dynamic Planner" decision-logic
//! bullets: altitude elimination (the first half of bullet 1),
//! transit preference (bullet 2), progress + filter tie-breaking
//! (bullets 3–4: survivors within [`TRANSIT_TIE_BAND_HOURS`] of the
//! best |HA| count as equally transiting, and among them least
//! completed-to-goal fraction wins, then a next-exposure filter
//! matching the last recorded frame, then `targets[]` order), and
//! bullet 6 in full — an exhausted target (every plan entry's
//! `count` met per the `record_exposure` counters) is eliminated,
//! all targets exhausted is `EndOfSession`, and otherwise when no
//! target survives, the Sun-elevation cut-off plus the Sun's
//! trend separates `WaitForTwilight` (dusk side), `EndOfSession`
//! (dawn side: the night is over), and `AllBelowMinAltitude` (true
//! astronomical night). Documented gaps: the set-time half of
//! bullet 1 and explicit bullet 5 (meridian-flip-aware exposure-fit
//! check; the choice of smallest-|HA| target satisfies it
//! indirectly) — tracked in the rp.md §"v1 implementation status"
//! callout.
//!
//! The returned `NextTargetReason` is a structured discriminant so
//! a planner plugin can branch without parsing free-form text.

use chrono::{DateTime, Utc};
use rp_ephemeris::{Ephemeris, Site};
use serde::Serialize;
use serde_json::Value;

use super::progress::SessionProgress;

/// Sun altitude (degrees) at astronomical dusk — the boundary that
/// rp.md's prose for `WaitForTwilight` references. Above this, the
/// sky is still too bright for deep-sky imaging (daylight, civil, or
/// nautical twilight); below it, true astronomical night.
const ASTRONOMICAL_DUSK_DEG: f64 = -18.0;

/// How far ahead the Sun is re-sampled to read its altitude trend
/// when the sky is brighter than astronomical dusk. Over 60 s the Sun
/// moves up to ≈ 0.25° in altitude — far above floating-point noise,
/// yet short enough that the sample cannot jump across a culmination
/// to the other side of the night.
const SUN_TREND_SAMPLE_SECS: i64 = 60;

/// Survivors whose |HA| is within this band of the best candidate's
/// count as equally transiting, letting the progress and filter
/// tie-breakers (rp.md §"Dynamic Planner" bullets 3–4) choose among
/// them. Half an hour of hour angle costs a negligible fraction of a
/// degree of altitude near culmination, so trading it for balanced
/// integration (and fewer filter changes) is free.
const TRANSIT_TIE_BAND_HOURS: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannerTarget {
    pub name: String,
    pub ra_hours: f64,
    pub dec_degrees: f64,
    /// Per-target altitude floor. `None` falls back to the
    /// planner-wide minimum supplied by the caller.
    pub min_altitude_degrees: Option<f64>,
    /// The target's `exposures[]` plan, in config order. The
    /// recommendation surfaces the first entry as `filter` /
    /// `duration_secs`; empty ⇒ both null (the orchestrator's own
    /// exposure parameters apply).
    pub exposures: Vec<ExposureSpec>,
}

/// One entry of a target's `exposures[]` plan (rp.md § Target
/// Definition).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExposureSpec {
    /// `None` for an unfiltered entry (no `filter` key, `null`, or
    /// `""` in config — e.g. an OSC rig without a filter wheel).
    pub filter: Option<String>,
    /// Exposure duration in seconds; positive and finite.
    pub duration_secs: f64,
    /// Integration goal for this entry (frames), tracked by the
    /// `record_exposure` counters. `None` = no finite goal: the entry
    /// recommends forever and its target never exhausts.
    pub count: Option<u32>,
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
    /// The recommended target's first *incomplete* `exposures[]`
    /// entry in plan order — what the `filter` / `duration_secs`
    /// fields of the tool result surface. `None` when there is no
    /// target or its plan is empty (the orchestrator's own exposure
    /// parameters apply).
    pub exposure: Option<ExposureSpec>,
}

/// Pick the next target to slew to. The decision is a pure function
/// of its arguments, so tests can drive it with a hand-rolled
/// `Ephemeris` mock, a frozen `now`, and a hand-filled progress
/// store.
pub fn next_target(
    eph: &impl Ephemeris,
    site: &Site,
    now: DateTime<Utc>,
    targets: &[PlannerTarget],
    default_min_altitude_deg: f64,
    progress: &SessionProgress,
) -> NextTargetRecommendation {
    if targets.is_empty() {
        return NextTargetRecommendation {
            target: None,
            reason: NextTargetReason::NoTargetsConfigured,
            exposure: None,
        };
    }

    // Step 1: eliminate. A target whose computed alt is below
    // `min_altitude_degrees` (per-target if set, else default) is
    // dropped, and so is an exhausted one — every `exposures[]`
    // entry's `count` met per the `record_exposure` counters
    // (rp.md §"Dynamic Planner" bullet 6's "met its integration
    // goal"). Set-time elimination (the "will set before one
    // exposure can complete" half of rp.md §"Dynamic Planner"
    // bullet 1) is a documented v1 gap — see the §"v1 implementation
    // status" callout in `docs/services/rp.md`.
    let mut survivors: Vec<&PlannerTarget> = Vec::new();
    for t in targets {
        if progress.is_exhausted(t) {
            tracing::debug!(
                target = %t.name,
                "target met its integration goal; eliminated from next_target evaluation"
            );
            continue;
        }
        let coords = rp_ephemeris::IcrsCoord {
            ra_hours: t.ra_hours,
            dec_degrees: t.dec_degrees,
        };
        let aa = match eph.alt_az(site, coords, now) {
            Ok(aa) => aa,
            Err(e) => {
                // ERFA can refuse the alt/az transform at degenerate
                // sites (e.g. exactly the pole). Log it so a
                // configuration problem doesn't disguise itself as
                // "all targets below floor"; continue past the
                // offender.
                tracing::debug!(
                    target = %t.name,
                    error = %e,
                    "alt/az transform failed; skipping target in next_target evaluation"
                );
                continue;
            }
        };
        let floor = t.min_altitude_degrees.unwrap_or(default_min_altitude_deg);
        if aa.altitude_degrees >= floor {
            survivors.push(t);
        }
    }

    if survivors.is_empty() {
        // Every target met its integration goal: the session is
        // complete regardless of what the sky is doing — the other
        // `EndOfSession` trigger of rp.md §"Dynamic Planner"
        // bullet 6.
        if targets.iter().all(|t| progress.is_exhausted(t)) {
            return NextTargetRecommendation {
                target: None,
                reason: NextTargetReason::EndOfSession,
                exposure: None,
            };
        }
        // Distinguish "the sky is too bright to image" from "all
        // targets are genuinely below the altitude floor": below the
        // Sun-altitude threshold for astronomical twilight (-18°,
        // true astronomical night) every target under its floor is
        // `AllBelowMinAltitude`. Brighter than that, the Sun's own
        // trend tells the two bright ends of the night apart: a
        // climbing Sun (re-sampled `SUN_TREND_SAMPLE_SECS` ahead) is
        // the dawn side — the night is over, `EndOfSession` — while
        // a descending Sun matches rp.md's "astronomical dusk has
        // not yet begun", `WaitForTwilight`. A level Sun (only at
        // the culminations) ties to waiting, because a wait loop
        // re-asks and self-corrects while ending a session is final.
        let sun_alt = eph.sun_position(site, now).alt_az.altitude_degrees;
        let reason = if sun_alt > ASTRONOMICAL_DUSK_DEG {
            let resample = now + chrono::Duration::seconds(SUN_TREND_SAMPLE_SECS);
            let sun_alt_later = eph.sun_position(site, resample).alt_az.altitude_degrees;
            if sun_alt_later > sun_alt {
                NextTargetReason::EndOfSession
            } else {
                NextTargetReason::WaitForTwilight
            }
        } else {
            NextTargetReason::AllBelowMinAltitude
        };
        return NextTargetRecommendation {
            target: None,
            reason,
            exposure: None,
        };
    }

    // Step 2: prefer transiting — smallest |HA| from the current LST
    // (bullet 2), with survivors inside `TRANSIT_TIE_BAND_HOURS` of
    // that best |HA| treated as ties for the progress and filter
    // tie-breakers (bullets 3–4) to order: least completed-to-goal
    // fraction first, then a next exposure whose filter matches the
    // last recorded frame's, then `targets[]` order (survivors keep
    // config order, so the scan's strict `<` is that final
    // tie-break).
    let lst = eph.sidereal_time(site, now).lst_hours;
    let abs_ha = |t: &PlannerTarget| signed_hour_angle(lst, t.ra_hours).abs();
    let Some(best_ha) = survivors
        .iter()
        .map(|t| abs_ha(t))
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    else {
        // Unreachable: the empty-survivors branch returns above. If a
        // future refactor invalidates that invariant we fall back to
        // the same "nothing above min altitude" outcome rather than
        // panicking.
        return NextTargetRecommendation {
            target: None,
            reason: NextTargetReason::AllBelowMinAltitude,
            exposure: None,
        };
    };
    let mut chosen: Option<(&PlannerTarget, (f64, bool, f64))> = None;
    for t in &survivors {
        let ha = abs_ha(t);
        if ha > best_ha + TRANSIT_TIE_BAND_HOURS {
            continue;
        }
        let filter_matches_last = match (
            progress.last_filter_key(),
            progress.next_incomplete_entry(t),
        ) {
            (Some(last), Some(entry)) => {
                super::progress::filter_key(entry.filter.as_deref()) == last
            }
            _ => false,
        };
        // Sort key inside the band, in bullet order: least fraction
        // (bullet 3), then a matching filter (bullet 4 — negated so
        // `false` = match sorts first), then the in-band |HA| itself
        // so two otherwise-equal candidates still prefer the closer
        // transit. Config order wins remaining exact ties via the
        // strict `<`.
        let key = (progress.fraction(t), !filter_matches_last, ha);
        let better = match &chosen {
            None => true,
            Some((_, k)) => key
                .partial_cmp(k)
                .unwrap_or(std::cmp::Ordering::Equal)
                .is_lt(),
        };
        if better {
            chosen = Some((t, key));
        }
    }
    let Some((chosen, _)) = chosen else {
        // Unreachable for the same reason as above: at least the
        // best-|HA| survivor is inside its own band.
        return NextTargetRecommendation {
            target: None,
            reason: NextTargetReason::AllBelowMinAltitude,
            exposure: None,
        };
    };

    NextTargetRecommendation {
        target: Some(chosen.clone()),
        reason: NextTargetReason::BestTransitingCandidate,
        exposure: progress.next_incomplete_entry(chosen).cloned(),
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

/// Project a store-backed [`rp_targets::Target`] onto a [`PlannerTarget`]
/// candidate for `next_target` (Decision 9 — altitude-gating parity,
/// `docs/plans/planetarium-target-import.md`). `name` carries the
/// target's `slug` (its stable identity — `display_name` is freely
/// operator-editable and unsuited as a lookup key). Every goal's
/// `desired_count` is a required, finite `u32` (`validate_goals`
/// rejects zero), so each maps to a `count: Some(_)` entry — a
/// store-backed target's plan is never "recommends forever" the way an
/// uncounted legacy `exposures[]` entry can be.
impl From<&rp_targets::Target> for PlannerTarget {
    fn from(t: &rp_targets::Target) -> Self {
        Self {
            name: t.slug.as_str().to_string(),
            ra_hours: t.ra_hours,
            dec_degrees: t.dec_degrees,
            min_altitude_degrees: t.scheduling.and_then(|s| s.min_altitude_degrees),
            exposures: t
                .goals
                .iter()
                .map(|g| ExposureSpec {
                    filter: (!g.filter.is_empty()).then(|| g.filter.clone()),
                    duration_secs: g.exposure.as_secs_f64(),
                    count: Some(g.desired_count),
                })
                .collect(),
        }
    }
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
            exposures: parse_exposures(entry, name),
        });
    }
    out
}

/// Parse a target row's `exposures[]` into typed entries, skipping
/// (with a `debug!` log) entries without a positive finite
/// `duration_secs` or with a non-string `filter`. Same rationale as
/// the target rows themselves: flag a config typo at parse time
/// instead of letting it surface as a confusing null plan at night.
fn parse_exposures(entry: &Value, target: &str) -> Vec<ExposureSpec> {
    let exposures = match entry.get("exposures") {
        None => return Vec::new(),
        Some(v) => v,
    };
    let Some(arr) = exposures.as_array() else {
        tracing::debug!(
            target = %target,
            "ignoring `exposures` that is not an array"
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for e in arr {
        let Some(duration_secs) = e.get("duration_secs").and_then(|n| n.as_f64()) else {
            tracing::debug!(
                target = %target, entry = ?e,
                "skipping exposure entry missing a numeric `duration_secs`"
            );
            continue;
        };
        if !duration_secs.is_finite() || duration_secs <= 0.0 {
            tracing::debug!(
                target = %target, duration_secs,
                "skipping exposure entry with a non-finite or non-positive `duration_secs`"
            );
            continue;
        }
        let filter = match e.get("filter") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => {
                tracing::debug!(
                    target = %target, filter = ?other,
                    "skipping exposure entry whose `filter` is not a string"
                );
                continue;
            }
        };
        let count = match e.get("count") {
            None | Some(Value::Null) => None,
            Some(v) => match v
                .as_u64()
                .filter(|c| *c > 0)
                .and_then(|c| u32::try_from(c).ok())
            {
                Some(c) => Some(c),
                None => {
                    tracing::debug!(
                        target = %target, count = ?v,
                        "skipping exposure entry whose `count` is not a positive integer"
                    );
                    continue;
                }
            },
        };
        out.push(ExposureSpec {
            filter,
            duration_secs,
            count,
        });
    }
    out
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
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
        /// Sun altitude at the tests' `now()` epoch.
        sun_alt: f64,
        /// Sun-altitude change per minute after `now()` — drives the
        /// dusk/dawn trend check. `0.0` freezes the Sun (a level Sun
        /// reads as the dusk side).
        sun_alt_rate_deg_per_min: f64,
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
        // The decision logic only consults `alt_az` and
        // `sun_position`; the remaining trait methods exist to satisfy
        // the impl block but are never called from these tests.
        // Mark them coverage-skip so they don't depress the patch %.
        #[cfg_attr(coverage_nightly, coverage(off))]
        fn transit(
            &self,
            _site: &Site,
            _target: IcrsCoord,
            _date: chrono::NaiveDate,
        ) -> Option<DateTime<Utc>> {
            None
        }
        #[cfg_attr(coverage_nightly, coverage(off))]
        fn rise_set(
            &self,
            _site: &Site,
            _target: IcrsCoord,
            _date: chrono::NaiveDate,
            _min: f64,
        ) -> Option<RiseSet> {
            None
        }
        #[cfg_attr(coverage_nightly, coverage(off))]
        fn meridian_flip(
            &self,
            _site: &Site,
            _target: IcrsCoord,
            _t: DateTime<Utc>,
            _side: rp_ephemeris::SideOfPier,
        ) -> Option<chrono::Duration> {
            None
        }
        fn sun_position(&self, _site: &Site, t: DateTime<Utc>) -> SunInfo {
            let minutes = (t - now()).num_seconds() as f64 / 60.0;
            SunInfo {
                coords: IcrsCoord {
                    ra_hours: 0.0,
                    dec_degrees: 0.0,
                },
                alt_az: AltAz {
                    altitude_degrees: self.sun_alt + self.sun_alt_rate_deg_per_min * minutes,
                    azimuth_degrees: 0.0,
                },
            }
        }
        #[cfg_attr(coverage_nightly, coverage(off))]
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
        #[cfg_attr(coverage_nightly, coverage(off))]
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
        #[cfg_attr(coverage_nightly, coverage(off))]
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
        let rec = next_target(
            &MockEphemeris::default(),
            &site(),
            now(),
            &[],
            20.0,
            &SessionProgress::default(),
        );
        assert!(rec.target.is_none());
        assert_eq!(rec.reason, NextTargetReason::NoTargetsConfigured);
    }

    #[test]
    fn target_below_min_alt_is_eliminated() {
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: -25.0, // true astronomical night (sun < -18°)
            sun_alt_rate_deg_per_min: 0.0,
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
            exposures: Vec::new(),
        }];
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            30.0,
            &SessionProgress::default(),
        );
        assert!(rec.target.is_none());
        assert_eq!(rec.reason, NextTargetReason::AllBelowMinAltitude);
    }

    #[test]
    fn a_level_daytime_sun_is_wait_for_twilight() {
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: 30.0, // the Sun is up and, frozen at rate 0, not climbing
            sun_alt_rate_deg_per_min: 0.0,
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
            exposures: Vec::new(),
        }];
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            30.0,
            &SessionProgress::default(),
        );
        assert!(rec.target.is_none());
        assert_eq!(rec.reason, NextTargetReason::WaitForTwilight);
    }

    #[test]
    fn nautical_twilight_returns_wait_for_twilight_not_all_below_min_altitude() {
        // Sun at -10° (nautical twilight, between civil at -6° and
        // astronomical at -18°). Per rp.md prose, "astronomical dusk
        // has not yet begun" → WaitForTwilight, not AllBelowMinAltitude.
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: -10.0,
            sun_alt_rate_deg_per_min: 0.0,
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
            exposures: Vec::new(),
        }];
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            30.0,
            &SessionProgress::default(),
        );
        assert_eq!(rec.reason, NextTargetReason::WaitForTwilight);
    }

    #[test]
    fn a_descending_twilight_sun_is_wait_for_twilight() {
        // Evening twilight: the Sun at -10° and sinking — the night
        // has not started, wait for it.
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: -10.0,
            sun_alt_rate_deg_per_min: -0.2,
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
            exposures: Vec::new(),
        }];
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            30.0,
            &SessionProgress::default(),
        );
        assert_eq!(rec.reason, NextTargetReason::WaitForTwilight);
    }

    #[test]
    fn a_climbing_twilight_sun_is_end_of_session() {
        // Morning twilight: the Sun at -10° and climbing — the night
        // is over.
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: -10.0,
            sun_alt_rate_deg_per_min: 0.2,
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
            exposures: Vec::new(),
        }];
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            30.0,
            &SessionProgress::default(),
        );
        assert!(rec.target.is_none());
        assert_eq!(rec.reason, NextTargetReason::EndOfSession);
    }

    #[test]
    fn a_climbing_daytime_sun_is_end_of_session() {
        // A session invoked mid-morning: the Sun is high and still
        // climbing. This calendar night is over — end, don't wait.
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: 30.0,
            sun_alt_rate_deg_per_min: 0.2,
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
            exposures: Vec::new(),
        }];
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            30.0,
            &SessionProgress::default(),
        );
        assert_eq!(rec.reason, NextTargetReason::EndOfSession);
    }

    #[test]
    fn a_climbing_sun_still_below_astronomical_dusk_is_all_below_min_altitude() {
        // Pre-dawn astronomical night: the Sun rises toward -18° but
        // has not crossed it. It is still properly dark, so a
        // below-floor target set is reported as such — dawn is only
        // declared once the sky is actually bright.
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: -25.0,
            sun_alt_rate_deg_per_min: 0.2,
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
            exposures: Vec::new(),
        }];
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            30.0,
            &SessionProgress::default(),
        );
        assert_eq!(rec.reason, NextTargetReason::AllBelowMinAltitude);
    }

    /// A target no computed altitude can reach — forces the
    /// no-survivors branch against the real ephemeris.
    fn never_visible_target() -> Vec<PlannerTarget> {
        vec![PlannerTarget {
            name: "below floor".into(),
            ra_hours: 0.0,
            dec_degrees: 0.0,
            min_altitude_degrees: Some(90.0),
            exposures: Vec::new(),
        }]
    }

    // The two real-ephemeris tests below pin the same equinox
    // instants the BDD dusk/dawn scenarios use, but through
    // `next_target` directly — the mock tests above fix the trend
    // maths, these keep it honest against the real sky. At the UK
    // site on 2026-03-20 the Sun sits near -11° descending at
    // 19:20 UTC and near -10° climbing at 05:00 UTC.

    #[test]
    fn real_ephemeris_evening_twilight_is_wait_for_twilight() {
        let eph = rp_ephemeris::ErfarsEphemeris::new();
        let site = Site::new(51.0786, -0.2944).unwrap();
        let t = Utc.with_ymd_and_hms(2026, 3, 20, 19, 20, 0).unwrap();
        let sun_alt = eph.sun_position(&site, t).alt_az.altitude_degrees;
        assert!(
            (-18.0..0.0).contains(&sun_alt),
            "the pinned instant must sit in twilight; the Sun is at {sun_alt}°"
        );
        let rec = next_target(
            &eph,
            &site,
            t,
            &never_visible_target(),
            20.0,
            &SessionProgress::default(),
        );
        assert_eq!(rec.reason, NextTargetReason::WaitForTwilight);
    }

    #[test]
    fn real_ephemeris_morning_twilight_is_end_of_session() {
        let eph = rp_ephemeris::ErfarsEphemeris::new();
        let site = Site::new(51.0786, -0.2944).unwrap();
        let t = Utc.with_ymd_and_hms(2026, 3, 20, 5, 0, 0).unwrap();
        let sun_alt = eph.sun_position(&site, t).alt_az.altitude_degrees;
        assert!(
            (-18.0..0.0).contains(&sun_alt),
            "the pinned instant must sit in twilight; the Sun is at {sun_alt}°"
        );
        let rec = next_target(
            &eph,
            &site,
            t,
            &never_visible_target(),
            20.0,
            &SessionProgress::default(),
        );
        assert_eq!(rec.reason, NextTargetReason::EndOfSession);
    }

    #[test]
    fn full_astronomical_night_with_no_targets_above_floor_is_all_below_min() {
        // Sun well below -18° (true astronomical night) and every
        // target still below the floor → distinguish from twilight.
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7123, 41.27), 10.0)],
            sun_alt: -25.0,
            sun_alt_rate_deg_per_min: 0.0,
            lst_hours: 12.0,
        };
        let targets = vec![PlannerTarget {
            name: "M31".into(),
            ra_hours: 0.7123,
            dec_degrees: 41.27,
            min_altitude_degrees: None,
            exposures: Vec::new(),
        }];
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            30.0,
            &SessionProgress::default(),
        );
        assert_eq!(rec.reason, NextTargetReason::AllBelowMinAltitude);
    }

    #[test]
    fn picks_target_closest_to_transit() {
        // LST = 12.0. Two targets above min alt:
        //   M31 at ra=0.7 → HA = 11.3 → very far from transit
        //   M42 at ra=11.0 → HA = 1.0 → close to transit
        let eph = MockEphemeris {
            alt_overrides: vec![((0.7, 41.0), 50.0), ((11.0, -5.0), 50.0)],
            sun_alt: -20.0,
            sun_alt_rate_deg_per_min: 0.0,
            lst_hours: 12.0,
        };
        let targets = vec![
            PlannerTarget {
                name: "M31".into(),
                ra_hours: 0.7,
                dec_degrees: 41.0,
                min_altitude_degrees: None,
                exposures: Vec::new(),
            },
            PlannerTarget {
                name: "M42".into(),
                ra_hours: 11.0,
                dec_degrees: -5.0,
                min_altitude_degrees: None,
                exposures: Vec::new(),
            },
        ];
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            20.0,
            &SessionProgress::default(),
        );
        let target = rec.target.expect("expected a target");
        assert_eq!(target.name, "M42");
        assert_eq!(rec.reason, NextTargetReason::BestTransitingCandidate);
    }

    // --- progress-aware selection (rp.md bullets 3, 4, and the
    // exhausted-targets half of bullet 6) -------------------------

    /// A dec-0 target above the floor at `ra_hours`, with a plan.
    fn target_with_plan(name: &str, ra_hours: f64, exposures: Vec<ExposureSpec>) -> PlannerTarget {
        PlannerTarget {
            name: name.into(),
            ra_hours,
            dec_degrees: 0.0,
            min_altitude_degrees: None,
            exposures,
        }
    }

    fn spec(filter: &str, count: u32) -> ExposureSpec {
        ExposureSpec {
            filter: Some(filter.into()),
            duration_secs: 60.0,
            count: Some(count),
        }
    }

    /// Every dec-0 target at the given RAs sits at 50° — selection
    /// tests care about hour angle and progress, not elimination.
    fn night_eph(ras: &[f64]) -> MockEphemeris {
        MockEphemeris {
            alt_overrides: ras.iter().map(|ra| ((*ra, 0.0), 50.0)).collect(),
            sun_alt: -25.0,
            sun_alt_rate_deg_per_min: 0.0,
            lst_hours: 12.0,
        }
    }

    #[test]
    fn an_exhausted_target_is_eliminated_and_the_backup_recommended() {
        // "M31" transits (HA 0) but its whole plan is complete; the
        // farther "M42" is the only live candidate.
        let eph = night_eph(&[12.0, 10.0]);
        let targets = vec![
            target_with_plan("M31", 12.0, vec![spec("L", 1)]),
            target_with_plan("M42", 10.0, vec![spec("L", 1)]),
        ];
        let mut p = SessionProgress::default();
        p.record("M31", Some("L"));
        let rec = next_target(&eph, &site(), now(), &targets, 20.0, &p);
        assert_eq!(rec.target.expect("expected a target").name, "M42");
    }

    #[test]
    fn all_targets_exhausted_is_end_of_session_even_in_deep_night() {
        // Sun at -25° (true astronomical night) and the target still
        // above its floor — but its integration goal is met, so the
        // session is over. This is the non-dawn `EndOfSession`.
        let eph = night_eph(&[12.0]);
        let targets = vec![target_with_plan("M31", 12.0, vec![spec("L", 1)])];
        let mut p = SessionProgress::default();
        p.record("M31", Some("L"));
        let rec = next_target(&eph, &site(), now(), &targets, 20.0, &p);
        assert!(rec.target.is_none());
        assert_eq!(rec.reason, NextTargetReason::EndOfSession);
    }

    #[test]
    fn a_below_floor_survivor_prevents_the_exhaustion_end_of_session() {
        // One target exhausted, the other merely below its floor: the
        // night is not over — the sky gating answers (dark sky ⇒
        // AllBelowMinAltitude), so the orchestrator keeps waiting for
        // the unfinished target to rise.
        let eph = MockEphemeris {
            alt_overrides: vec![((12.0, 0.0), 50.0), ((10.0, 0.0), 5.0)],
            sun_alt: -25.0,
            sun_alt_rate_deg_per_min: 0.0,
            lst_hours: 12.0,
        };
        let targets = vec![
            target_with_plan("done", 12.0, vec![spec("L", 1)]),
            target_with_plan("still rising", 10.0, vec![spec("L", 1)]),
        ];
        let mut p = SessionProgress::default();
        p.record("done", Some("L"));
        let rec = next_target(&eph, &site(), now(), &targets, 20.0, &p);
        assert!(rec.target.is_none());
        assert_eq!(rec.reason, NextTargetReason::AllBelowMinAltitude);
    }

    #[test]
    fn the_recommendation_rotates_to_the_first_incomplete_plan_entry() {
        let eph = night_eph(&[12.0]);
        let targets = vec![target_with_plan(
            "M31",
            12.0,
            vec![spec("L", 1), spec("R", 1)],
        )];
        let mut p = SessionProgress::default();
        let rec = next_target(&eph, &site(), now(), &targets, 20.0, &p);
        assert_eq!(
            rec.exposure.expect("plan entry").filter.as_deref(),
            Some("L")
        );
        p.record("M31", Some("L"));
        let rec = next_target(&eph, &site(), now(), &targets, 20.0, &p);
        assert_eq!(
            rec.exposure.expect("plan entry").filter.as_deref(),
            Some("R"),
            "the completed Luminance goal must rotate the recommendation to Red"
        );
    }

    #[test]
    fn least_progress_wins_inside_the_transit_tie_band() {
        // "closer" transits exactly (HA 0) but is half done; "fresh"
        // sits 0.3 h away — inside the 0.5 h band, so bullet 3 hands
        // it the recommendation.
        let eph = night_eph(&[12.0, 11.7]);
        let targets = vec![
            target_with_plan("closer", 12.0, vec![spec("L", 2)]),
            target_with_plan("fresh", 11.7, vec![spec("L", 2)]),
        ];
        let mut p = SessionProgress::default();
        p.record("closer", Some("L"));
        let rec = next_target(&eph, &site(), now(), &targets, 20.0, &p);
        assert_eq!(rec.target.expect("expected a target").name, "fresh");
    }

    #[test]
    fn a_matching_filter_breaks_a_progress_tie() {
        // Both candidates are untouched (fraction 0) and in-band; the
        // last recorded frame was Red, so the target whose next
        // exposure is Red wins (bullet 4) despite the larger |HA| and
        // later config position.
        let eph = night_eph(&[12.0, 11.7]);
        let targets = vec![
            target_with_plan("blue next", 12.0, vec![spec("Blue", 5)]),
            target_with_plan("red next", 11.7, vec![spec("Red", 5)]),
        ];
        let mut p = SessionProgress::default();
        p.record("somewhere else entirely", Some("Red"));
        let rec = next_target(&eph, &site(), now(), &targets, 20.0, &p);
        assert_eq!(rec.target.expect("expected a target").name, "red next");
    }

    #[test]
    fn outside_the_band_the_closer_transit_wins_regardless_of_progress() {
        // 1.1 h of hour angle is past the 0.5 h tie band: transit
        // preference (bullet 2) stays primary and the nearly-done
        // transiting target still wins.
        let eph = night_eph(&[12.0, 10.9]);
        let targets = vec![
            target_with_plan("transiting", 12.0, vec![spec("L", 10)]),
            target_with_plan("far and fresh", 10.9, vec![spec("L", 10)]),
        ];
        let mut p = SessionProgress::default();
        for _ in 0..9 {
            p.record("transiting", Some("L"));
        }
        let rec = next_target(&eph, &site(), now(), &targets, 20.0, &p);
        assert_eq!(rec.target.expect("expected a target").name, "transiting");
    }

    #[test]
    fn per_target_min_altitude_overrides_default() {
        let eph = MockEphemeris {
            alt_overrides: vec![((1.0, 0.0), 25.0)],
            sun_alt: -20.0,
            sun_alt_rate_deg_per_min: 0.0,
            lst_hours: 1.0,
        };
        let targets = vec![PlannerTarget {
            name: "T1".into(),
            ra_hours: 1.0,
            dec_degrees: 0.0,
            min_altitude_degrees: Some(20.0),
            exposures: Vec::new(),
        }];
        // default 30 would eliminate; per-target 20 keeps it.
        let rec = next_target(
            &eph,
            &site(),
            now(),
            &targets,
            30.0,
            &SessionProgress::default(),
        );
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

    fn store_target(
        slug: &str,
        scheduling: Option<rp_targets::SchedulingConstraints>,
        goals: Vec<rp_targets::AcquisitionGoal>,
    ) -> rp_targets::Target {
        rp_targets::Target {
            slug: rp_targets::TargetSlug::new(slug).unwrap(),
            display_name: slug.to_string(),
            ra_hours: 1.0,
            dec_degrees: 2.0,
            catalog_ref: None,
            object_type: None,
            magnitude: None,
            size_arcmin: None,
            priority: 0,
            active: true,
            goals,
            scheduling,
            grading: None,
            notes: None,
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            updated_at: "2026-01-01T00:00:00.000Z".to_string(),
        }
    }

    #[test]
    fn from_store_target_uses_slug_as_identity() {
        let t = store_target("ngc7000", None, Vec::new());
        let planner_target = PlannerTarget::from(&t);
        assert_eq!(planner_target.name, "ngc7000");
        assert_eq!(planner_target.ra_hours, 1.0);
        assert_eq!(planner_target.dec_degrees, 2.0);
        assert_eq!(planner_target.min_altitude_degrees, None);
    }

    #[test]
    fn from_store_target_reads_the_scheduling_override() {
        let t = store_target(
            "ngc7000",
            Some(rp_targets::SchedulingConstraints {
                min_altitude_degrees: Some(35.0),
                ..Default::default()
            }),
            Vec::new(),
        );
        assert_eq!(PlannerTarget::from(&t).min_altitude_degrees, Some(35.0));
    }

    #[test]
    fn from_store_target_converts_goals_to_finite_exposure_specs() {
        let goal = rp_targets::AcquisitionGoal {
            filter: "L".to_string(),
            binning: rp_targets::Binning { x: 1, y: 1 },
            exposure: std::time::Duration::from_secs(300),
            desired_count: 20,
        };
        let t = store_target("ngc7000", None, vec![goal]);
        let planner_target = PlannerTarget::from(&t);
        assert_eq!(
            planner_target.exposures,
            vec![ExposureSpec {
                filter: Some("L".to_string()),
                duration_secs: 300.0,
                count: Some(20),
            }]
        );
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

    #[test]
    fn parse_targets_reads_exposures_in_order() {
        let v = serde_json::json!([{
            "name": "M31",
            "ra_hours": 0.7,
            "dec_degrees": 41.0,
            "exposures": [
                {"filter": "Luminance", "duration_secs": 300, "count": 40},
                {"filter": "Red", "duration_secs": 120.5},
                {"duration_secs": 60},
            ],
        }]);
        let parsed = parse_targets_from_value(&v);
        assert_eq!(
            parsed[0].exposures,
            vec![
                ExposureSpec {
                    filter: Some("Luminance".to_string()),
                    duration_secs: 300.0,
                    count: Some(40),
                },
                ExposureSpec {
                    filter: Some("Red".to_string()),
                    duration_secs: 120.5,
                    count: None,
                },
                ExposureSpec {
                    filter: None,
                    duration_secs: 60.0,
                    count: None,
                },
            ]
        );
    }

    #[test]
    fn parse_targets_without_exposures_yields_an_empty_plan() {
        let v = serde_json::json!([
            {"name": "bare", "ra_hours": 1.0, "dec_degrees": 0.0},
            {"name": "not_array", "ra_hours": 2.0, "dec_degrees": 0.0, "exposures": "oops"},
        ]);
        let parsed = parse_targets_from_value(&v);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].exposures.is_empty());
        assert!(parsed[1].exposures.is_empty());
    }

    #[test]
    fn parse_exposures_skips_invalid_entries_and_normalises_empty_filters() {
        let v = serde_json::json!([{
            "name": "M31",
            "ra_hours": 0.7,
            "dec_degrees": 41.0,
            "exposures": [
                {"filter": "Red"},
                {"filter": "Red", "duration_secs": 0},
                {"filter": "Red", "duration_secs": -5},
                {"filter": "Red", "duration_secs": "300"},
                {"filter": 5, "duration_secs": 300},
                {"filter": "", "duration_secs": 30},
                {"filter": null, "duration_secs": 45},
            ],
        }]);
        let parsed = parse_targets_from_value(&v);
        assert_eq!(
            parsed[0].exposures,
            vec![
                ExposureSpec {
                    filter: None,
                    duration_secs: 30.0,
                    count: None,
                },
                ExposureSpec {
                    filter: None,
                    duration_secs: 45.0,
                    count: None,
                },
            ],
            "only entries with a positive numeric duration survive; \
             empty/null filters normalise to None"
        );
    }

    #[test]
    fn parse_exposures_skips_entries_with_an_invalid_count() {
        let v = serde_json::json!([{
            "name": "M31",
            "ra_hours": 0.7,
            "dec_degrees": 41.0,
            "exposures": [
                {"duration_secs": 10, "count": 0},
                {"duration_secs": 20, "count": -3},
                {"duration_secs": 30, "count": 2.5},
                {"duration_secs": 40, "count": "5"},
                {"duration_secs": 50, "count": null},
                {"duration_secs": 60, "count": 7},
            ],
        }]);
        let parsed = parse_targets_from_value(&v);
        assert_eq!(
            parsed[0].exposures,
            vec![
                ExposureSpec {
                    filter: None,
                    duration_secs: 50.0,
                    count: None,
                },
                ExposureSpec {
                    filter: None,
                    duration_secs: 60.0,
                    count: Some(7),
                },
            ],
            "a `count` must be a positive integer when present; \
             null reads as absent (no finite goal)"
        );
    }
}
