use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::camera::CameraConfig;
use super::cover_calibrator::CoverCalibratorConfig;
use super::dome::DomeConfig;
use super::filter_wheel::FilterWheelConfig;
use super::focuser::FocuserConfig;
use super::mount::MountConfig;
use super::observing_conditions::ObservingConditionsConfig;
use super::rotator::RotatorConfig;
use super::safety_monitor::SafetyMonitorConfig;
use super::switch::SwitchConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EquipmentConfig {
    #[serde(default)]
    pub cameras: Vec<CameraConfig>,
    #[serde(default)]
    pub mount: Option<MountConfig>,
    #[serde(default)]
    pub focusers: Vec<FocuserConfig>,
    #[serde(default)]
    pub filter_wheels: Vec<FilterWheelConfig>,
    #[serde(default)]
    pub cover_calibrators: Vec<CoverCalibratorConfig>,
    #[serde(default)]
    pub safety_monitors: Vec<SafetyMonitorConfig>,
    #[serde(default)]
    pub switches: Vec<SwitchConfig>,
    #[serde(default)]
    pub rotators: Vec<RotatorConfig>,
    #[serde(default)]
    pub observing_conditions: Vec<ObservingConditionsConfig>,
    #[serde(default)]
    pub domes: Vec<DomeConfig>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use crate::config::load_config;

    #[test]
    fn equipment_config_rejects_unknown_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {"rotator": {}},
                "server": { "port": 0 }
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err().to_string();
        assert!(err.contains("rotator"), "{err}");
    }
}
