//! QHY Q-Focuser JSON command/response protocol
//!
//! The Q-Focuser communicates via JSON objects over serial. Each command has a
//! `cmd_id` field, and responses echo the command ID in an `idx` field.

use serde_json::Value;
use tracing::debug;

use crate::error::{QhyFocuserError, Result};

/// Commands that can be sent to the QHY Q-Focuser
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Get firmware and board version (cmd_id: 1)
    GetVersion,
    /// Relative move (cmd_id: 2)
    RelativeMove {
        /// Direction: 1 = inward, -1 = outward
        direction: i8,
        steps: u32,
    },
    /// Abort current movement (cmd_id: 3)
    Abort,
    /// Read temperature and voltage (cmd_id: 4)
    ReadTemperature,
    /// Get current position (cmd_id: 5)
    GetPosition,
    /// Move to absolute position (cmd_id: 6)
    AbsoluteMove { position: i64 },
    /// Set reverse direction (cmd_id: 7)
    SetReverse { enabled: bool },
    /// Sync position counter (cmd_id: 11)
    SyncPosition { position: i64 },
    /// Set movement speed (cmd_id: 13)
    SetSpeed { speed: u8 },
    /// Set hold current (cmd_id: 16)
    SetHoldCurrent { ihold: u8, irun: u8 },
    /// Set power-down mode (cmd_id: 19)
    SetPdnMode { pdn: u8 },
}

impl Command {
    /// Get the command ID for this command
    pub fn cmd_id(&self) -> u8 {
        match self {
            Command::GetVersion => 1,
            Command::RelativeMove { .. } => 2,
            Command::Abort => 3,
            Command::ReadTemperature => 4,
            Command::GetPosition => 5,
            Command::AbsoluteMove { .. } => 6,
            Command::SetReverse { .. } => 7,
            Command::SyncPosition { .. } => 11,
            Command::SetSpeed { .. } => 13,
            Command::SetHoldCurrent { .. } => 16,
            Command::SetPdnMode { .. } => 19,
        }
    }

    /// Serialize the command to a JSON string for serial transmission
    pub fn to_json_string(&self) -> String {
        let json = match self {
            Command::GetVersion => {
                serde_json::json!({"cmd_id": 1})
            }
            Command::RelativeMove { direction, steps } => {
                serde_json::json!({
                    "cmd_id": 2,
                    "dir": direction,
                    "step": steps
                })
            }
            Command::Abort => {
                serde_json::json!({"cmd_id": 3})
            }
            Command::ReadTemperature => {
                serde_json::json!({"cmd_id": 4})
            }
            Command::GetPosition => {
                serde_json::json!({"cmd_id": 5})
            }
            Command::AbsoluteMove { position } => {
                serde_json::json!({
                    "cmd_id": 6,
                    "tar": position
                })
            }
            Command::SetReverse { enabled } => {
                serde_json::json!({
                    "cmd_id": 7,
                    "rev": if *enabled { 1 } else { 0 }
                })
            }
            Command::SyncPosition { position } => {
                serde_json::json!({
                    "cmd_id": 11,
                    "init_val": position
                })
            }
            Command::SetSpeed { speed } => {
                serde_json::json!({
                    "cmd_id": 13,
                    "speed": speed
                })
            }
            Command::SetHoldCurrent { ihold, irun } => {
                serde_json::json!({
                    "cmd_id": 16,
                    "ihold": ihold,
                    "irun": irun
                })
            }
            Command::SetPdnMode { pdn } => {
                serde_json::json!({
                    "cmd_id": 19,
                    "pdn_d": pdn
                })
            }
        };
        json.to_string()
    }
}

/// Parsed version response from cmd_id 1
#[derive(Debug, Clone, PartialEq)]
pub struct VersionResponse {
    pub firmware_version: String,
    pub board_version: String,
}

/// Parsed temperature/voltage response from cmd_id 4
#[derive(Debug, Clone, PartialEq)]
pub struct TemperatureResponse {
    /// Outer temperature in degrees Celsius
    pub outer_temp: f64,
    /// Chip temperature in degrees Celsius
    pub chip_temp: f64,
    /// Input voltage in volts
    pub voltage: f64,
}

/// Parsed position response from cmd_id 5
#[derive(Debug, Clone, PartialEq)]
pub struct PositionResponse {
    pub position: i64,
}

/// Parse a JSON response string and validate the `idx` (command ID) field
pub fn parse_response(response: &str, expected_cmd_id: u8) -> Result<Value> {
    debug!("Parsing response: {}", response);

    let value: Value = serde_json::from_str(response)
        .map_err(|e| QhyFocuserError::InvalidResponse(format!("Invalid JSON: {}", e)))?;

    let idx = value
        .get("idx")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| QhyFocuserError::InvalidResponse("Missing 'idx' field".to_string()))?;

    if idx != expected_cmd_id as u64 {
        return Err(QhyFocuserError::InvalidResponse(format!(
            "Expected idx {}, got {}",
            expected_cmd_id, idx
        )));
    }

    Ok(value)
}

/// Parse a version response (cmd_id 1)
pub fn parse_version_response(response: &str) -> Result<VersionResponse> {
    let value = parse_response(response, 1)?;

    let firmware_version = value
        .get("firmware_version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let board_version = value
        .get("board_version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(VersionResponse {
        firmware_version,
        board_version,
    })
}

/// Parse a temperature response (cmd_id 4)
///
/// Raw values from the device: temp values divided by 1000, voltage by 10
pub fn parse_temperature_response(response: &str) -> Result<TemperatureResponse> {
    let value = parse_response(response, 4)?;

    let outer_temp_raw = value
        .get("o_t")
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
        .ok_or_else(|| QhyFocuserError::ParseError("Missing 'o_t' field".to_string()))?;

    let chip_temp_raw = value
        .get("c_t")
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
        .ok_or_else(|| QhyFocuserError::ParseError("Missing 'c_t' field".to_string()))?;

    let voltage_raw = value
        .get("c_r")
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
        .ok_or_else(|| QhyFocuserError::ParseError("Missing 'c_r' field".to_string()))?;

    Ok(TemperatureResponse {
        outer_temp: outer_temp_raw / 1000.0,
        chip_temp: chip_temp_raw / 1000.0,
        voltage: voltage_raw / 10.0,
    })
}

/// Parse a position response (cmd_id 5)
pub fn parse_position_response(response: &str) -> Result<PositionResponse> {
    let value = parse_response(response, 5)?;

    let position = value
        .get("pos")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| QhyFocuserError::ParseError("Missing 'pos' field".to_string()))?;

    Ok(PositionResponse { position })
}
