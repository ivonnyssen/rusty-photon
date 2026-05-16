//! Falcon Rotator wire protocol
//!
//! ASCII commands over 9600-8N1 serial, all LF-terminated in both directions.
//! See `docs/services/falcon-rotator.md#protocol-reference` for the source
//! command table.

use crate::error::Result;

/// Commands the driver issues to the Falcon Rotator.
///
/// Variants intentionally omit `FF` (firmware reload) — the design doc lists
/// it as a maintenance operation the driver never sends.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `F#` — ping / status check. Expect `FR_OK`.
    Ping,
    /// `FA` — full status. Expect `FR_OK:steps:deg:moving:limit:derot:reverse`.
    FullStatus,
    /// `FV` — firmware version. Expect `FV:n.n`.
    FirmwareVersion,
    /// `FD` — current position in degrees. Expect `FD:nn.nn`.
    PositionDeg,
    /// `FP` — current position in steps. Expect `FP:n..`.
    PositionSteps,
    /// `VS` — input voltage (raw / unscaled). Expect `VS:n..`.
    Voltage,
    /// `DR:0` — disable de-rotation.
    DerotationOff,
    /// `DR:<ms>` — enable de-rotation at `ms` ms/step.
    DerotationRate(u32),
    /// `SD:<deg>` — device-side sync (rewrite stored counter).
    SyncDeg(f64),
    /// `MD:<deg>` — move to absolute degrees.
    MoveDeg(f64),
    /// `MS:<steps>` — move to absolute steps.
    MoveSteps(u32),
    /// `FH` — halt. Expect `FH:1`.
    Halt,
    /// `FR` — is running. Expect `FR:1` or `FR:0`.
    IsRunning,
    /// `FN:<b>` — set motor reverse (persisted in EEPROM).
    SetReverse(bool),
}

impl Command {
    /// Render the command into the exact ASCII payload the Falcon expects
    /// (without the LF terminator; the writer appends that).
    pub fn to_command_string(&self) -> String {
        let _ = self;
        unimplemented!("Command::to_command_string is implemented in Phase 3a")
    }
}

/// Parsed Falcon `FA` full-status response.
///
/// Wire format: `FR_OK:position_in_steps:position_in_deg:is_moving:limit_detect:do_derotation:motor_reverse`
#[derive(Debug, Clone, PartialEq)]
pub struct FalconStatus {
    pub position_steps: u32,
    pub position_deg: f64,
    pub is_moving: bool,
    pub limit_detect: bool,
    pub do_derotation: bool,
    pub motor_reverse: bool,
}

/// Parse the `FR_OK:...` response from `FA`.
pub fn parse_full_status(_response: &str) -> Result<FalconStatus> {
    unimplemented!("parse_full_status is implemented in Phase 3a")
}

/// Parse the `FV:n.n` firmware version response.
pub fn parse_firmware_version(_response: &str) -> Result<String> {
    unimplemented!("parse_firmware_version is implemented in Phase 3a")
}

/// Parse the `FD:nn.nn` degrees response.
pub fn parse_position_deg(_response: &str) -> Result<f64> {
    unimplemented!("parse_position_deg is implemented in Phase 3a")
}

/// Parse the `FP:n..` steps response.
pub fn parse_position_steps(_response: &str) -> Result<u32> {
    unimplemented!("parse_position_steps is implemented in Phase 3a")
}

/// Parse the `VS:n..` raw voltage response.
pub fn parse_voltage_raw(_response: &str) -> Result<u32> {
    unimplemented!("parse_voltage_raw is implemented in Phase 3a")
}

/// Parse the `FR:0` / `FR:1` is-running response.
pub fn parse_is_running(_response: &str) -> Result<bool> {
    unimplemented!("parse_is_running is implemented in Phase 3a")
}

/// Validate a `FR_OK` ping response (with optional trailing whitespace).
pub fn validate_ping_response(_response: &str) -> Result<()> {
    unimplemented!("validate_ping_response is implemented in Phase 3a")
}

/// Validate that `response` echoes the issued `command` (for `MD:`, `SD:`, `DR:`, `FH`, `FN:`).
pub fn validate_echo(_command: &Command, _response: &str) -> Result<()> {
    unimplemented!("validate_echo is implemented in Phase 3a")
}
