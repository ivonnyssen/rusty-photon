//! The `target_store` config block (`docs/services/rp.md` § Target Store →
//! Configuration): `db_path`, `default_goals`, and `default_scheduling`
//! today (Decision 9's altitude-gating parity,
//! `docs/plans/planetarium-target-import.md`); `default_grading` lands with
//! the on-disk frame scan.
//!
//! Typed on [`crate::config::Config`] as [`TargetStoreConfigWire`] rather
//! than an untyped `Value`: the block is now the *only* meaning of the key
//! (the legacy `targets[]` planner array was retired — targets live in the
//! redb store, added via `add_target`), so `Config`'s top-level
//! `deny_unknown_fields` rejects a stray legacy `targets` key or a leftover
//! array shape loudly at load, and `GET /api/config/schema` carries the
//! real structure. The wire type stays rp-local (mirroring [`GoalWire`]'s
//! role for goals) so the store leaf keeps its deliberately schemars-free
//! build (`crates/rp-targets/BUILD.bazel`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::planner::goal_wire::GoalWire;

/// Parsed, validated `target_store` config block feeding the target-store
/// MCP tools (`add_target`'s `default_goals` fallback, the store's on-disk
/// location). Produced from [`TargetStoreConfigWire`] by
/// [`parse_target_store_config`].
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

/// The `target_store` config block as it appears on the wire (config JSON)
/// — the type of [`crate::config::Config::target_store`]. `db_path` and
/// `default_goals`/`default_scheduling` mirror [`TargetStoreConfig`], but
/// `default_goals` carries the [`GoalWire`] string shape (`binning`
/// `"1x1"`, `exposure_duration` `"5m"`) that the `TryFrom<&GoalWire>`
/// conversion validates, and
/// `default_scheduling` is the schemars-able [`SchedulingWire`] projection.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TargetStoreConfigWire {
    /// See [`TargetStoreConfig::db_path`].
    pub db_path: Option<String>,
    /// See [`TargetStoreConfig::default_goals`]; wire shape is [`GoalWire`].
    pub default_goals: Vec<GoalWire>,
    /// See [`TargetStoreConfig::default_scheduling`]; wire shape is
    /// [`SchedulingWire`].
    pub default_scheduling: SchedulingWire,
}

/// JsonSchema-able wire projection of [`rp_targets::SchedulingConstraints`]
/// — the store leaf is deliberately schemars-free
/// (`crates/rp-targets/BUILD.bazel`), so the config layer owns the
/// schema-bearing shape (the same reason [`GoalWire`] exists for goals).
/// Field-for-field identical; each `None` falls back to the rp-config
/// global default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SchedulingWire {
    /// Minimum altitude, in degrees, the target must be above.
    pub min_altitude_degrees: Option<f64>,
    /// Minimum angular separation from the Moon, in degrees.
    pub min_moon_separation_degrees: Option<f64>,
    /// Maximum Moon illumination fraction (`0.0`-`1.0`).
    pub max_moon_illumination_fraction: Option<f64>,
    /// Maximum `|hour angle|` from the meridian, in hours.
    pub meridian_window_hours: Option<f64>,
}

impl From<SchedulingWire> for rp_targets::SchedulingConstraints {
    fn from(w: SchedulingWire) -> Self {
        Self {
            min_altitude_degrees: w.min_altitude_degrees,
            min_moon_separation_degrees: w.min_moon_separation_degrees,
            max_moon_illumination_fraction: w.max_moon_illumination_fraction,
            meridian_window_hours: w.meridian_window_hours,
        }
    }
}

/// Validates a [`TargetStoreConfigWire`] into [`TargetStoreConfig`]: serde
/// has already checked the block's shape at config-load (typed field +
/// `deny_unknown_fields`), so the only remaining fallible step is parsing
/// each `default_goals` entry's `binning` / `exposure_duration` strings.
///
/// # Errors
///
/// Returns a human-readable message if a `default_goals` entry fails
/// its `TryFrom<&GoalWire>` conversion into [`rp_targets::AcquisitionGoal`].
pub fn parse_target_store_config(
    wire: &TargetStoreConfigWire,
) -> Result<TargetStoreConfig, String> {
    let mut default_goals = Vec::with_capacity(wire.default_goals.len());
    for g in &wire.default_goals {
        default_goals.push(rp_targets::AcquisitionGoal::try_from(g)?);
    }
    Ok(TargetStoreConfig {
        db_path: wire.db_path.clone(),
        default_goals,
        default_scheduling: wire.default_scheduling.into(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn default_wire_gives_defaults() {
        let config = parse_target_store_config(&TargetStoreConfigWire::default()).unwrap();
        assert_eq!(config, TargetStoreConfig::default());
    }

    #[test]
    fn object_shape_parses_db_path_and_default_goals() {
        let wire: TargetStoreConfigWire = serde_json::from_value(serde_json::json!({
            "db_path": "/data/lights/targets.redb",
            "default_goals": [
                {"filter": "L", "binning": "1x1", "exposure_duration": "5m", "desired_count": 20}
            ]
        }))
        .unwrap();
        let config = parse_target_store_config(&wire).unwrap();
        assert_eq!(config.db_path.as_deref(), Some("/data/lights/targets.redb"));
        assert_eq!(config.default_goals.len(), 1);
        assert_eq!(config.default_goals[0].filter, "L");
    }

    #[test]
    fn object_shape_parses_default_scheduling() {
        let wire: TargetStoreConfigWire = serde_json::from_value(serde_json::json!({
            "default_scheduling": { "min_altitude_degrees": 25.0 }
        }))
        .unwrap();
        let config = parse_target_store_config(&wire).unwrap();
        assert_eq!(config.default_scheduling.min_altitude_degrees, Some(25.0));
    }

    #[test]
    fn wire_rejects_unknown_field() {
        let err =
            serde_json::from_value::<TargetStoreConfigWire>(serde_json::json!({"bogus": true}))
                .unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
    }

    #[test]
    fn wire_rejects_a_leftover_array_shape() {
        // The retired legacy `targets[]` planner array — an un-migrated
        // config must fail loudly at load, not silently yield empty
        // target-store settings.
        let err = serde_json::from_value::<TargetStoreConfigWire>(serde_json::json!([
            {"name": "M31", "ra_hours": 0.7, "dec_degrees": 41.0}
        ]))
        .unwrap_err();
        assert!(
            err.to_string().contains("invalid type"),
            "expected a type-mismatch error, got: {err}"
        );
    }

    #[test]
    fn object_shape_rejects_bad_default_goal() {
        let wire: TargetStoreConfigWire = serde_json::from_value(serde_json::json!({
            "default_goals": [{"filter": "L", "binning": "bad", "exposure_duration": "5m", "desired_count": 20}]
        }))
        .unwrap();
        let err = parse_target_store_config(&wire).unwrap_err();
        assert!(err.contains("binning"), "{err}");
    }

    #[test]
    fn wire_round_trips_through_json() {
        let wire: TargetStoreConfigWire = serde_json::from_value(serde_json::json!({
            "db_path": "/data/targets.redb",
            "default_goals": [
                {"filter": "L", "binning": "1x1", "exposure_duration": "5m", "desired_count": 20}
            ],
            "default_scheduling": { "min_altitude_degrees": 20.0 }
        }))
        .unwrap();
        let round_tripped: TargetStoreConfigWire =
            serde_json::from_value(serde_json::to_value(&wire).unwrap()).unwrap();
        assert_eq!(wire, round_tripped);
    }
}
