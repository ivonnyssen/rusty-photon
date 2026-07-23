//! Parses `Config.targets` (`docs/services/rp.md` § Target Store →
//! Configuration) when it holds the P1 target-store settings object —
//! `db_path`, `default_goals`, and `default_scheduling` today (Decision
//! 9's altitude-gating parity, `docs/plans/planetarium-target-import.md`);
//! `default_grading` lands with the on-disk frame scan.
//!
//! `Config.targets` stays untyped `Value` (see [`crate::config::Config`])
//! because the same top-level key still carries the legacy `targets[]`
//! planner array that
//! [`crate::planner::decision::parse_targets_from_value`] reads — the two
//! coexist during the P1 migration, distinguished by JSON shape (object
//! = new target-store settings, array/absent = legacy). The hard
//! cutover that retires the array shape is tracked separately
//! (`target_store_planner.feature`, still `@wip`).

use serde::Deserialize;
use serde_json::Value;

use crate::planner::goal_wire::{parse_goal, GoalWire};

/// Parsed `targets` config block feeding the target-store MCP tools
/// (`add_target`'s `default_goals` fallback, the store's on-disk
/// location).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TargetStoreConfig {
    /// Overrides the default `<session.data_directory>/targets.redb`
    /// location.
    pub db_path: Option<String>,
    /// Applied by `add_target` when the caller supplies no `goals[]`
    /// (Decision 10 — rp-owned policy, not bridge/UI config).
    pub default_goals: Vec<rp_targets::AcquisitionGoal>,
    /// Fallback scheduling constraints for a target whose own
    /// `scheduling` is `None`. `get_next_target`'s altitude-gating
    /// parity (Decision 9) reads `min_altitude_degrees` from here when
    /// a store-backed target carries no per-target override.
    pub default_scheduling: rp_targets::SchedulingConstraints,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawTargetStoreConfig {
    db_path: Option<String>,
    default_goals: Vec<GoalWire>,
    default_scheduling: rp_targets::SchedulingConstraints,
}

/// Parses `config.targets` into [`TargetStoreConfig`]. A JSON object is
/// the new target-store settings shape; an array (the legacy
/// `targets[]` planner config) or `null`/absent carries no
/// target-store settings, so this returns the all-defaults
/// [`TargetStoreConfig`] rather than erroring.
///
/// # Errors
///
/// Returns a human-readable message if `targets` is an object but
/// doesn't match [`RawTargetStoreConfig`]'s shape, or if a
/// `default_goals` entry fails [`parse_goal`].
pub fn parse_target_store_config(v: &Value) -> Result<TargetStoreConfig, String> {
    let raw: RawTargetStoreConfig = match v {
        Value::Object(_) => serde_json::from_value(v.clone()).map_err(|e| e.to_string())?,
        _ => RawTargetStoreConfig::default(),
    };
    let mut default_goals = Vec::with_capacity(raw.default_goals.len());
    for g in &raw.default_goals {
        default_goals.push(parse_goal(g)?);
    }
    Ok(TargetStoreConfig {
        db_path: raw.db_path,
        default_goals,
        default_scheduling: raw.default_scheduling,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn absent_targets_key_gives_defaults() {
        let config = parse_target_store_config(&Value::Null).unwrap();
        assert_eq!(config, TargetStoreConfig::default());
    }

    #[test]
    fn legacy_array_shape_gives_defaults() {
        let v = serde_json::json!([{"name": "M31", "ra_hours": 0.7, "dec_degrees": 41.0}]);
        let config = parse_target_store_config(&v).unwrap();
        assert_eq!(config, TargetStoreConfig::default());
    }

    #[test]
    fn object_shape_parses_db_path_and_default_goals() {
        let v = serde_json::json!({
            "db_path": "/data/lights/targets.redb",
            "default_goals": [
                {"filter": "L", "binning": "1x1", "exposure": "300s", "desired_count": 20}
            ]
        });
        let config = parse_target_store_config(&v).unwrap();
        assert_eq!(config.db_path.as_deref(), Some("/data/lights/targets.redb"));
        assert_eq!(config.default_goals.len(), 1);
        assert_eq!(config.default_goals[0].filter, "L");
    }

    #[test]
    fn object_shape_parses_default_scheduling() {
        let v = serde_json::json!({
            "default_scheduling": { "min_altitude_degrees": 25.0 }
        });
        let config = parse_target_store_config(&v).unwrap();
        assert_eq!(config.default_scheduling.min_altitude_degrees, Some(25.0));
    }

    #[test]
    fn object_shape_rejects_unknown_field() {
        let v = serde_json::json!({"bogus": true});
        let err = parse_target_store_config(&v).unwrap_err();
        assert!(err.contains("bogus"), "{err}");
    }

    #[test]
    fn object_shape_rejects_bad_default_goal() {
        let v = serde_json::json!({
            "default_goals": [{"filter": "L", "binning": "bad", "exposure": "300s", "desired_count": 20}]
        });
        let err = parse_target_store_config(&v).unwrap_err();
        assert!(err.contains("binning"), "{err}");
    }
}
