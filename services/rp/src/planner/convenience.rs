//! Convenience-tool body helpers: `get_target_status`,
//! `get_next_target`, `get_meridian_status`. Each produces a JSON
//! `Value` from typed inputs; the MCP tool body in `crate::mcp` handles
//! parameter parsing, equipment lookup, and `CallToolResult` shaping.
//!
//! Per rp.md §"Dynamic Planner": v1 does not yet wire per-target
//! progress (`record_exposure`) through the planner, so the
//! `progress` field on `target_status` is `null`. A future commit
//! will populate it once session-state plumbing is connected to the
//! planner.

use chrono::{DateTime, Utc};
use rp_ephemeris::{Ephemeris, ErfarsEphemeris, IcrsCoord, SideOfPier, Site};
use serde_json::{json, Value};

use super::decision::{signed_hour_angle, NextTargetRecommendation};

/// Status of a single named target: alt, az, hour-angle, time-to-set.
/// `time_to_set_seconds` is `null` when the target is circumpolar
/// above `min_altitude` or never reaches it on the supplied date.
pub fn target_status_view(
    site: &Site,
    target: IcrsCoord,
    target_name: &str,
    now: DateTime<Utc>,
    min_altitude_degrees: f64,
) -> Result<Value, String> {
    let eph = ErfarsEphemeris::new();
    let aa = eph
        .alt_az(site, target, now)
        .map_err(|e| format!("alt/az transform failed: {e}"))?;
    let lst = eph.sidereal_time(site, now).lst_hours;
    let ha = signed_hour_angle(lst, target.ra_hours);

    // `rise_set` answers for transits within a single UTC date. A
    // target that was up at the start of the UTC day (transit happened
    // late on the previous calendar day) reports a `set_utc` that may
    // already be in the past for *today's* date — what the caller
    // actually wants is "the next set, possibly from yesterday's
    // visibility window". Try today; if its set is in the past or
    // missing, look at the previous UTC date too.
    let today = now.date_naive();
    let yesterday = today.pred_opt();
    let pick_set = |rs: Option<rp_ephemeris::RiseSet>| {
        rs.and_then(|r| {
            if r.set_utc > now {
                Some((r.set_utc - now).num_seconds())
            } else {
                None
            }
        })
    };
    let today_rs = eph.rise_set(site, target, today, min_altitude_degrees);
    let time_to_set_seconds = pick_set(today_rs).or_else(|| {
        yesterday.and_then(|d| pick_set(eph.rise_set(site, target, d, min_altitude_degrees)))
    });

    Ok(json!({
        "target_name": target_name,
        "altitude_degrees": aa.altitude_degrees,
        "azimuth_degrees": aa.azimuth_degrees,
        "hour_angle_hours": ha,
        "time_to_set_seconds": time_to_set_seconds,
        "progress": serde_json::Value::Null,
    }))
}

/// Project a [`NextTargetRecommendation`] onto the JSON surface. The
/// `reason` field carries the structured discriminant so a planner
/// plugin can branch without parsing free-form text.
pub fn next_target_view(rec: NextTargetRecommendation) -> Value {
    // `NextTargetReason` is a plain enum so this never errors in
    // practice; fall back to `Value::Null` rather than panicking if
    // serde ever rejects the variant.
    let reason = serde_json::to_value(rec.reason).unwrap_or(serde_json::Value::Null);
    // The exposure plan: the first `exposures[]` entry of the
    // recommended target, null when it defines none. Rotating within
    // the plan (least progress, filter batching) is deferred until
    // `record_exposure` counters exist — see rp.md §"Dynamic Planner"
    // decision-logic bullets 3–4.
    let plan = rec.target.as_ref().and_then(|t| t.exposures.first());
    let filter = plan
        .and_then(|p| p.filter.clone())
        .map_or(Value::Null, Value::String);
    let duration_secs = plan.map_or(Value::Null, |p| json!(p.duration_secs));
    let target = rec.target.as_ref().map(|t| {
        json!({
            "name": t.name,
            "ra_hours": t.ra_hours,
            "dec_degrees": t.dec_degrees,
            "min_altitude_degrees": t.min_altitude_degrees,
        })
    });
    json!({
        "target": target,
        "reason": reason,
        "filter": filter,
        "duration_secs": duration_secs,
    })
}

/// Status of the meridian-flip clock: time-to-flip from the mount's
/// current pointing, plus the side of pier.
pub fn meridian_status_view(
    site: &Site,
    mount_ra_hours: f64,
    mount_dec_degrees: f64,
    now: DateTime<Utc>,
    side: SideOfPier,
) -> Value {
    let eph = ErfarsEphemeris::new();
    let target = IcrsCoord {
        ra_hours: mount_ra_hours,
        dec_degrees: mount_dec_degrees,
    };
    let dur = eph.meridian_flip(site, target, now, side);
    let side_str = match side {
        SideOfPier::East => "east",
        SideOfPier::West => "west",
        SideOfPier::Unknown => "unknown",
    };
    json!({
        "time_to_flip_seconds": dur.map(|d| d.num_seconds()),
        "side_of_pier": side_str,
        "mount_ra_hours": mount_ra_hours,
        "mount_dec_degrees": mount_dec_degrees,
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::planner::decision::{ExposureSpec, NextTargetReason, PlannerTarget};

    fn site() -> Site {
        Site::new(47.6062, -122.3321).unwrap()
    }

    #[test]
    fn target_status_for_polaris_emits_expected_fields() {
        let polaris = IcrsCoord {
            ra_hours: 2.5301944,
            dec_degrees: 89.2641111,
        };
        let now = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 1, 6, 0, 0).unwrap();
        let v = target_status_view(&site(), polaris, "Polaris", now, 20.0).unwrap();
        assert_eq!(v["target_name"], "Polaris");
        assert!(v["altitude_degrees"].as_f64().is_some());
        assert!(v["azimuth_degrees"].as_f64().is_some());
        assert!(v["hour_angle_hours"].as_f64().is_some());
        // Polaris is circumpolar at Seattle, so time_to_set_seconds
        // is null.
        assert!(v["time_to_set_seconds"].is_null());
        assert!(v["progress"].is_null());
    }

    #[test]
    fn next_target_view_serialises_no_targets_branch() {
        let rec = NextTargetRecommendation {
            target: None,
            reason: NextTargetReason::NoTargetsConfigured,
        };
        let v = next_target_view(rec);
        assert_eq!(v["reason"], "no_targets_configured");
        assert!(v["target"].is_null());
        assert!(v["filter"].is_null());
        assert!(v["duration_secs"].is_null());
    }

    #[test]
    fn next_target_view_serialises_recommendation_branch() {
        let rec = NextTargetRecommendation {
            target: Some(PlannerTarget {
                name: "M31".into(),
                ra_hours: 0.7,
                dec_degrees: 41.0,
                min_altitude_degrees: Some(25.0),
                exposures: Vec::new(),
            }),
            reason: NextTargetReason::BestTransitingCandidate,
        };
        let v = next_target_view(rec);
        assert_eq!(v["reason"], "best_transiting_candidate");
        assert_eq!(v["target"]["name"], "M31");
        assert_eq!(v["target"]["min_altitude_degrees"], 25.0);
        assert!(
            v["filter"].is_null() && v["duration_secs"].is_null(),
            "a target without exposures[] must leave the plan null: {v}"
        );
    }

    #[test]
    fn next_target_view_returns_the_first_exposure_plan_entry() {
        let rec = NextTargetRecommendation {
            target: Some(PlannerTarget {
                name: "M31".into(),
                ra_hours: 0.7,
                dec_degrees: 41.0,
                min_altitude_degrees: None,
                exposures: vec![
                    ExposureSpec {
                        filter: Some("Luminance".to_string()),
                        duration_secs: 300.0,
                    },
                    ExposureSpec {
                        filter: Some("Red".to_string()),
                        duration_secs: 120.0,
                    },
                ],
            }),
            reason: NextTargetReason::BestTransitingCandidate,
        };
        let v = next_target_view(rec);
        assert_eq!(v["filter"], "Luminance");
        assert_eq!(v["duration_secs"], 300.0);
        assert!(
            v["target"].get("exposures").is_none(),
            "the wire target object carries coordinates only: {v}"
        );
    }

    #[test]
    fn next_target_view_leaves_filter_null_for_an_unfiltered_plan_entry() {
        let rec = NextTargetRecommendation {
            target: Some(PlannerTarget {
                name: "OSC Field".into(),
                ra_hours: 0.7,
                dec_degrees: 41.0,
                min_altitude_degrees: None,
                exposures: vec![ExposureSpec {
                    filter: None,
                    duration_secs: 60.0,
                }],
            }),
            reason: NextTargetReason::BestTransitingCandidate,
        };
        let v = next_target_view(rec);
        assert!(v["filter"].is_null());
        assert_eq!(v["duration_secs"], 60.0);
    }

    #[test]
    fn meridian_status_view_includes_side_of_pier() {
        let now = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 1, 6, 0, 0).unwrap();
        let v = meridian_status_view(&site(), 12.0, 0.0, now, SideOfPier::East);
        assert_eq!(v["side_of_pier"], "east");
        assert!(v["time_to_flip_seconds"].as_i64().is_some());
    }
}
