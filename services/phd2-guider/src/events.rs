//! PHD2 event types and application state

use serde::{Deserialize, Serialize};

use crate::error::Phd2Error;

/// PHD2 application state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppState {
    Stopped,
    Selected,
    Calibrating,
    Guiding,
    LostLock,
    Paused,
    Looping,
}

impl std::fmt::Display for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppState::Stopped => write!(f, "Stopped"),
            AppState::Selected => write!(f, "Selected"),
            AppState::Calibrating => write!(f, "Calibrating"),
            AppState::Guiding => write!(f, "Guiding"),
            AppState::LostLock => write!(f, "LostLock"),
            AppState::Paused => write!(f, "Paused"),
            AppState::Looping => write!(f, "Looping"),
        }
    }
}

impl std::str::FromStr for AppState {
    type Err = Phd2Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "Stopped" => Ok(AppState::Stopped),
            "Selected" => Ok(AppState::Selected),
            "Calibrating" => Ok(AppState::Calibrating),
            "Guiding" => Ok(AppState::Guiding),
            "LostLock" => Ok(AppState::LostLock),
            "Paused" => Ok(AppState::Paused),
            "Looping" => Ok(AppState::Looping),
            _ => Err(Phd2Error::InvalidState(format!("Unknown state: {}", s))),
        }
    }
}

/// Guide step statistics from PHD2
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GuideStepStats {
    pub frame: u64,
    pub time: f64,
    pub mount: String,
    #[serde(rename = "dx")]
    pub dx: f64,
    #[serde(rename = "dy")]
    pub dy: f64,
    #[serde(rename = "RADistanceRaw")]
    pub ra_distance_raw: Option<f64>,
    #[serde(rename = "DECDistanceRaw")]
    pub dec_distance_raw: Option<f64>,
    #[serde(rename = "RADistanceGuide")]
    pub ra_distance_guide: Option<f64>,
    #[serde(rename = "DECDistanceGuide")]
    pub dec_distance_guide: Option<f64>,
    #[serde(rename = "RADuration")]
    pub ra_duration: Option<i32>,
    #[serde(rename = "RADirection")]
    pub ra_direction: Option<String>,
    #[serde(rename = "DECDuration")]
    pub dec_duration: Option<i32>,
    #[serde(rename = "DECDirection")]
    pub dec_direction: Option<String>,
    #[serde(rename = "StarMass")]
    pub star_mass: Option<f64>,
    #[serde(rename = "SNR")]
    pub snr: Option<f64>,
    #[serde(rename = "HFD")]
    pub hfd: Option<f64>,
    #[serde(rename = "AvgDist")]
    pub avg_dist: Option<f64>,
    #[serde(rename = "RALimited")]
    pub ra_limited: Option<bool>,
    #[serde(rename = "DecLimited")]
    pub dec_limited: Option<bool>,
    #[serde(rename = "ErrorCode")]
    pub error_code: Option<i32>,
}

/// PHD2 event notification
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "Event")]
pub enum Phd2Event {
    /// Sent on connection, contains PHD2 version info
    Version {
        #[serde(rename = "PHDVersion")]
        phd_version: String,
        #[serde(rename = "PHDSubver")]
        phd_subver: Option<String>,
        #[serde(rename = "MsgVersion")]
        msg_version: Option<u32>,
        #[serde(rename = "OverlapSupport")]
        overlap_support: Option<bool>,
    },

    /// Application state changed
    AppState {
        #[serde(rename = "State")]
        state: String,
    },

    /// Guide step with statistics
    GuideStep(GuideStepStats),

    /// Dither operation completed
    GuidingDithered {
        #[serde(rename = "dx")]
        dx: f64,
        #[serde(rename = "dy")]
        dy: f64,
    },

    /// Settling completed
    SettleDone {
        #[serde(rename = "Status")]
        status: i32,
        #[serde(rename = "Error")]
        error: Option<String>,
    },

    /// Star was selected
    StarSelected {
        #[serde(rename = "X")]
        x: f64,
        #[serde(rename = "Y")]
        y: f64,
    },

    /// Star was lost
    StarLost {
        #[serde(rename = "Frame")]
        frame: u64,
        #[serde(rename = "Time")]
        time: f64,
        #[serde(rename = "StarMass")]
        star_mass: f64,
        #[serde(rename = "SNR")]
        snr: f64,
        #[serde(rename = "AvgDist")]
        avg_dist: Option<f64>,
        #[serde(rename = "ErrorCode")]
        error_code: Option<i32>,
        #[serde(rename = "Status")]
        status: String,
    },

    /// Lock position was set
    LockPositionSet {
        #[serde(rename = "X")]
        x: f64,
        #[serde(rename = "Y")]
        y: f64,
    },

    /// Lock shift limit reached
    LockPositionShiftLimitReached,

    /// Calibration in progress
    Calibrating {
        #[serde(rename = "Mount")]
        mount: String,
        #[serde(rename = "dir")]
        dir: String,
        #[serde(rename = "dist")]
        dist: f64,
        #[serde(rename = "dx")]
        dx: f64,
        #[serde(rename = "dy")]
        dy: f64,
        #[serde(rename = "pos")]
        pos: Vec<f64>,
        #[serde(rename = "step")]
        step: u32,
        #[serde(rename = "State")]
        state: String,
    },

    /// Calibration finished
    CalibrationComplete {
        #[serde(rename = "Mount")]
        mount: String,
    },

    /// Calibration failed
    CalibrationFailed {
        #[serde(rename = "Reason")]
        reason: String,
    },

    /// Calibration was flipped
    CalibrationDataFlipped {
        #[serde(rename = "Mount")]
        mount: String,
    },

    /// Looping exposures started
    LoopingExposures {
        #[serde(rename = "Frame")]
        frame: u64,
    },

    /// Looping exposures stopped
    LoopingExposuresStopped,

    /// Guiding was paused
    Paused,

    /// Guiding was resumed
    Resumed,

    /// Guide parameter changed
    GuideParamChange {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Value")]
        value: serde_json::Value,
    },

    /// Configuration changed
    ConfigurationChange,

    /// Alert message
    Alert {
        #[serde(rename = "Msg")]
        msg: String,
        #[serde(rename = "Type")]
        alert_type: String,
    },

    /// Start guiding event
    StartGuiding,

    /// Settling in progress
    Settling {
        #[serde(rename = "Distance")]
        distance: f64,
        #[serde(rename = "Time")]
        time: f64,
        #[serde(rename = "SettleTime")]
        settle_time: f64,
        #[serde(rename = "StarLocked")]
        star_locked: bool,
    },

    /// Guiding stopped
    GuidingStopped,

    // ========================================================================
    // Connection state events (internal, not from PHD2)
    // ========================================================================
    /// Connection to PHD2 was lost
    #[serde(skip)]
    ConnectionLost {
        /// Reason for the connection loss
        reason: String,
    },

    /// Attempting to reconnect to PHD2
    #[serde(skip)]
    Reconnecting {
        /// Current reconnection attempt number
        attempt: u32,
        /// Maximum attempts (None for unlimited)
        max_attempts: Option<u32>,
    },

    /// Successfully reconnected to PHD2
    #[serde(skip)]
    Reconnected,

    /// Reconnection failed after all attempts
    #[serde(skip)]
    ReconnectFailed {
        /// Reason for final failure
        reason: String,
    },
}
