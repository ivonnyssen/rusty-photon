//! Unit tests for PHD2 types

use phd2_guider::{
    CalibrationData, CalibrationTarget, CoolerStatus, Equipment, EquipmentDevice, GuideAxis,
    Profile, Rect, StarImage,
};

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

// ============================================================================
// GuideAxis Tests
// ============================================================================

#[test]
fn test_guide_axis_to_api_string() {
    assert_eq!(GuideAxis::Ra.to_api_string(), "ra");
    assert_eq!(GuideAxis::Dec.to_api_string(), "dec");
}

#[test]
fn test_guide_axis_display() {
    assert_eq!(format!("{}", GuideAxis::Ra), "RA");
    assert_eq!(format!("{}", GuideAxis::Dec), "Dec");
}

#[test]
fn test_guide_axis_equality() {
    assert_eq!(GuideAxis::Ra, GuideAxis::Ra);
    assert_eq!(GuideAxis::Dec, GuideAxis::Dec);
    assert_ne!(GuideAxis::Ra, GuideAxis::Dec);
}

// ============================================================================
// CoolerStatus Tests
// ============================================================================

#[test]
fn test_cooler_status_parsing_full() {
    let json = r#"{
        "temperature": -10.5,
        "coolerOn": true,
        "setpoint": -10.0,
        "power": 45.5
    }"#;
    let status: CoolerStatus = serde_json::from_str(json).unwrap();

    assert_eq!(status.temperature, -10.5);
    assert!(status.cooler_on);
    assert_eq!(status.setpoint, Some(-10.0));
    assert_eq!(status.power, Some(45.5));
}

#[test]
fn test_cooler_status_parsing_cooler_off() {
    let json = r#"{
        "temperature": 25.0,
        "coolerOn": false
    }"#;
    let status: CoolerStatus = serde_json::from_str(json).unwrap();

    assert_eq!(status.temperature, 25.0);
    assert!(!status.cooler_on);
    assert!(status.setpoint.is_none());
    assert!(status.power.is_none());
}

// ============================================================================
// StarImage Tests
// ============================================================================

#[test]
fn test_star_image_parsing_full() {
    let json = r#"{
        "frame": 42,
        "width": 32,
        "height": 32,
        "star_pos": [16.5, 15.3],
        "pixels": "AAAA"
    }"#;
    let image: StarImage = serde_json::from_str(json).unwrap();

    assert_eq!(image.frame, 42);
    assert_eq!(image.width, 32);
    assert_eq!(image.height, 32);
    assert_eq!(image.star_pos, Some(vec![16.5, 15.3]));
    assert_eq!(image.pixels, "AAAA");
}

#[test]
fn test_star_image_parsing_no_star_pos() {
    let json = r#"{
        "frame": 1,
        "width": 64,
        "height": 64,
        "pixels": "AAABBBCCC"
    }"#;
    let image: StarImage = serde_json::from_str(json).unwrap();

    assert_eq!(image.frame, 1);
    assert_eq!(image.width, 64);
    assert_eq!(image.height, 64);
    assert!(image.star_pos.is_none());
    assert_eq!(image.pixels, "AAABBBCCC");
}
