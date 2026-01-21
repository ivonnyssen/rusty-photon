//! Common types used across the PHD2 guider client

use serde::{Deserialize, Serialize};

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
}
