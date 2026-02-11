//! Unit tests for PHD2 client

use phd2_guider::{
    CalibrationData, CalibrationTarget, Equipment, Phd2Client, Phd2Config, Rect, SettleParams,
};

#[test]
fn test_guide_request_params_format() {
    let settle = SettleParams::default();
    let settle_obj = serde_json::json!({
        "pixels": settle.pixels,
        "time": settle.time,
        "timeout": settle.timeout
    });
    let params = serde_json::json!({
        "settle": settle_obj,
        "recalibrate": false
    });

    assert!(params["settle"]["pixels"].as_f64().is_some());
    assert!(params["settle"]["time"].as_u64().is_some());
    assert!(params["settle"]["timeout"].as_u64().is_some());
    assert!(!params["recalibrate"].as_bool().unwrap());
}

#[test]
fn test_guide_request_with_roi() {
    let settle = SettleParams::default();
    let roi = Rect::new(100, 100, 200, 200);

    let settle_obj = serde_json::json!({
        "pixels": settle.pixels,
        "time": settle.time,
        "timeout": settle.timeout
    });
    let mut params = serde_json::json!({
        "settle": settle_obj,
        "recalibrate": true
    });
    params["roi"] = serde_json::json!([roi.x, roi.y, roi.width, roi.height]);

    let roi_arr = params["roi"].as_array().unwrap();
    assert_eq!(roi_arr.len(), 4);
    assert_eq!(roi_arr[0].as_i64().unwrap(), 100);
    assert_eq!(roi_arr[1].as_i64().unwrap(), 100);
    assert_eq!(roi_arr[2].as_i64().unwrap(), 200);
    assert_eq!(roi_arr[3].as_i64().unwrap(), 200);
}

#[test]
fn test_dither_request_params_format() {
    let settle = SettleParams::default();
    let settle_obj = serde_json::json!({
        "pixels": settle.pixels,
        "time": settle.time,
        "timeout": settle.timeout
    });
    let params = serde_json::json!({
        "amount": 5.0,
        "raOnly": true,
        "settle": settle_obj
    });

    assert_eq!(params["amount"].as_f64().unwrap(), 5.0);
    assert!(params["raOnly"].as_bool().unwrap());
    assert!(params["settle"]["pixels"].as_f64().is_some());
}

#[test]
fn test_pause_request_params_full() {
    let params = serde_json::json!({"paused": true, "full": "full"});
    assert!(params["paused"].as_bool().unwrap());
    assert_eq!(params["full"].as_str().unwrap(), "full");
}

#[test]
fn test_pause_request_params_partial() {
    let params = serde_json::json!({"paused": true});
    assert!(params["paused"].as_bool().unwrap());
    assert!(params.get("full").is_none());
}

#[test]
fn test_resume_request_params() {
    let params = serde_json::json!({"paused": false});
    assert!(!params["paused"].as_bool().unwrap());
}

#[test]
fn test_get_current_equipment_response_parsing() {
    // Simulate PHD2's get_current_equipment response
    let response_json = serde_json::json!({
        "camera": {"name": "ZWO ASI120MM Mini", "connected": true},
        "mount": {"name": "EQMOD ASCOM HEQ5/6", "connected": true},
        "aux_mount": null,
        "AO": null,
        "rotator": null
    });

    let equipment: Equipment = serde_json::from_value(response_json).unwrap();

    let camera = equipment.camera.unwrap();
    assert_eq!(camera.name, "ZWO ASI120MM Mini");
    assert!(camera.connected);

    let mount = equipment.mount.unwrap();
    assert_eq!(mount.name, "EQMOD ASCOM HEQ5/6");
    assert!(mount.connected);

    assert!(equipment.aux_mount.is_none());
    assert!(equipment.ao.is_none());
    assert!(equipment.rotator.is_none());
}

#[test]
fn test_client_auto_reconnect_default_enabled() {
    let config = Phd2Config::default();
    let client = Phd2Client::new(config);
    assert!(client.is_auto_reconnect_enabled());
}

#[test]
fn test_client_auto_reconnect_disabled_in_config() {
    use phd2_guider::ReconnectConfig;

    let config = Phd2Config {
        reconnect: ReconnectConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = Phd2Client::new(config);
    assert!(!client.is_auto_reconnect_enabled());
}

#[test]
fn test_client_toggle_auto_reconnect() {
    let config = Phd2Config::default();
    let client = Phd2Client::new(config);

    assert!(client.is_auto_reconnect_enabled());
    client.set_auto_reconnect_enabled(false);
    assert!(!client.is_auto_reconnect_enabled());
    client.set_auto_reconnect_enabled(true);
    assert!(client.is_auto_reconnect_enabled());
}

#[tokio::test]
async fn test_client_initial_state() {
    let config = Phd2Config::default();
    let client = Phd2Client::new(config);

    assert!(!client.is_connected().await);
    assert!(!client.is_reconnecting().await);
    assert!(client.get_phd2_version().await.is_none());
}

// ========================================================================
// Star Selection Method Tests
// ========================================================================

#[test]
fn test_find_star_request_params_no_roi() {
    let params: Option<serde_json::Value> = None;
    assert!(params.is_none());
}

#[test]
fn test_find_star_request_params_with_roi() {
    let roi = Rect::new(100, 200, 300, 400);
    let params = serde_json::json!([roi.x, roi.y, roi.width, roi.height]);

    let arr = params.as_array().unwrap();
    assert_eq!(arr.len(), 4);
    assert_eq!(arr[0].as_i64().unwrap(), 100);
    assert_eq!(arr[1].as_i64().unwrap(), 200);
    assert_eq!(arr[2].as_i64().unwrap(), 300);
    assert_eq!(arr[3].as_i64().unwrap(), 400);
}

#[test]
fn test_get_lock_position_response_parsing() {
    let response = serde_json::json!([256.5, 512.3]);
    let arr = response.as_array().unwrap();
    let x = arr[0].as_f64().unwrap();
    let y = arr[1].as_f64().unwrap();

    assert_eq!(x, 256.5);
    assert_eq!(y, 512.3);
}

#[test]
fn test_get_lock_position_null_response() {
    let response = serde_json::json!(null);
    assert!(response.is_null());
}

#[test]
fn test_set_lock_position_request_params() {
    let params = serde_json::json!({
        "X": 256.5,
        "Y": 512.3,
        "EXACT": true
    });

    assert_eq!(params["X"].as_f64().unwrap(), 256.5);
    assert_eq!(params["Y"].as_f64().unwrap(), 512.3);
    assert!(params["EXACT"].as_bool().unwrap());
}

#[test]
fn test_set_lock_position_request_params_not_exact() {
    let params = serde_json::json!({
        "X": 100.0,
        "Y": 200.0,
        "EXACT": false
    });

    assert_eq!(params["X"].as_f64().unwrap(), 100.0);
    assert_eq!(params["Y"].as_f64().unwrap(), 200.0);
    assert!(!params["EXACT"].as_bool().unwrap());
}

// ========================================================================
// Calibration Method Tests
// ========================================================================

#[test]
fn test_get_calibration_data_request_params_mount() {
    let params = serde_json::json!({
        "which": CalibrationTarget::Mount.to_get_api_string()
    });
    assert_eq!(params["which"].as_str().unwrap(), "Mount");
}

#[test]
fn test_get_calibration_data_request_params_ao() {
    let params = serde_json::json!({
        "which": CalibrationTarget::AO.to_get_api_string()
    });
    assert_eq!(params["which"].as_str().unwrap(), "AO");
}

#[test]
fn test_clear_calibration_request_params_mount() {
    let params = serde_json::json!({
        "which": CalibrationTarget::Mount.to_clear_api_string()
    });
    assert_eq!(params["which"].as_str().unwrap(), "mount");
}

#[test]
fn test_clear_calibration_request_params_ao() {
    let params = serde_json::json!({
        "which": CalibrationTarget::AO.to_clear_api_string()
    });
    assert_eq!(params["which"].as_str().unwrap(), "ao");
}

#[test]
fn test_clear_calibration_request_params_both() {
    let params = serde_json::json!({
        "which": CalibrationTarget::Both.to_clear_api_string()
    });
    assert_eq!(params["which"].as_str().unwrap(), "both");
}

#[test]
fn test_get_calibration_data_response_parsing() {
    let response = serde_json::json!({
        "calibrated": true,
        "xAngle": 45.5,
        "xRate": 15.2,
        "xParity": "+",
        "yAngle": 135.5,
        "yRate": 14.8,
        "yParity": "-",
        "declination": 30.0
    });

    let data: CalibrationData = serde_json::from_value(response).unwrap();
    assert!(data.calibrated);
    assert_eq!(data.x_angle, 45.5);
    assert_eq!(data.x_rate, 15.2);
    assert_eq!(data.x_parity, "+");
    assert_eq!(data.y_angle, 135.5);
    assert_eq!(data.y_rate, 14.8);
    assert_eq!(data.y_parity, "-");
    assert_eq!(data.declination, Some(30.0));
}

#[test]
fn test_get_calibration_data_response_not_calibrated() {
    let response = serde_json::json!({
        "calibrated": false,
        "xAngle": 0.0,
        "xRate": 0.0,
        "xParity": "+",
        "yAngle": 0.0,
        "yRate": 0.0,
        "yParity": "+"
    });

    let data: CalibrationData = serde_json::from_value(response).unwrap();
    assert!(!data.calibrated);
    assert_eq!(data.x_rate, 0.0);
    assert!(data.declination.is_none());
}

// ========================================================================
// Camera Exposure Method Tests
// ========================================================================

#[test]
fn test_get_exposure_response_parsing() {
    let response = serde_json::json!(2000);
    let exposure = response.as_u64().map(|v| v as u32).unwrap();
    assert_eq!(exposure, 2000);
}

#[test]
fn test_set_exposure_request_params() {
    let params = serde_json::json!(1500);
    assert_eq!(params.as_u64().unwrap(), 1500);
}

#[test]
fn test_get_exposure_durations_response_parsing() {
    let response = serde_json::json!([100, 200, 500, 1000, 2000, 3000, 5000]);
    let durations: Vec<u32> = serde_json::from_value(response).unwrap();
    assert_eq!(durations.len(), 7);
    assert_eq!(durations[0], 100);
    assert_eq!(durations[3], 1000);
    assert_eq!(durations[6], 5000);
}

#[test]
fn test_get_camera_frame_size_response_parsing() {
    let response = serde_json::json!([1280, 960]);
    let arr = response.as_array().unwrap();
    let width = arr[0].as_u64().map(|v| v as u32).unwrap();
    let height = arr[1].as_u64().map(|v| v as u32).unwrap();
    assert_eq!(width, 1280);
    assert_eq!(height, 960);
}

#[test]
fn test_get_use_subframes_response_parsing() {
    let response_true = serde_json::json!(true);
    assert!(response_true.as_bool().unwrap());

    let response_false = serde_json::json!(false);
    assert!(!response_false.as_bool().unwrap());
}

#[test]
fn test_capture_single_frame_no_params() {
    let params: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    assert!(params.is_empty());
}

#[test]
fn test_capture_single_frame_with_exposure() {
    let mut params = serde_json::Map::new();
    params.insert("exposure".to_string(), serde_json::json!(3000));
    assert_eq!(params["exposure"].as_u64().unwrap(), 3000);
}

#[test]
fn test_capture_single_frame_with_subframe() {
    let rect = Rect::new(100, 100, 200, 200);
    let mut params = serde_json::Map::new();
    params.insert(
        "subframe".to_string(),
        serde_json::json!([rect.x, rect.y, rect.width, rect.height]),
    );

    let subframe = params["subframe"].as_array().unwrap();
    assert_eq!(subframe.len(), 4);
    assert_eq!(subframe[0].as_i64().unwrap(), 100);
    assert_eq!(subframe[1].as_i64().unwrap(), 100);
    assert_eq!(subframe[2].as_i64().unwrap(), 200);
    assert_eq!(subframe[3].as_i64().unwrap(), 200);
}

#[test]
fn test_capture_single_frame_with_all_params() {
    let rect = Rect::new(50, 50, 256, 256);
    let mut params = serde_json::Map::new();
    params.insert("exposure".to_string(), serde_json::json!(2000));
    params.insert(
        "subframe".to_string(),
        serde_json::json!([rect.x, rect.y, rect.width, rect.height]),
    );

    assert_eq!(params["exposure"].as_u64().unwrap(), 2000);
    let subframe = params["subframe"].as_array().unwrap();
    assert_eq!(subframe[0].as_i64().unwrap(), 50);
    assert_eq!(subframe[2].as_i64().unwrap(), 256);
}
