//! Wire-format conversion for [`rp_targets::AcquisitionGoal`]: the JSON
//! shape `add_target`/`set_goals`/`targets.default_goals` (config) all
//! share — `binning` as `"AxB"` and `exposure` as a humantime string
//! (`"300s"`), rather than `AcquisitionGoal`'s derived struct/duration
//! shapes. Shared by [`crate::mcp::built_in::targets`] (the MCP tool
//! bodies) and [`crate::config::target_store`] (parsing
//! `targets.default_goals`) so the two stay byte-for-byte consistent.

use std::time::Duration;

use rp_targets::{AcquisitionGoal, Binning};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The wire shape of one `goals[]` entry, as accepted by `add_target`,
/// `set_goals`, and `targets.default_goals` in config.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GoalWire {
    pub filter: String,
    /// `"AxB"`, e.g. `"1x1"`, `"2x2"`.
    pub binning: String,
    /// A humantime duration string, e.g. `"300s"`.
    pub exposure: String,
    pub desired_count: u32,
}

/// Parses one wire-format goal into [`AcquisitionGoal`].
///
/// # Errors
///
/// Returns a human-readable message naming the offending value when
/// `binning` isn't `"AxB"` or `exposure` isn't a valid humantime string.
pub fn parse_goal(g: &GoalWire) -> Result<AcquisitionGoal, String> {
    Ok(AcquisitionGoal {
        filter: g.filter.clone(),
        binning: parse_binning(&g.binning)?,
        exposure: humantime::parse_duration(&g.exposure)
            .map_err(|e| format!("goal exposure {:?}: {e}", g.exposure))?,
        desired_count: g.desired_count,
    })
}

fn parse_binning(s: &str) -> Result<Binning, String> {
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

/// Renders an exposure `Duration` back to the wire string, exactly —
/// whole-second exposures round-trip byte-for-byte (`"300s"` in,
/// `"300s"` out), since [`humantime::format_duration`] would otherwise
/// pick a coarser unit (`"5m"`) that fails a literal round-trip
/// comparison.
pub fn format_exposure(d: Duration) -> String {
    if d.subsec_nanos() == 0 {
        format!("{}s", d.as_secs())
    } else {
        humantime::format_duration(d).to_string()
    }
}

/// Renders one goal back to its wire JSON shape (the inverse of
/// [`parse_goal`]).
pub fn goal_to_json(g: &AcquisitionGoal) -> Value {
    json!({
        "filter": g.filter,
        "binning": g.binning.to_string(),
        "exposure": format_exposure(g.exposure),
        "desired_count": g.desired_count,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    fn wire(filter: &str, binning: &str, exposure: &str, desired_count: u32) -> GoalWire {
        GoalWire {
            filter: filter.to_string(),
            binning: binning.to_string(),
            exposure: exposure.to_string(),
            desired_count,
        }
    }

    #[test]
    fn parse_goal_round_trips_whole_second_exposure() {
        let goal = parse_goal(&wire("Ha", "1x1", "300s", 20)).unwrap();
        assert_eq!(goal.binning, Binning { x: 1, y: 1 });
        assert_eq!(goal.exposure, Duration::from_secs(300));
        assert_eq!(
            goal_to_json(&goal),
            json!({
                "filter": "Ha", "binning": "1x1", "exposure": "300s", "desired_count": 20
            })
        );
    }

    #[test]
    fn parse_goal_rejects_malformed_binning() {
        let err = parse_goal(&wire("Ha", "1", "300s", 20)).unwrap_err();
        assert!(err.contains("binning"), "{err}");
    }

    #[test]
    fn parse_goal_rejects_malformed_exposure() {
        let err = parse_goal(&wire("Ha", "1x1", "not-a-duration", 20)).unwrap_err();
        assert!(err.contains("exposure"), "{err}");
    }
}
