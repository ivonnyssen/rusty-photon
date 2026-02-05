//! Switch definitions for the PPBA device
//!
//! This module defines all switches exposed by the PPBA device via the ASCOM Switch interface.
//! Switches are numbered from 0 to MAX_SWITCH - 1.

/// Total number of switches exposed by the PPBA device
pub const MAX_SWITCH: usize = 16;

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
    /// Try to convert a usize to a SwitchId
    pub fn from_id(id: usize) -> Option<Self> {
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
    pub fn id(&self) -> usize {
        *self as usize
    }

    /// Get the switch information for this switch
    pub fn info(&self) -> SwitchInfo {
        let id = self.id();
        match self {
            // Controllable switches
            Self::Quad12V => SwitchInfo {
                id,
                name: "Quad 12V Output",
                description: "Controls the quad 12V power output",
                can_write: true,
                min_value: 0.0,
                max_value: 1.0,
                step: 1.0,
            },
            Self::AdjustableOutput => SwitchInfo {
                id,
                name: "Adjustable Output",
                description: "Controls the adjustable voltage output on/off",
                can_write: true,
                min_value: 0.0,
                max_value: 1.0,
                step: 1.0,
            },
            Self::DewHeaterA => SwitchInfo {
                id,
                name: "Dew Heater A",
                description: "PWM control for Dew Heater A (0-255)",
                can_write: true,
                min_value: 0.0,
                max_value: 255.0,
                step: 1.0,
            },
            Self::DewHeaterB => SwitchInfo {
                id,
                name: "Dew Heater B",
                description: "PWM control for Dew Heater B (0-255)",
                can_write: true,
                min_value: 0.0,
                max_value: 255.0,
                step: 1.0,
            },
            Self::UsbHub => SwitchInfo {
                id,
                name: "USB Hub",
                description: "Controls the USB 2.0 hub power",
                can_write: true,
                min_value: 0.0,
                max_value: 1.0,
                step: 1.0,
            },
            Self::AutoDew => SwitchInfo {
                id,
                name: "Auto-Dew",
                description: "Enables automatic dew heater control",
                can_write: true,
                min_value: 0.0,
                max_value: 1.0,
                step: 1.0,
            },

            // Read-only switches - Power Statistics
            Self::AverageCurrent => SwitchInfo {
                id,
                name: "Average Current",
                description: "Average current draw in Amps",
                can_write: false,
                min_value: 0.0,
                max_value: 20.0,
                step: 0.01,
            },
            Self::AmpHours => SwitchInfo {
                id,
                name: "Amp Hours",
                description: "Cumulative amp-hours consumed",
                can_write: false,
                min_value: 0.0,
                max_value: 9999.0,
                step: 0.01,
            },
            Self::WattHours => SwitchInfo {
                id,
                name: "Watt Hours",
                description: "Cumulative watt-hours consumed",
                can_write: false,
                min_value: 0.0,
                max_value: 99999.0,
                step: 0.1,
            },
            Self::Uptime => SwitchInfo {
                id,
                name: "Uptime",
                description: "Device uptime in hours",
                can_write: false,
                min_value: 0.0,
                max_value: 99999.0,
                step: 0.01,
            },

            // Read-only switches - Sensor Data
            Self::InputVoltage => SwitchInfo {
                id,
                name: "Input Voltage",
                description: "Input voltage in Volts",
                can_write: false,
                min_value: 0.0,
                max_value: 15.0,
                step: 0.1,
            },
            Self::TotalCurrent => SwitchInfo {
                id,
                name: "Total Current",
                description: "Total current draw in Amps",
                can_write: false,
                min_value: 0.0,
                max_value: 20.0,
                step: 0.01,
            },
            Self::Temperature => SwitchInfo {
                id,
                name: "Temperature",
                description: "Ambient temperature in Celsius",
                can_write: false,
                min_value: -40.0,
                max_value: 60.0,
                step: 0.1,
            },
            Self::Humidity => SwitchInfo {
                id,
                name: "Humidity",
                description: "Relative humidity percentage",
                can_write: false,
                min_value: 0.0,
                max_value: 100.0,
                step: 1.0,
            },
            Self::Dewpoint => SwitchInfo {
                id,
                name: "Dewpoint",
                description: "Calculated dewpoint in Celsius",
                can_write: false,
                min_value: -40.0,
                max_value: 60.0,
                step: 0.1,
            },
            Self::PowerWarning => SwitchInfo {
                id,
                name: "Power Warning",
                description: "Power warning flag (overcurrent/short circuit)",
                can_write: false,
                min_value: 0.0,
                max_value: 1.0,
                step: 1.0,
            },
        }
    }
}

/// Information about a switch
#[derive(Debug, Clone)]
pub struct SwitchInfo {
    pub id: usize,
    pub name: &'static str,
    pub description: &'static str,
    pub can_write: bool,
    pub min_value: f64,
    pub max_value: f64,
    pub step: f64,
}
