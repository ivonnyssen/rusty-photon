//! The plan data model: [`Target`], [`AcquisitionGoal`], and the small
//! value types they're built from. See `docs/crates/rp-targets.md` "Data
//! Model" for the authoritative field-by-field contract.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::TargetStoreError;

/// Immutable, filename-safe identity for a [`Target`].
///
/// Parse-don't-validate (see
/// `docs/skills/development-workflow.md#parse-dont-validate-for-config`):
/// [`TargetSlug::new`] lower-cases, strips all whitespace, and rejects any
/// remaining character outside `[a-z0-9-]`, so a valid `TargetSlug` is
/// always a safe directory and filename token. The slug is the `{target}`
/// token in every frame's on-disk path, so it must never change once
/// frames exist under it — renames go through `Target::display_name`
/// instead.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, derive_more::Display,
)]
#[serde(transparent)]
pub struct TargetSlug(String);

impl TargetSlug {
    /// Normalizes `input` (lower-case, strip whitespace) and validates the
    /// result is non-empty and entirely `[a-z0-9-]`.
    ///
    /// # Errors
    ///
    /// Returns [`TargetSlugError::Empty`] if normalization leaves nothing,
    /// or [`TargetSlugError::InvalidChars`] if a character outside
    /// `[a-z0-9-]` remains.
    pub fn new(input: &str) -> Result<Self, TargetSlugError> {
        let normalized: String = input
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();

        if normalized.is_empty() {
            return Err(TargetSlugError::Empty);
        }

        if !normalized
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(TargetSlugError::InvalidChars {
                input: input.to_string(),
            });
        }

        Ok(Self(normalized))
    }

    /// The normalized slug string, e.g. `"ngc7000-2"`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Errors constructing a [`TargetSlug`]. Caller-side (rp) validation before
/// [`crate::TargetStore::upsert_target`] is ever called.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetSlugError {
    /// `input` was empty, or contained only whitespace.
    #[error("target slug cannot be empty")]
    Empty,
    /// `input` contained a character outside `[a-z0-9-]` after
    /// lower-casing and whitespace stripping.
    #[error("target slug {input:?} contains characters outside [a-z0-9-]")]
    InvalidChars {
        /// The original, unnormalized input that failed validation.
        input: String,
    },
}

/// Frame binning, rendered as `"{x}x{y}"` (e.g. `"1x1"`, `"2x2"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, derive_more::Display)]
#[display("{x}x{y}")]
pub struct Binning {
    /// Horizontal binning factor.
    pub x: u8,
    /// Vertical binning factor.
    pub y: u8,
}

/// Desired frame count for one acquisition sub-spec. The
/// `(filter, binning, exposure)` triple is the quota key from the
/// filename scheme — frame type is always `Light` for goals, and gain is
/// a fixed per-setup camera setting rather than a sub-spec dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionGoal {
    /// Filter name, e.g. `"Ha"`, `"L"`, `"R"`.
    pub filter: String,
    /// Frame binning.
    pub binning: Binning,
    /// Per-frame exposure length.
    #[serde(with = "humantime_serde")]
    pub exposure: Duration,
    /// Number of good frames desired for this sub-spec.
    pub desired_count: u32,
}

/// Per-target scheduling constraints. Each `None` field falls back to the
/// rp-config global default. Storage only — evaluated by rp's planner via
/// `rp-ephemeris`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SchedulingConstraints {
    /// Minimum altitude, in degrees, the target must be above to be
    /// eligible for imaging.
    #[serde(default)]
    pub min_altitude_degrees: Option<f64>,
    /// Minimum angular separation from the Moon, in degrees.
    #[serde(default)]
    pub min_moon_separation_degrees: Option<f64>,
    /// Maximum Moon illumination fraction (`0.0`-`1.0`) the target may be
    /// imaged under.
    #[serde(default)]
    pub max_moon_illumination_fraction: Option<f64>,
    /// Maximum `|hour angle|` from the meridian, in hours, the target may
    /// be imaged at. `None` means no meridian window constraint.
    #[serde(default)]
    pub meridian_window_hours: Option<f64>,
}

/// Per-target grading thresholds. The grading plugin owns the *meaning* of
/// these; this crate only stores the overriding values. Each `None` falls
/// back to the rp-config global default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct GradingThresholds {
    /// Maximum half-flux-radius, in pixels, for a frame to grade "good".
    #[serde(default)]
    pub max_hfr_pixels: Option<f64>,
    /// Minimum detected star count for a frame to grade "good".
    #[serde(default)]
    pub min_star_count: Option<u32>,
    /// Maximum star eccentricity for a frame to grade "good".
    #[serde(default)]
    pub max_eccentricity: Option<f64>,
    /// Minimum signal-to-noise ratio for a frame to grade "good".
    #[serde(default)]
    pub min_snr: Option<f64>,
}

/// A planned pointing plus its acquisition goals — one row in the target
/// store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Immutable identity and on-disk/filename token.
    pub slug: TargetSlug,
    /// Operator-facing name; freely editable without breaking identity or
    /// existing on-disk frames.
    pub display_name: String,

    /// Right ascension, decimal hours, ICRS.
    pub ra_hours: f64,
    /// Declination, decimal degrees, ICRS.
    pub dec_degrees: f64,

    /// Canonical catalog name this was resolved from, e.g. `"NGC 224"`.
    /// `None` for non-catalog targets (comets, custom framings).
    #[serde(default)]
    pub catalog_ref: Option<String>,
    /// Denormalized from `rp_catalog::ResolvedTarget` at add-time.
    #[serde(default)]
    pub object_type: Option<String>,
    /// Denormalized catalog magnitude, if known.
    #[serde(default)]
    pub magnitude: Option<f64>,
    /// Denormalized catalog angular size in arcminutes, if known.
    #[serde(default)]
    pub size_arcmin: Option<f64>,

    /// Scheduling priority; higher values are preferred by the planner.
    pub priority: i32,
    /// Whether the planner may select this target. Bridge-imported targets
    /// land with `active: false` pending operator review.
    pub active: bool,
    /// Desired frame counts per acquisition sub-spec.
    #[serde(default)]
    pub goals: Vec<AcquisitionGoal>,

    /// Per-target scheduling overrides. `None` uses the rp-config global
    /// default in full.
    #[serde(default)]
    pub scheduling: Option<SchedulingConstraints>,
    /// Per-target grading overrides. `None` uses the rp-config global
    /// default in full.
    #[serde(default)]
    pub grading: Option<GradingThresholds>,

    /// Free-form operator/provenance notes.
    #[serde(default)]
    pub notes: Option<String>,
    /// RFC3339 creation timestamp; set by rp at the call boundary (this
    /// crate never reads the clock). Preserved across `upsert_target` of an
    /// existing slug.
    pub created_at: String,
    /// RFC3339 last-update timestamp; set by rp at the call boundary.
    pub updated_at: String,
}

/// Validates a goal set for [`crate::TargetStore::upsert_target`] and
/// [`crate::TargetStore::set_goals`]: no two goals may share the same
/// `(filter, binning, exposure)` key, and no goal may have a zero
/// `desired_count` or zero `exposure`.
///
/// # Errors
///
/// Returns [`TargetStoreError::InvalidGoals`] naming the offending goal.
pub fn validate_goals(goals: &[AcquisitionGoal]) -> Result<(), TargetStoreError> {
    for goal in goals {
        if goal.desired_count == 0 {
            return Err(TargetStoreError::InvalidGoals {
                reason: format!(
                    "goal for filter {:?} at {} has desired_count == 0",
                    goal.filter, goal.binning
                ),
            });
        }
        if goal.exposure.is_zero() {
            return Err(TargetStoreError::InvalidGoals {
                reason: format!(
                    "goal for filter {:?} at {} has a zero exposure",
                    goal.filter, goal.binning
                ),
            });
        }
    }

    for (i, a) in goals.iter().enumerate() {
        for b in &goals[i + 1..] {
            if a.filter == b.filter && a.binning == b.binning && a.exposure == b.exposure {
                return Err(TargetStoreError::InvalidGoals {
                    reason: format!(
                        "duplicate goal key: filter {:?}, binning {}, exposure {:?}",
                        a.filter, a.binning, a.exposure
                    ),
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn slug_lowercases_and_strips_whitespace() {
        let slug = TargetSlug::new("NGC 7000").unwrap();
        assert_eq!(slug.as_str(), "ngc7000");
    }

    #[test]
    fn slug_allows_hyphens_and_digits() {
        let slug = TargetSlug::new("ngc7000-2").unwrap();
        assert_eq!(slug.as_str(), "ngc7000-2");
    }

    #[test]
    fn slug_rejects_empty_input() {
        let err = TargetSlug::new("   ").unwrap_err();
        assert_eq!(err, TargetSlugError::Empty);
    }

    #[test]
    fn slug_rejects_out_of_charset_characters() {
        let err = TargetSlug::new("m33!").unwrap_err();
        assert_eq!(
            err,
            TargetSlugError::InvalidChars {
                input: "m33!".to_string()
            }
        );
    }

    #[test]
    fn binning_displays_as_wxh() {
        assert_eq!(Binning { x: 2, y: 2 }.to_string(), "2x2");
    }

    fn goal(filter: &str, x: u8, y: u8, secs: u64, desired_count: u32) -> AcquisitionGoal {
        AcquisitionGoal {
            filter: filter.to_string(),
            binning: Binning { x, y },
            exposure: Duration::from_secs(secs),
            desired_count,
        }
    }

    #[test]
    fn validate_goals_accepts_distinct_keys() {
        let goals = vec![goal("Ha", 1, 1, 300, 20), goal("Ha", 1, 1, 120, 20)];
        validate_goals(&goals).unwrap();
    }

    #[test]
    fn validate_goals_rejects_duplicate_key() {
        let goals = vec![goal("Ha", 1, 1, 300, 20), goal("Ha", 1, 1, 300, 10)];
        let err = validate_goals(&goals).unwrap_err();
        assert!(matches!(err, TargetStoreError::InvalidGoals { .. }));
    }

    #[test]
    fn validate_goals_rejects_zero_desired_count() {
        let goals = vec![goal("Ha", 1, 1, 300, 0)];
        let err = validate_goals(&goals).unwrap_err();
        assert!(matches!(err, TargetStoreError::InvalidGoals { .. }));
    }

    #[test]
    fn validate_goals_rejects_zero_exposure() {
        let goals = vec![goal("Ha", 1, 1, 0, 20)];
        let err = validate_goals(&goals).unwrap_err();
        assert!(matches!(err, TargetStoreError::InvalidGoals { .. }));
    }
}
