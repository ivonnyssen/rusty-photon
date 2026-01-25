//! PPBA protocol implementation
//!
//! This module handles the serial protocol for the Pegasus Astro Pocket Powerbox Advance Gen2.
//!
//! Serial Settings: 9600 baud, 8N1, newline-terminated commands
//!
//! Commands return their echo followed by data, terminated by newline.

use crate::error::{PpbaError, Result};

/// Commands that can be sent to the PPBA device
#[derive(Debug, Clone, PartialEq)]
pub enum PpbaCommand {
    /// Ping/status check - returns PPBA_OK
    Ping,
    /// Get firmware version - returns n.n.n
    FirmwareVersion,
    /// Get full status (PA command)
    Status,
    /// Get power statistics (PS command)
    PowerStats,
    /// Set quad 12V output (0/1)
    SetQuad12V(bool),
    /// Set adjustable output (0=off, 1=on)
    SetAdjustable(bool),
    /// Set Dew Heater A PWM (0-255)
    SetDewA(u8),
    /// Set Dew Heater B PWM (0-255)
    SetDewB(u8),
    /// Set USB hub power (0/1)
    SetUsbHub(bool),
    /// Set Auto-Dew enable (0/1)
    SetAutoDew(bool),
}

impl PpbaCommand {
    /// Serialize the command to a string to send to the device
    pub fn to_command_string(&self) -> String {
        match self {
            PpbaCommand::Ping => "P#".to_string(),
            PpbaCommand::FirmwareVersion => "PV".to_string(),
            PpbaCommand::Status => "PA".to_string(),
            PpbaCommand::PowerStats => "PS".to_string(),
            PpbaCommand::SetQuad12V(on) => format!("P1:{}", if *on { 1 } else { 0 }),
            PpbaCommand::SetAdjustable(on) => format!("P2:{}", if *on { 1 } else { 0 }),
            PpbaCommand::SetDewA(pwm) => format!("P3:{}", pwm),
            PpbaCommand::SetDewB(pwm) => format!("P4:{}", pwm),
            PpbaCommand::SetUsbHub(on) => format!("PU:{}", if *on { 1 } else { 0 }),
            PpbaCommand::SetAutoDew(on) => format!("PD:{}", if *on { 1 } else { 0 }),
        }
    }
}

/// Parsed status response from the PA command
///
/// Response format: `PPBA:voltage:current:temp:humidity:dewpoint:quad:adj:dewA:dewB:autodew:warn:pwradj`
#[derive(Debug, Clone, Default)]
pub struct PpbaStatus {
    /// Input voltage in Volts
    pub voltage: f64,
    /// Total current in Amps
    pub current: f64,
    /// Temperature in Celsius
    pub temperature: f64,
    /// Humidity percentage
    pub humidity: f64,
    /// Dewpoint in Celsius
    pub dewpoint: f64,
    /// Quad 12V output state (0=off, 1=on)
    pub quad_12v: bool,
    /// Adjustable output state (0=off, 1=on)
    pub adjustable_output: bool,
    /// Dew Heater A PWM value (0-255)
    pub dew_a: u8,
    /// Dew Heater B PWM value (0-255)
    pub dew_b: u8,
    /// Auto-Dew enabled
    pub auto_dew: bool,
    /// Power warning flag
    pub power_warning: bool,
    /// Adjustable output power level
    pub power_adj: u8,
}

/// Parsed power statistics response from the PS command
///
/// Response format: `PS:averageAmps:ampHours:wattHours:uptime_ms`
#[derive(Debug, Clone, Default)]
pub struct PpbaPowerStats {
    /// Average current in Amps
    pub average_amps: f64,
    /// Cumulative amp-hours
    pub amp_hours: f64,
    /// Cumulative watt-hours
    pub watt_hours: f64,
    /// Uptime in milliseconds
    pub uptime_ms: u64,
}

impl PpbaPowerStats {
    /// Get uptime in hours
    pub fn uptime_hours(&self) -> f64 {
        self.uptime_ms as f64 / 3_600_000.0
    }
}

/// Parse the PA status response
///
/// Expected format: `PPBA:voltage:current:temp:humidity:dewpoint:quad:adj:dewA:dewB:autodew:warn:pwradj`
pub fn parse_status_response(response: &str) -> Result<PpbaStatus> {
    let response = response.trim();

    // Check prefix
    if !response.starts_with("PPBA:") {
        return Err(PpbaError::InvalidResponse(format!(
            "Expected PPBA: prefix, got: {}",
            response
        )));
    }

    // Split by colon and skip the "PPBA" prefix
    let parts: Vec<&str> = response.split(':').collect();

    // Expect 13 parts: PPBA + 12 values
    if parts.len() < 13 {
        return Err(PpbaError::InvalidResponse(format!(
            "Expected 13 parts in PA response, got {}: {}",
            parts.len(),
            response
        )));
    }

    let voltage = parse_f64(parts[1], "voltage")?;
    let current = parse_f64(parts[2], "current")?;
    let temperature = parse_f64(parts[3], "temperature")?;
    let humidity = parse_f64(parts[4], "humidity")?;
    let dewpoint = parse_f64(parts[5], "dewpoint")?;
    let quad_12v = parse_bool(parts[6], "quad_12v")?;
    let adjustable_output = parse_bool(parts[7], "adjustable_output")?;
    let dew_a = parse_u8(parts[8], "dew_a")?;
    let dew_b = parse_u8(parts[9], "dew_b")?;
    let auto_dew = parse_bool(parts[10], "auto_dew")?;
    let power_warning = parse_bool(parts[11], "power_warning")?;
    let power_adj = parse_u8(parts[12], "power_adj")?;

    Ok(PpbaStatus {
        voltage,
        current,
        temperature,
        humidity,
        dewpoint,
        quad_12v,
        adjustable_output,
        dew_a,
        dew_b,
        auto_dew,
        power_warning,
        power_adj,
    })
}

/// Parse the PS power statistics response
///
/// Expected format: `PS:averageAmps:ampHours:wattHours:uptime_ms`
pub fn parse_power_stats_response(response: &str) -> Result<PpbaPowerStats> {
    let response = response.trim();

    // Check prefix
    if !response.starts_with("PS:") {
        return Err(PpbaError::InvalidResponse(format!(
            "Expected PS: prefix, got: {}",
            response
        )));
    }

    // Split by colon and skip the "PS" prefix
    let parts: Vec<&str> = response.split(':').collect();

    // Expect 5 parts: PS + 4 values
    if parts.len() < 5 {
        return Err(PpbaError::InvalidResponse(format!(
            "Expected 5 parts in PS response, got {}: {}",
            parts.len(),
            response
        )));
    }

    let average_amps = parse_f64(parts[1], "average_amps")?;
    let amp_hours = parse_f64(parts[2], "amp_hours")?;
    let watt_hours = parse_f64(parts[3], "watt_hours")?;
    let uptime_ms = parse_u64(parts[4], "uptime_ms")?;

    Ok(PpbaPowerStats {
        average_amps,
        amp_hours,
        watt_hours,
        uptime_ms,
    })
}

/// Validate a ping response
pub fn validate_ping_response(response: &str) -> Result<()> {
    let response = response.trim();
    if response == "PPBA_OK" {
        Ok(())
    } else {
        Err(PpbaError::InvalidResponse(format!(
            "Expected PPBA_OK, got: {}",
            response
        )))
    }
}

/// Parse a set command response (echo of the command)
///
/// For example, `P1:1` for SetQuad12V(true)
pub fn validate_set_response(command: &PpbaCommand, response: &str) -> Result<()> {
    let expected = command.to_command_string();
    let response = response.trim();

    if response == expected {
        Ok(())
    } else {
        Err(PpbaError::InvalidResponse(format!(
            "Expected {}, got: {}",
            expected, response
        )))
    }
}

// Helper parsing functions
fn parse_f64(s: &str, field: &str) -> Result<f64> {
    s.parse::<f64>()
        .map_err(|_| PpbaError::ParseError(format!("Invalid {} value: {}", field, s)))
}

fn parse_u8(s: &str, field: &str) -> Result<u8> {
    s.parse::<u8>()
        .map_err(|_| PpbaError::ParseError(format!("Invalid {} value: {}", field, s)))
}

fn parse_u64(s: &str, field: &str) -> Result<u64> {
    s.parse::<u64>()
        .map_err(|_| PpbaError::ParseError(format!("Invalid {} value: {}", field, s)))
}

fn parse_bool(s: &str, field: &str) -> Result<bool> {
    match s {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(PpbaError::ParseError(format!(
            "Invalid {} boolean value: {}",
            field, s
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_to_string() {
        assert_eq!(PpbaCommand::Ping.to_command_string(), "P#");
        assert_eq!(PpbaCommand::FirmwareVersion.to_command_string(), "PV");
        assert_eq!(PpbaCommand::Status.to_command_string(), "PA");
        assert_eq!(PpbaCommand::PowerStats.to_command_string(), "PS");
        assert_eq!(PpbaCommand::SetQuad12V(true).to_command_string(), "P1:1");
        assert_eq!(PpbaCommand::SetQuad12V(false).to_command_string(), "P1:0");
        assert_eq!(PpbaCommand::SetAdjustable(true).to_command_string(), "P2:1");
        assert_eq!(PpbaCommand::SetDewA(128).to_command_string(), "P3:128");
        assert_eq!(PpbaCommand::SetDewB(255).to_command_string(), "P4:255");
        assert_eq!(PpbaCommand::SetUsbHub(true).to_command_string(), "PU:1");
        assert_eq!(PpbaCommand::SetAutoDew(false).to_command_string(), "PD:0");
    }

    #[test]
    fn test_parse_status_response_valid() {
        let response = "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0";
        let status = parse_status_response(response).unwrap();

        assert_eq!(status.voltage, 12.5);
        assert_eq!(status.current, 3.2);
        assert_eq!(status.temperature, 25.0);
        assert_eq!(status.humidity, 60.0);
        assert_eq!(status.dewpoint, 15.5);
        assert!(status.quad_12v);
        assert!(!status.adjustable_output);
        assert_eq!(status.dew_a, 128);
        assert_eq!(status.dew_b, 64);
        assert!(status.auto_dew);
        assert!(!status.power_warning);
        assert_eq!(status.power_adj, 0);
    }

    #[test]
    fn test_parse_status_response_with_newline() {
        let response = "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0\n";
        let status = parse_status_response(response).unwrap();
        assert_eq!(status.voltage, 12.5);
    }

    #[test]
    fn test_parse_status_response_invalid_prefix() {
        let response = "INVALID:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0";
        let result = parse_status_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_status_response_too_few_parts() {
        let response = "PPBA:12.5:3.2";
        let result = parse_status_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_power_stats_valid() {
        let response = "PS:2.5:10.5:126.0:3600000";
        let stats = parse_power_stats_response(response).unwrap();

        assert_eq!(stats.average_amps, 2.5);
        assert_eq!(stats.amp_hours, 10.5);
        assert_eq!(stats.watt_hours, 126.0);
        assert_eq!(stats.uptime_ms, 3600000);
        assert_eq!(stats.uptime_hours(), 1.0);
    }

    #[test]
    fn test_parse_power_stats_invalid_prefix() {
        let response = "INVALID:2.5:10.5:126.0:3600000";
        let result = parse_power_stats_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_ping_response_valid() {
        assert!(validate_ping_response("PPBA_OK").is_ok());
        assert!(validate_ping_response("PPBA_OK\n").is_ok());
    }

    #[test]
    fn test_validate_ping_response_invalid() {
        assert!(validate_ping_response("INVALID").is_err());
        assert!(validate_ping_response("").is_err());
    }

    #[test]
    fn test_validate_set_response_valid() {
        let cmd = PpbaCommand::SetQuad12V(true);
        assert!(validate_set_response(&cmd, "P1:1").is_ok());
        assert!(validate_set_response(&cmd, "P1:1\n").is_ok());
    }

    #[test]
    fn test_validate_set_response_invalid() {
        let cmd = PpbaCommand::SetQuad12V(true);
        assert!(validate_set_response(&cmd, "P1:0").is_err());
    }

    #[test]
    fn test_uptime_hours_calculation() {
        let stats = PpbaPowerStats {
            average_amps: 0.0,
            amp_hours: 0.0,
            watt_hours: 0.0,
            uptime_ms: 7_200_000, // 2 hours
        };
        assert_eq!(stats.uptime_hours(), 2.0);
    }
}
