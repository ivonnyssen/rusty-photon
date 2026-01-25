//! Switch definitions for the PPBA device
//!
//! This module defines all switches exposed by the PPBA device via the ASCOM Switch interface.
//! Switches are numbered from 0 to MAX_SWITCH - 1.

/// Total number of switches exposed by the PPBA device
pub const MAX_SWITCH: u16 = 16;

/// Switch identifiers for the PPBA device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchId {
    // Controllable switches (CanWrite = true)
    /// Quad 12V output (boolean: 0=off, 1=on)
    Quad12V = 0,
    /// Adjustable output (boolean: 0=off, 1=on)
    AdjustableOutput = 1,
    /// Dew Heater A PWM (analog: 0-255)
    DewHeaterA = 2,
    /// Dew Heater B PWM (analog: 0-255)
    DewHeaterB = 3,
    /// USB Hub control (boolean: 0=off, 1=on)
    UsbHub = 4,
    /// Auto-Dew enable (boolean: 0=off, 1=on)
    AutoDew = 5,

    // Read-only switches - Power Statistics (from PS command)
    /// Average current draw in Amps
    AverageCurrent = 6,
    /// Cumulative amp-hours consumed
    AmpHours = 7,
    /// Cumulative watt-hours consumed
    WattHours = 8,
    /// Device uptime in hours
    Uptime = 9,

    // Read-only switches - Sensor Data (from PA command)
    /// Input voltage in Volts
    InputVoltage = 10,
    /// Total current draw in Amps
    TotalCurrent = 11,
    /// Ambient temperature in Celsius
    Temperature = 12,
    /// Relative humidity percentage
    Humidity = 13,
    /// Calculated dewpoint in Celsius
    Dewpoint = 14,
    /// Power warning flag (overcurrent/short)
    PowerWarning = 15,
}

impl SwitchId {
    /// Try to convert a u16 to a SwitchId
    pub fn from_id(id: u16) -> Option<Self> {
        match id {
            0 => Some(Self::Quad12V),
            1 => Some(Self::AdjustableOutput),
            2 => Some(Self::DewHeaterA),
            3 => Some(Self::DewHeaterB),
            4 => Some(Self::UsbHub),
            5 => Some(Self::AutoDew),
            6 => Some(Self::AverageCurrent),
            7 => Some(Self::AmpHours),
            8 => Some(Self::WattHours),
            9 => Some(Self::Uptime),
            10 => Some(Self::InputVoltage),
            11 => Some(Self::TotalCurrent),
            12 => Some(Self::Temperature),
            13 => Some(Self::Humidity),
            14 => Some(Self::Dewpoint),
            15 => Some(Self::PowerWarning),
            _ => None,
        }
    }

    /// Get the numeric ID for this switch
    pub fn id(&self) -> u16 {
        *self as u16
    }
}

/// Information about a switch
#[derive(Debug, Clone)]
pub struct SwitchInfo {
    pub id: u16,
    pub name: &'static str,
    pub description: &'static str,
    pub can_write: bool,
    pub min_value: f64,
    pub max_value: f64,
    pub step: f64,
}

/// Get switch information for a given switch ID
pub fn get_switch_info(id: u16) -> Option<SwitchInfo> {
    let switch_id = SwitchId::from_id(id)?;
    Some(match switch_id {
        // Controllable switches
        SwitchId::Quad12V => SwitchInfo {
            id,
            name: "Quad 12V Output",
            description: "Controls the quad 12V power output",
            can_write: true,
            min_value: 0.0,
            max_value: 1.0,
            step: 1.0,
        },
        SwitchId::AdjustableOutput => SwitchInfo {
            id,
            name: "Adjustable Output",
            description: "Controls the adjustable voltage output on/off",
            can_write: true,
            min_value: 0.0,
            max_value: 1.0,
            step: 1.0,
        },
        SwitchId::DewHeaterA => SwitchInfo {
            id,
            name: "Dew Heater A",
            description: "PWM control for Dew Heater A (0-255)",
            can_write: true,
            min_value: 0.0,
            max_value: 255.0,
            step: 1.0,
        },
        SwitchId::DewHeaterB => SwitchInfo {
            id,
            name: "Dew Heater B",
            description: "PWM control for Dew Heater B (0-255)",
            can_write: true,
            min_value: 0.0,
            max_value: 255.0,
            step: 1.0,
        },
        SwitchId::UsbHub => SwitchInfo {
            id,
            name: "USB Hub",
            description: "Controls the USB 2.0 hub power",
            can_write: true,
            min_value: 0.0,
            max_value: 1.0,
            step: 1.0,
        },
        SwitchId::AutoDew => SwitchInfo {
            id,
            name: "Auto-Dew",
            description: "Enables automatic dew heater control",
            can_write: true,
            min_value: 0.0,
            max_value: 1.0,
            step: 1.0,
        },

        // Read-only switches - Power Statistics
        SwitchId::AverageCurrent => SwitchInfo {
            id,
            name: "Average Current",
            description: "Average current draw in Amps",
            can_write: false,
            min_value: 0.0,
            max_value: 20.0,
            step: 0.01,
        },
        SwitchId::AmpHours => SwitchInfo {
            id,
            name: "Amp Hours",
            description: "Cumulative amp-hours consumed",
            can_write: false,
            min_value: 0.0,
            max_value: 9999.0,
            step: 0.01,
        },
        SwitchId::WattHours => SwitchInfo {
            id,
            name: "Watt Hours",
            description: "Cumulative watt-hours consumed",
            can_write: false,
            min_value: 0.0,
            max_value: 99999.0,
            step: 0.1,
        },
        SwitchId::Uptime => SwitchInfo {
            id,
            name: "Uptime",
            description: "Device uptime in hours",
            can_write: false,
            min_value: 0.0,
            max_value: 99999.0,
            step: 0.01,
        },

        // Read-only switches - Sensor Data
        SwitchId::InputVoltage => SwitchInfo {
            id,
            name: "Input Voltage",
            description: "Input voltage in Volts",
            can_write: false,
            min_value: 0.0,
            max_value: 15.0,
            step: 0.1,
        },
        SwitchId::TotalCurrent => SwitchInfo {
            id,
            name: "Total Current",
            description: "Total current draw in Amps",
            can_write: false,
            min_value: 0.0,
            max_value: 20.0,
            step: 0.01,
        },
        SwitchId::Temperature => SwitchInfo {
            id,
            name: "Temperature",
            description: "Ambient temperature in Celsius",
            can_write: false,
            min_value: -40.0,
            max_value: 60.0,
            step: 0.1,
        },
        SwitchId::Humidity => SwitchInfo {
            id,
            name: "Humidity",
            description: "Relative humidity percentage",
            can_write: false,
            min_value: 0.0,
            max_value: 100.0,
            step: 1.0,
        },
        SwitchId::Dewpoint => SwitchInfo {
            id,
            name: "Dewpoint",
            description: "Calculated dewpoint in Celsius",
            can_write: false,
            min_value: -40.0,
            max_value: 60.0,
            step: 0.1,
        },
        SwitchId::PowerWarning => SwitchInfo {
            id,
            name: "Power Warning",
            description: "Power warning flag (overcurrent/short circuit)",
            can_write: false,
            min_value: 0.0,
            max_value: 1.0,
            step: 1.0,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_switch_id_from_id_valid() {
        assert_eq!(SwitchId::from_id(0), Some(SwitchId::Quad12V));
        assert_eq!(SwitchId::from_id(5), Some(SwitchId::AutoDew));
        assert_eq!(SwitchId::from_id(15), Some(SwitchId::PowerWarning));
    }

    #[test]
    fn test_switch_id_from_id_invalid() {
        assert_eq!(SwitchId::from_id(16), None);
        assert_eq!(SwitchId::from_id(100), None);
    }

    #[test]
    fn test_switch_id_roundtrip() {
        for id in 0..MAX_SWITCH {
            let switch_id = SwitchId::from_id(id).unwrap();
            assert_eq!(switch_id.id(), id);
        }
    }

    #[test]
    fn test_get_switch_info_all_switches() {
        for id in 0..MAX_SWITCH {
            let info = get_switch_info(id).unwrap();
            assert_eq!(info.id, id);
            assert!(!info.name.is_empty());
            assert!(!info.description.is_empty());
            assert!(info.min_value <= info.max_value);
            assert!(info.step > 0.0);
        }
    }

    #[test]
    fn test_get_switch_info_invalid() {
        assert!(get_switch_info(16).is_none());
        assert!(get_switch_info(100).is_none());
    }

    #[test]
    fn test_controllable_switches_can_write() {
        // Switches 0-5 should be writable
        for id in 0..6 {
            let info = get_switch_info(id).unwrap();
            assert!(info.can_write, "Switch {} should be writable", id);
        }
    }

    #[test]
    fn test_readonly_switches_cannot_write() {
        // Switches 6-15 should be read-only
        for id in 6..16 {
            let info = get_switch_info(id).unwrap();
            assert!(!info.can_write, "Switch {} should be read-only", id);
        }
    }

    #[test]
    fn test_boolean_switches_range() {
        // Boolean switches: 0, 1, 4, 5, 15
        let boolean_switches = [0, 1, 4, 5, 15];
        for id in boolean_switches {
            let info = get_switch_info(id).unwrap();
            assert_eq!(info.min_value, 0.0, "Switch {} min should be 0", id);
            assert_eq!(info.max_value, 1.0, "Switch {} max should be 1", id);
            assert_eq!(info.step, 1.0, "Switch {} step should be 1", id);
        }
    }

    #[test]
    fn test_pwm_switches_range() {
        // PWM switches: 2, 3 (Dew Heaters)
        let pwm_switches = [2, 3];
        for id in pwm_switches {
            let info = get_switch_info(id).unwrap();
            assert_eq!(info.min_value, 0.0, "Switch {} min should be 0", id);
            assert_eq!(info.max_value, 255.0, "Switch {} max should be 255", id);
            assert_eq!(info.step, 1.0, "Switch {} step should be 1", id);
        }
    }
}
