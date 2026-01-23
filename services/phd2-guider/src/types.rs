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

/// Guide axis for algorithm parameter operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideAxis {
    /// Right Ascension axis
    Ra,
    /// Declination axis
    Dec,
}

impl GuideAxis {
    /// Get the string representation for the PHD2 API
    pub fn to_api_string(&self) -> &'static str {
        match self {
            GuideAxis::Ra => "ra",
            GuideAxis::Dec => "dec",
        }
    }
}

impl fmt::Display for GuideAxis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GuideAxis::Ra => write!(f, "RA"),
            GuideAxis::Dec => write!(f, "Dec"),
        }
    }
}

/// Camera cooler status from PHD2
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolerStatus {
    /// Current sensor temperature in degrees Celsius
    pub temperature: f64,
    /// Whether the cooler is enabled
    #[serde(rename = "coolerOn")]
    pub cooler_on: bool,
    /// Target temperature setpoint in degrees Celsius (if cooler is on)
    pub setpoint: Option<f64>,
    /// Cooler power percentage (0-100)
    pub power: Option<f64>,
}

/// Star image data from PHD2
///
/// Contains the guide star image as base64-encoded data along with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarImage {
    /// Frame number
    pub frame: u64,
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// Star position X coordinate
    #[serde(rename = "star_pos")]
    pub star_pos: Option<Vec<f64>>,
    /// Base64-encoded image data
    pub pixels: String,
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
