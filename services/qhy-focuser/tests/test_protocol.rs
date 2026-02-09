//! Tests for the QHY Q-Focuser protocol module

use qhy_focuser::protocol::*;

// ============================================================================
// Command serialization tests
// ============================================================================

#[test]
fn test_get_version_command() {
    let cmd = Command::GetVersion;
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 1);
    assert_eq!(cmd.cmd_id(), 1);
}

#[test]
fn test_relative_move_command() {
    let cmd = Command::RelativeMove {
        direction: 1,
        speed: 3,
        steps: 1000,
    };
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 2);
    assert_eq!(parsed["direction"], 1);
    assert_eq!(parsed["speed"], 3);
    assert_eq!(parsed["steps"], 1000);
    assert_eq!(cmd.cmd_id(), 2);
}

#[test]
fn test_abort_command() {
    let cmd = Command::Abort;
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 3);
    assert_eq!(cmd.cmd_id(), 3);
}

#[test]
fn test_read_temperature_command() {
    let cmd = Command::ReadTemperature;
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 4);
    assert_eq!(cmd.cmd_id(), 4);
}

#[test]
fn test_get_position_command() {
    let cmd = Command::GetPosition;
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 5);
    assert_eq!(cmd.cmd_id(), 5);
}

#[test]
fn test_absolute_move_command() {
    let cmd = Command::AbsoluteMove {
        position: 32000,
        speed: 0,
    };
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 6);
    assert_eq!(parsed["position"], 32000);
    assert_eq!(parsed["speed"], 0);
    assert_eq!(cmd.cmd_id(), 6);
}

#[test]
fn test_absolute_move_negative_position() {
    let cmd = Command::AbsoluteMove {
        position: -5000,
        speed: 4,
    };
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["position"], -5000);
}

#[test]
fn test_set_reverse_enabled() {
    let cmd = Command::SetReverse { enabled: true };
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 7);
    assert_eq!(parsed["enabled"], 1);
}

#[test]
fn test_set_reverse_disabled() {
    let cmd = Command::SetReverse { enabled: false };
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["enabled"], 0);
}

#[test]
fn test_sync_position_command() {
    let cmd = Command::SyncPosition { position: 10000 };
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 11);
    assert_eq!(parsed["position"], 10000);
}

#[test]
fn test_set_speed_command() {
    let cmd = Command::SetSpeed { speed: 5 };
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 13);
    assert_eq!(parsed["speed"], 5);
}

#[test]
fn test_set_hold_current_command() {
    let cmd = Command::SetHoldCurrent {
        ihold: 10,
        irun: 20,
    };
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 16);
    assert_eq!(parsed["ihold"], 10);
    assert_eq!(parsed["irun"], 20);
}

#[test]
fn test_set_pdn_mode_command() {
    let cmd = Command::SetPdnMode { pdn: 1 };
    let json = cmd.to_json_string();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["cmd_id"], 19);
    assert_eq!(parsed["pdn"], 1);
}

// ============================================================================
// Response parsing tests
// ============================================================================

#[test]
fn test_parse_response_valid_idx() {
    let response = r#"{"idx": 5, "position": 1000}"#;
    let value = parse_response(response, 5).unwrap();
    assert_eq!(value["idx"], 5);
}

#[test]
fn test_parse_response_wrong_idx() {
    let response = r#"{"idx": 3, "position": 1000}"#;
    let err = parse_response(response, 5).unwrap_err();
    assert!(err.to_string().contains("Expected idx 5, got 3"));
}

#[test]
fn test_parse_response_missing_idx() {
    let response = r#"{"position": 1000}"#;
    let err = parse_response(response, 5).unwrap_err();
    assert!(err.to_string().contains("Missing 'idx' field"));
}

#[test]
fn test_parse_response_invalid_json() {
    let err = parse_response("not json", 5).unwrap_err();
    assert!(err.to_string().contains("Invalid JSON"));
}

#[test]
fn test_parse_version_response() {
    let response = r#"{"idx": 1, "firmware_version": "2.1.0", "board_version": "1.0"}"#;
    let version = parse_version_response(response).unwrap();
    assert_eq!(version.firmware_version, "2.1.0");
    assert_eq!(version.board_version, "1.0");
}

#[test]
fn test_parse_version_response_missing_fields() {
    let response = r#"{"idx": 1}"#;
    let version = parse_version_response(response).unwrap();
    assert_eq!(version.firmware_version, "unknown");
    assert_eq!(version.board_version, "unknown");
}

#[test]
fn test_parse_temperature_response() {
    // Raw: o_t=25000 -> 25.0°C, c_t=30000 -> 30.0°C, c_r=125 -> 12.5V
    let response = r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#;
    let temp = parse_temperature_response(response).unwrap();
    assert!((temp.outer_temp - 25.0).abs() < 0.001);
    assert!((temp.chip_temp - 30.0).abs() < 0.001);
    assert!((temp.voltage - 12.5).abs() < 0.001);
}

#[test]
fn test_parse_temperature_response_negative_temp() {
    // Raw: o_t=-5000 -> -5.0°C
    let response = r#"{"idx": 4, "o_t": -5000, "c_t": 10000, "c_r": 120}"#;
    let temp = parse_temperature_response(response).unwrap();
    assert!((temp.outer_temp - (-5.0)).abs() < 0.001);
}

#[test]
fn test_parse_temperature_response_missing_field() {
    let response = r#"{"idx": 4, "o_t": 25000, "c_t": 30000}"#;
    let err = parse_temperature_response(response).unwrap_err();
    assert!(err.to_string().contains("c_r"));
}

#[test]
fn test_parse_position_response() {
    let response = r#"{"idx": 5, "position": 32000}"#;
    let pos = parse_position_response(response).unwrap();
    assert_eq!(pos.position, 32000);
}

#[test]
fn test_parse_position_response_negative() {
    let response = r#"{"idx": 5, "position": -10000}"#;
    let pos = parse_position_response(response).unwrap();
    assert_eq!(pos.position, -10000);
}

#[test]
fn test_parse_position_response_zero() {
    let response = r#"{"idx": 5, "position": 0}"#;
    let pos = parse_position_response(response).unwrap();
    assert_eq!(pos.position, 0);
}

#[test]
fn test_parse_position_response_missing_field() {
    let response = r#"{"idx": 5}"#;
    let err = parse_position_response(response).unwrap_err();
    assert!(err.to_string().contains("position"));
}

// ============================================================================
// Command properties tests
// ============================================================================

#[test]
fn test_command_clone() {
    let cmd = Command::AbsoluteMove {
        position: 100,
        speed: 0,
    };
    let cloned = cmd.clone();
    assert_eq!(cmd, cloned);
}

#[test]
fn test_command_debug() {
    let cmd = Command::GetVersion;
    let debug = format!("{:?}", cmd);
    assert!(debug.contains("GetVersion"));
}

#[test]
fn test_version_response_clone() {
    let resp = VersionResponse {
        firmware_version: "1.0".to_string(),
        board_version: "2.0".to_string(),
    };
    let cloned = resp.clone();
    assert_eq!(resp, cloned);
}

#[test]
fn test_temperature_response_debug() {
    let resp = TemperatureResponse {
        outer_temp: 25.0,
        chip_temp: 30.0,
        voltage: 12.5,
    };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("25.0"));
}

#[test]
fn test_position_response_debug() {
    let resp = PositionResponse { position: 1000 };
    let debug = format!("{:?}", resp);
    assert!(debug.contains("1000"));
}
