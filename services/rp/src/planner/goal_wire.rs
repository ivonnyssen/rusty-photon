//! Wire-format conversion for [`rp_targets::AcquisitionGoal`]: the JSON
//! shape `add_target`/`set_goals`/`targets.default_goals` (config) all
//! share — `binning` as `"AxB"` and `exposure_duration` as a humantime string
//! (`"5m"`), rather than `AcquisitionGoal`'s derived struct/duration
//! shapes. Shared by [`crate::mcp::built_in::targets`] (the MCP tool
//! bodies) and [`crate::config::target_store`] (parsing
//! `targets.default_goals`) so the two stay byte-for-byte consistent.

use rp_targets::{AcquisitionGoal, Binning};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The wire shape of one `goals[]` entry, as accepted by `add_target`,
/// `set_goals`, and `targets.default_goals` in config.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct GoalWire {
    pub filter: String,
    /// `"AxB"`, e.g. `"1x1"`, `"2x2"`.
    pub binning: String,
    /// A humantime duration string, e.g. `"5m"` (`"300s"` is accepted too;
    /// humantime rolls whole minutes up, so it reads back as `"5m"`).
    pub exposure_duration: String,
    pub desired_count: u32,
}

/// Parses one wire-format goal into [`AcquisitionGoal`].
///
/// # Errors
///
/// Returns a human-readable message naming the offending value when
/// `binning` isn't `"AxB"` or `exposure_duration` isn't a valid humantime string.
pub fn parse_goal(g: &GoalWire) -> Result<AcquisitionGoal, String> {
    Ok(AcquisitionGoal {
        filter: g.filter.clone(),
        binning: parse_binning(&g.binning)?,
        exposure_duration: humantime::parse_duration(&g.exposure_duration)
            .map_err(|e| format!("goal exposure_duration {:?}: {e}", g.exposure_duration))?,
        desired_count: g.desired_count,
    })
}

/// Parses `"AxB"` binning (e.g. `"1x1"`, `"2x2"`) into a [`Binning`].
/// Shared with [`crate::config::naming_template`], whose `parse()`
/// recovers a `{binning}` token's value from the same wire shape.
///
/// # Errors
///
/// Returns a human-readable message when `s` isn't `"AxB"` with two
/// valid `u8` factors.
pub(crate) fn parse_binning(s: &str) -> Result<Binning, String> {
    let (x, y) = s
        .split_once('x')
        .ok_or_else(|| format!("invalid binning {s:?}: expected \"AxB\", e.g. \"1x1\""))?;
    let x = x
        .parse::<u8>()
        .map_err(|_| format!("invalid binning {s:?}: expected \"AxB\", e.g. \"1x1\""))?;
    let y = y
        .parse::<u8>()
        .map_err(|_| format!("invalid binning {s:?}: expected \"AxB\", e.g. \"1x1\""))?;
    Ok(Binning { x, y })
}

/// Renders one goal back to its wire JSON shape (the inverse of
/// [`parse_goal`]). `exposure_duration` uses [`humantime::format_duration`]
/// — the exact encoding `AcquisitionGoal`'s `humantime_serde` field uses
/// for the redb store, so the MCP wire and the store agree (a 300 s sub is
/// `"5m"`, a 32 µs bias is `"32us"`; the file-naming template renders the
/// same string with humantime's inter-unit spaces removed).
pub fn goal_to_json(g: &AcquisitionGoal) -> Value {
    json!({
        "filter": g.filter,
        "binning": g.binning.to_string(),
        "exposure_duration": humantime::format_duration(g.exposure_duration).to_string(),
        "desired_count": g.desired_count,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn wire(filter: &str, binning: &str, exposure_duration: &str, desired_count: u32) -> GoalWire {
        GoalWire {
            filter: filter.to_string(),
            binning: binning.to_string(),
            exposure_duration: exposure_duration.to_string(),
            desired_count,
        }
    }

    #[test]
    fn parse_goal_round_trips_exposure_duration_through_humantime() {
        // Wire in as `300s`, back out as humantime's `5m` — the same
        // rollup `humantime_serde` applies for the store, so the wire and
        // the store agree. The `Duration` itself round-trips exactly.
        let goal = parse_goal(&wire("Ha", "1x1", "300s", 20)).unwrap();
        assert_eq!(goal.binning, Binning { x: 1, y: 1 });
        assert_eq!(goal.exposure_duration, Duration::from_secs(300));
        assert_eq!(
            goal_to_json(&goal),
            json!({
                "filter": "Ha", "binning": "1x1", "exposure_duration": "5m", "desired_count": 20
            })
        );
    }

    #[test]
    fn goal_to_json_encodes_a_sub_second_bias_exposure() {
        // The motivating case: a sub-second exposure the old whole-second
        // encoding could not represent now survives the wire.
        let goal = parse_goal(&wire("Dark", "1x1", "32us", 50)).unwrap();
        assert_eq!(goal.exposure_duration, Duration::from_micros(32));
        assert_eq!(goal_to_json(&goal)["exposure_duration"], "32us");
    }

    #[test]
    fn parse_goal_rejects_malformed_binning() {
        let err = parse_goal(&wire("Ha", "1", "300s", 20)).unwrap_err();
        assert!(err.contains("binning"), "{err}");
    }

    #[test]
    fn parse_goal_rejects_malformed_exposure_duration() {
        let err = parse_goal(&wire("Ha", "1x1", "not-a-duration", 20)).unwrap_err();
        assert!(err.contains("exposure_duration"), "{err}");
    }
}
