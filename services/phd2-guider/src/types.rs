//! Common types used across the PHD2 guider client

use serde::{Deserialize, Serialize};
use std::fmt;

/// Rectangle for specifying regions of interest
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    /// Create a new rectangle
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// PHD2 equipment profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: i32,
    pub name: String,
}

/// Information about a single piece of equipment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquipmentDevice {
    /// Name of the device
    pub name: String,
    /// Whether the device is currently connected
    pub connected: bool,
}

/// Current equipment configuration from PHD2
///
/// Contains information about all equipment devices in the current profile.
/// Fields are optional because not all equipment types may be configured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Equipment {
    /// Guide camera
    pub camera: Option<EquipmentDevice>,
    /// Primary mount
    pub mount: Option<EquipmentDevice>,
    /// Auxiliary mount (for dual mount setups)
    #[serde(rename = "aux_mount")]
    pub aux_mount: Option<EquipmentDevice>,
    /// Adaptive optics device
    #[serde(rename = "AO")]
    pub ao: Option<EquipmentDevice>,
    /// Rotator device
    pub rotator: Option<EquipmentDevice>,
}

/// Target for calibration operations (mount or adaptive optics)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationTarget {
    /// Primary mount
    Mount,
    /// Adaptive optics device
    AO,
    /// Both mount and AO (only valid for clear_calibration)
    Both,
}

impl CalibrationTarget {
    /// Get the string representation for get_calibration_data API (capitalized)
    pub fn to_get_api_string(&self) -> &'static str {
        match self {
            CalibrationTarget::Mount => "Mount",
            CalibrationTarget::AO => "AO",
            CalibrationTarget::Both => "Mount", // Default to Mount for get operations
        }
    }

    /// Get the string representation for clear_calibration API (lowercase)
    pub fn to_clear_api_string(&self) -> &'static str {
        match self {
            CalibrationTarget::Mount => "mount",
            CalibrationTarget::AO => "ao",
            CalibrationTarget::Both => "both",
        }
    }
}

impl fmt::Display for CalibrationTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalibrationTarget::Mount => write!(f, "Mount"),
            CalibrationTarget::AO => write!(f, "AO"),
            CalibrationTarget::Both => write!(f, "Both"),
        }
    }
}

/// Calibration data from PHD2
///
/// Contains the calibration parameters for either the mount or adaptive optics device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationData {
    /// Whether the device is calibrated
    pub calibrated: bool,
    /// RA axis angle in degrees
    #[serde(rename = "xAngle")]
    pub x_angle: f64,
    /// RA axis rate in pixels per second
    #[serde(rename = "xRate")]
    pub x_rate: f64,
    /// RA axis parity ("+" or "-")
    #[serde(rename = "xParity")]
    pub x_parity: String,
    /// Dec axis angle in degrees
    #[serde(rename = "yAngle")]
    pub y_angle: f64,
    /// Dec axis rate in pixels per second
    #[serde(rename = "yRate")]
    pub y_rate: f64,
    /// Dec axis parity ("+" or "-")
    #[serde(rename = "yParity")]
    pub y_parity: String,
    /// Declination at time of calibration (if available)
    #[serde(default)]
    pub declination: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_creation() {
        let rect = Rect::new(100, 200, 50, 50);
        assert_eq!(rect.x, 100);
        assert_eq!(rect.y, 200);
        assert_eq!(rect.width, 50);
        assert_eq!(rect.height, 50);
    }

    #[test]
    fn test_rect_serialization() {
        let rect = Rect::new(100, 200, 50, 50);
        let json = serde_json::to_value(&rect).unwrap();
        assert_eq!(json["x"], 100);
        assert_eq!(json["y"], 200);
        assert_eq!(json["width"], 50);
        assert_eq!(json["height"], 50);
    }

    #[test]
    fn test_profile_parsing() {
        let json = r#"{"id":1,"name":"Default Equipment"}"#;
        let profile: Profile = serde_json::from_str(json).unwrap();
        assert_eq!(profile.id, 1);
        assert_eq!(profile.name, "Default Equipment");
    }

    #[test]
    fn test_equipment_device_parsing() {
        let json = r#"{"name":"ZWO ASI120MM","connected":true}"#;
        let device: EquipmentDevice = serde_json::from_str(json).unwrap();
        assert_eq!(device.name, "ZWO ASI120MM");
        assert!(device.connected);
    }

    #[test]
    fn test_equipment_full_parsing() {
        let json = r#"{
            "camera": {"name": "ZWO ASI120MM", "connected": true},
            "mount": {"name": "ASCOM Telescope Driver", "connected": true},
            "aux_mount": null,
            "AO": null,
            "rotator": null
        }"#;
        let equipment: Equipment = serde_json::from_str(json).unwrap();

        let camera = equipment.camera.unwrap();
        assert_eq!(camera.name, "ZWO ASI120MM");
        assert!(camera.connected);

        let mount = equipment.mount.unwrap();
        assert_eq!(mount.name, "ASCOM Telescope Driver");
        assert!(mount.connected);

        assert!(equipment.aux_mount.is_none());
        assert!(equipment.ao.is_none());
        assert!(equipment.rotator.is_none());
    }

    #[test]
    fn test_equipment_with_ao_parsing() {
        let json = r#"{
            "camera": {"name": "Guide Camera", "connected": true},
            "mount": {"name": "Mount", "connected": true},
            "aux_mount": null,
            "AO": {"name": "StarlightXpress AO", "connected": true},
            "rotator": null
        }"#;
        let equipment: Equipment = serde_json::from_str(json).unwrap();

        assert!(equipment.camera.is_some());
        assert!(equipment.mount.is_some());
        assert!(equipment.aux_mount.is_none());

        let ao = equipment.ao.unwrap();
        assert_eq!(ao.name, "StarlightXpress AO");
        assert!(ao.connected);

        assert!(equipment.rotator.is_none());
    }

    #[test]
    fn test_calibration_target_get_api_string() {
        assert_eq!(CalibrationTarget::Mount.to_get_api_string(), "Mount");
        assert_eq!(CalibrationTarget::AO.to_get_api_string(), "AO");
        assert_eq!(CalibrationTarget::Both.to_get_api_string(), "Mount");
    }

    #[test]
    fn test_calibration_target_clear_api_string() {
        assert_eq!(CalibrationTarget::Mount.to_clear_api_string(), "mount");
        assert_eq!(CalibrationTarget::AO.to_clear_api_string(), "ao");
        assert_eq!(CalibrationTarget::Both.to_clear_api_string(), "both");
    }

    #[test]
    fn test_calibration_target_display() {
        assert_eq!(format!("{}", CalibrationTarget::Mount), "Mount");
        assert_eq!(format!("{}", CalibrationTarget::AO), "AO");
        assert_eq!(format!("{}", CalibrationTarget::Both), "Both");
    }

    #[test]
    fn test_calibration_data_parsing() {
        let json = r#"{
            "calibrated": true,
            "xAngle": 45.0,
            "xRate": 15.5,
            "xParity": "+",
            "yAngle": 135.0,
            "yRate": 14.2,
            "yParity": "-",
            "declination": 30.5
        }"#;
        let data: CalibrationData = serde_json::from_str(json).unwrap();

        assert!(data.calibrated);
        assert_eq!(data.x_angle, 45.0);
        assert_eq!(data.x_rate, 15.5);
        assert_eq!(data.x_parity, "+");
        assert_eq!(data.y_angle, 135.0);
        assert_eq!(data.y_rate, 14.2);
        assert_eq!(data.y_parity, "-");
        assert_eq!(data.declination, Some(30.5));
    }

    #[test]
    fn test_calibration_data_without_declination() {
        let json = r#"{
            "calibrated": true,
            "xAngle": 45.0,
            "xRate": 15.5,
            "xParity": "+",
            "yAngle": 135.0,
            "yRate": 14.2,
            "yParity": "-"
        }"#;
        let data: CalibrationData = serde_json::from_str(json).unwrap();

        assert!(data.calibrated);
        assert!(data.declination.is_none());
    }

    #[test]
    fn test_calibration_data_not_calibrated() {
        let json = r#"{
            "calibrated": false,
            "xAngle": 0.0,
            "xRate": 0.0,
            "xParity": "+",
            "yAngle": 0.0,
            "yRate": 0.0,
            "yParity": "+"
        }"#;
        let data: CalibrationData = serde_json::from_str(json).unwrap();

        assert!(!data.calibrated);
        assert_eq!(data.x_angle, 0.0);
        assert_eq!(data.x_rate, 0.0);
    }
}
