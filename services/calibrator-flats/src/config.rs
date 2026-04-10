use serde::Deserialize;

/// Filter plan entry: which filter, how many frames.
#[derive(Debug, Clone, Deserialize)]
pub struct FilterPlan {
    pub name: String,
    pub count: u32,
}

/// Flat calibration plan passed via the orchestrator plugin config.
#[derive(Debug, Clone, Deserialize)]
pub struct FlatPlan {
    pub camera_id: String,
    pub filter_wheel_id: String,
    pub calibrator_id: String,
    /// Target median as fraction of max ADU (default 0.5 = 50%)
    #[serde(default = "default_target_adu_fraction")]
    pub target_adu_fraction: f64,
    /// Acceptable deviation from target (default 0.05 = 5%)
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// Max iterations to find correct exposure time per filter
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// Starting exposure time in milliseconds
    #[serde(default = "default_initial_duration_ms")]
    pub initial_duration_ms: u32,
    /// Calibrator brightness (null/absent = max_brightness)
    #[serde(default)]
    pub brightness: Option<u32>,
    /// Filters to capture flats for
    pub filters: Vec<FilterPlan>,
}

fn default_target_adu_fraction() -> f64 {
    0.5
}

fn default_tolerance() -> f64 {
    0.05
}

fn default_max_iterations() -> u32 {
    10
}

fn default_initial_duration_ms() -> u32 {
    1000
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn deserialize_flat_plan_with_defaults() {
        let json = r#"{
            "camera_id": "main-cam",
            "filter_wheel_id": "main-fw",
            "calibrator_id": "flat-panel",
            "filters": [
                {"name": "Luminance", "count": 20}
            ]
        }"#;
        let plan: FlatPlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.camera_id, "main-cam");
        assert_eq!(plan.target_adu_fraction, 0.5);
        assert_eq!(plan.tolerance, 0.05);
        assert_eq!(plan.max_iterations, 10);
        assert_eq!(plan.initial_duration_ms, 1000);
        assert!(plan.brightness.is_none());
        assert_eq!(plan.filters.len(), 1);
    }

    #[test]
    fn deserialize_flat_plan_with_overrides() {
        let json = r#"{
            "camera_id": "main-cam",
            "filter_wheel_id": "main-fw",
            "calibrator_id": "flat-panel",
            "target_adu_fraction": 0.4,
            "tolerance": 0.1,
            "max_iterations": 5,
            "initial_duration_ms": 500,
            "brightness": 80,
            "filters": [
                {"name": "Red", "count": 10},
                {"name": "Blue", "count": 15}
            ]
        }"#;
        let plan: FlatPlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.target_adu_fraction, 0.4);
        assert_eq!(plan.tolerance, 0.1);
        assert_eq!(plan.max_iterations, 5);
        assert_eq!(plan.initial_duration_ms, 500);
        assert_eq!(plan.brightness, Some(80));
        assert_eq!(plan.filters.len(), 2);
    }
}
