use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a train is for. The guiding train tells rp which camera's
/// focus and rotation state the guider depends on; everything else is
/// an imaging train.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TrainPurpose {
    #[default]
    Imaging,
    Guiding,
}

/// Effective focal length of a light path in millimetres.
///
/// Validated at load (parse-don't-validate): a non-finite or
/// non-positive value is rejected during deserialization, so a bad
/// config fails at startup rather than at capture time. Serializes
/// transparently as the inner `f64` (and the JSON Schema is a plain
/// number), matching the `try_from = "f64"` deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "f64")]
pub struct FocalLengthMm(f64);

impl FocalLengthMm {
    /// The single validating constructor. Rejects non-finite or
    /// non-positive lengths, naming the field in the error.
    pub fn try_new(value: f64) -> Result<Self, String> {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!(
                "focal_length_mm must be a positive finite number, got {value}"
            ));
        }
        Ok(Self(value))
    }

    /// The focal length in millimetres.
    pub fn value(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for FocalLengthMm {
    type Error = String;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

/// A positive focuser-step count for the V-curve sweep grid
/// (`auto_focus.step_size`). Parse-don't-validate: zero, negative,
/// and i32-overflowing values are rejected at deserialize with the
/// field named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "i64")]
pub struct SweepStepSize(i32);

impl SweepStepSize {
    pub fn value(self) -> i32 {
        self.0
    }
}

impl TryFrom<i64> for SweepStepSize {
    type Error = String;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match i32::try_from(value) {
            Ok(v) if v > 0 => Ok(Self(v)),
            _ => Err(format!(
                "auto_focus.step_size must be a positive integer, got {value}"
            )),
        }
    }
}

/// A positive sweep half-width in focuser steps
/// (`auto_focus.half_width`), validated like [`SweepStepSize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "i64")]
pub struct SweepHalfWidth(i32);

impl SweepHalfWidth {
    pub fn value(self) -> i32 {
        self.0
    }
}

impl TryFrom<i64> for SweepHalfWidth {
    type Error = String;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match i32::try_from(value) {
            Ok(v) if v > 0 => Ok(Self(v)),
            _ => Err(format!(
                "auto_focus.half_width must be a positive integer, got {value}"
            )),
        }
    }
}

/// Per-train V-curve sweep parameters (`optical_trains[].auto_focus`,
/// rp.md § Optical Trains): the five fields the `auto_focus` tool
/// requires per call, as train-scoped config. Backs train-addressed
/// `auto_focus` calls (per-call parameters override field by field)
/// and is required on every train a `refocus_train` expansion runs in.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrainAutoFocusConfig {
    /// Per-frame exposure for every point in the sweep.
    #[serde(with = "humantime_serde")]
    #[schemars(with = "String")]
    pub duration: Duration,
    pub step_size: SweepStepSize,
    pub half_width: SweepHalfWidth,
    /// Minimum component pixel area for the per-frame `measure_basic`.
    pub min_area: usize,
    /// Maximum component pixel area for the per-frame `measure_basic`.
    pub max_area: usize,
    /// Per-frame `measure_basic` threshold in sigma units. Omitted →
    /// the tool default (5.0).
    #[serde(default)]
    pub threshold_sigma: Option<f64>,
    /// Minimum non-null HFR samples for the parabolic fit. Omitted →
    /// the tool default (5).
    #[serde(default)]
    pub min_fit_points: Option<usize>,
}

/// One `equipment.optical_trains[]` entry (rp.md § Optical Trains): an
/// ordered list of roster device ids, objective side first,
/// terminating in a camera. Membership expresses coupling, position
/// expresses optical order. The cross-array graph rules (roster
/// existence, terminal camera, order consistency, the
/// one-guiding-train rule) live in
/// [`crate::equipment::trains::TrainModel::try_from_equipment`],
/// shared by `load_config` and `PUT /api/config`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpticalTrainConfig {
    pub id: String,
    #[serde(default)]
    pub purpose: TrainPurpose,
    /// Effective focal length of this light path. Omitted → captures
    /// through this train's camera carry no `optics` block, exactly
    /// like a camera outside any train.
    #[serde(default)]
    pub focal_length_mm: Option<FocalLengthMm>,
    /// Roster device ids, objective side first. The last entry must be
    /// a camera; the rest are focusers, rotators, and filter wheels.
    pub devices: Vec<String>,
    /// V-curve sweep parameters for focusing this train. Omitted → the
    /// train cannot be auto-focused by `refocus_train`, and
    /// train-addressed `auto_focus` calls must pass every sweep
    /// parameter per call.
    #[serde(default)]
    pub auto_focus: Option<TrainAutoFocusConfig>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::config::load_config;

    fn write_config(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, contents).unwrap();
        (dir, path)
    }

    #[test]
    fn optical_train_minimal_defaults_to_imaging_without_focal_length() {
        let (_dir, path) = write_config(
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "cameras": [
                        {"id": "main-cam", "alpaca_url": "http://localhost:11120"}
                    ],
                    "optical_trains": [
                        {"id": "main", "devices": ["main-cam"]}
                    ]
                },
                "server": { "port": 0 }
            }"#,
        );

        let config = load_config(&path).unwrap();
        let train = &config.equipment.optical_trains[0];
        assert_eq!(train.id, "main");
        assert_eq!(train.purpose, TrainPurpose::Imaging);
        assert!(train.focal_length_mm.is_none());
        assert_eq!(train.devices, vec!["main-cam"]);
    }

    #[test]
    fn optical_train_full_entry_round_trips() {
        let (_dir, path) = write_config(
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "cameras": [
                        {"id": "guide-cam", "alpaca_url": "http://localhost:11121"}
                    ],
                    "focusers": [
                        {"id": "guide-focuser", "alpaca_url": "http://localhost:11113"}
                    ],
                    "mount": {
                        "alpaca_url": "http://localhost:11122",
                        "guiding": {"url": "http://localhost:11130"}
                    },
                    "optical_trains": [
                        {
                            "id": "guide",
                            "purpose": "guiding",
                            "focal_length_mm": 200.0,
                            "devices": ["guide-focuser", "guide-cam"]
                        }
                    ]
                },
                "server": { "port": 0 }
            }"#,
        );

        let config = load_config(&path).unwrap();
        let train = &config.equipment.optical_trains[0];
        assert_eq!(train.purpose, TrainPurpose::Guiding);
        assert_eq!(train.focal_length_mm.map(FocalLengthMm::value), Some(200.0));

        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(
            value.pointer("/equipment/optical_trains/0/purpose"),
            Some(&serde_json::json!("guiding"))
        );
        assert_eq!(
            value.pointer("/equipment/optical_trains/0/focal_length_mm"),
            Some(&serde_json::json!(200.0))
        );
    }

    #[test]
    fn optical_train_rejects_unknown_purpose_at_parse() {
        let (_dir, path) = write_config(
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "optical_trains": [
                        {"id": "main", "purpose": "solar", "devices": ["main-cam"]}
                    ]
                },
                "server": { "port": 0 }
            }"#,
        );

        let err = load_config(&path).unwrap_err().to_string();
        assert!(err.contains("unknown variant `solar`"), "{err}");
    }

    #[test]
    fn optical_train_rejects_non_positive_focal_length_at_parse() {
        let (_dir, path) = write_config(
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "optical_trains": [
                        {"id": "main", "focal_length_mm": -100.0, "devices": ["main-cam"]}
                    ]
                },
                "server": { "port": 0 }
            }"#,
        );

        let err = load_config(&path).unwrap_err().to_string();
        assert!(
            err.contains("focal_length_mm must be a positive finite number"),
            "{err}"
        );
    }

    #[test]
    fn optical_train_rejects_unknown_field() {
        let (_dir, path) = write_config(
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "optical_trains": [
                        {"id": "main", "devices": ["main-cam"], "camera_id": "main-cam"}
                    ]
                },
                "server": { "port": 0 }
            }"#,
        );

        let err = load_config(&path).unwrap_err().to_string();
        assert!(err.contains("camera_id"), "{err}");
    }

    #[test]
    fn optical_train_auto_focus_block_round_trips() {
        let (_dir, path) = write_config(
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "cameras": [
                        {"id": "main-cam", "alpaca_url": "http://localhost:11120"}
                    ],
                    "optical_trains": [
                        {"id": "main", "devices": ["main-cam"],
                         "auto_focus": {"duration": "3s", "step_size": 100,
                                        "half_width": 1000, "min_area": 4,
                                        "max_area": 500}}
                    ]
                },
                "server": { "port": 0 }
            }"#,
        );

        let config = load_config(&path).unwrap();
        let block = config.equipment.optical_trains[0]
            .auto_focus
            .as_ref()
            .unwrap();
        assert_eq!(block.duration, Duration::from_secs(3));
        assert_eq!(block.step_size.value(), 100);
        assert_eq!(block.half_width.value(), 1000);
        assert_eq!(block.min_area, 4);
        assert_eq!(block.max_area, 500);
        assert!(block.threshold_sigma.is_none());
        assert!(block.min_fit_points.is_none());

        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(
            value.pointer("/equipment/optical_trains/0/auto_focus/duration"),
            Some(&serde_json::json!("3s"))
        );
        assert_eq!(
            value.pointer("/equipment/optical_trains/0/auto_focus/step_size"),
            Some(&serde_json::json!(100))
        );
    }

    #[test]
    fn auto_focus_block_rejects_non_positive_sweep_fields_at_parse() {
        for (field, bad, named) in [
            (
                "step_size",
                serde_json::json!(0),
                "auto_focus.step_size must be a positive integer",
            ),
            (
                "half_width",
                serde_json::json!(-5),
                "auto_focus.half_width must be a positive integer",
            ),
        ] {
            let mut config = serde_json::json!({
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "optical_trains": [
                        {"id": "main", "devices": ["main-cam"],
                         "auto_focus": {"duration": "3s", "step_size": 100,
                                        "half_width": 1000, "min_area": 4,
                                        "max_area": 500}}
                    ]
                },
                "server": { "port": 0 }
            });
            config["equipment"]["optical_trains"][0]["auto_focus"][field] = bad;
            let (_dir, path) = write_config(&config.to_string());

            let err = load_config(&path).unwrap_err().to_string();
            assert!(err.contains(named), "{field}: {err}");
        }
    }

    #[test]
    fn auto_focus_block_rejects_unknown_field() {
        let (_dir, path) = write_config(
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "optical_trains": [
                        {"id": "main", "devices": ["main-cam"],
                         "auto_focus": {"duration": "3s", "step_size": 100,
                                        "half_width": 1000, "min_area": 4,
                                        "max_area": 500, "sweep_mode": "fast"}}
                    ]
                },
                "server": { "port": 0 }
            }"#,
        );

        let err = load_config(&path).unwrap_err().to_string();
        assert!(err.contains("sweep_mode"), "{err}");
    }

    #[test]
    fn sweep_newtype_validation_boundaries() {
        assert_eq!(SweepStepSize::try_from(1i64).unwrap().value(), 1);
        assert!(SweepStepSize::try_from(0i64)
            .unwrap_err()
            .contains("auto_focus.step_size"));
        assert!(SweepStepSize::try_from(i64::from(i32::MAX) + 1).is_err());
        assert_eq!(SweepHalfWidth::try_from(200i64).unwrap().value(), 200);
        assert!(SweepHalfWidth::try_from(-1i64)
            .unwrap_err()
            .contains("auto_focus.half_width"));
    }

    #[test]
    fn focal_length_newtype_validation_boundaries() {
        assert_eq!(FocalLengthMm::try_new(360.0).unwrap().value(), 360.0);
        // The `<= 0.0` edge and the non-finite branch (unreachable from
        // JSON, defensive-only) are rejected and name the field.
        assert!(FocalLengthMm::try_new(0.0)
            .unwrap_err()
            .contains("focal_length_mm"));
        assert!(FocalLengthMm::try_new(-1.0).is_err());
        assert!(FocalLengthMm::try_new(f64::NAN).is_err());
        assert!(FocalLengthMm::try_new(f64::INFINITY).is_err());
    }
}
