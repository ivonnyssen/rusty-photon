use serde::Deserialize;
use serde_json::Value;

use super::camera::CameraConfig;
use super::cover_calibrator::CoverCalibratorConfig;
use super::filter_wheel::FilterWheelConfig;
use super::focuser::FocuserConfig;
use super::mount::MountConfig;

#[derive(Debug, Clone, Deserialize)]
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
    pub safety_monitors: Vec<Value>,
}
