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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_event_parsing() {
        let json = r#"{"Event":"Version","PHDVersion":"2.6.11","PHDSubver":"","MsgVersion":1}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::Version { phd_version, .. } => {
                assert_eq!(phd_version, "2.6.11");
            }
            _ => panic!("Expected Version event"),
        }
    }

    #[test]
    fn test_app_state_event_parsing() {
        let json = r#"{"Event":"AppState","State":"Guiding"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::AppState { state } => {
                assert_eq!(state, "Guiding");
            }
            _ => panic!("Expected AppState event"),
        }
    }

    #[test]
    fn test_app_state_from_str() {
        assert_eq!("Stopped".parse::<AppState>().unwrap(), AppState::Stopped);
        assert_eq!("Guiding".parse::<AppState>().unwrap(), AppState::Guiding);
        assert_eq!(
            "Calibrating".parse::<AppState>().unwrap(),
            AppState::Calibrating
        );
        assert!("Unknown".parse::<AppState>().is_err());
    }

    #[test]
    fn test_guide_step_parsing() {
        let json = r#"{"Event":"GuideStep","Frame":1,"Time":1.5,"Mount":"Mount","dx":0.5,"dy":-0.3,"RADistanceRaw":0.4,"DECDistanceRaw":-0.2}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::GuideStep(stats) => {
                assert_eq!(stats.frame, 1);
                assert_eq!(stats.dx, 0.5);
                assert_eq!(stats.dy, -0.3);
            }
            _ => panic!("Expected GuideStep event"),
        }
    }

    #[test]
    fn test_star_lost_event_parsing() {
        let json = r#"{"Event":"StarLost","Frame":10,"Time":5.0,"StarMass":1000.0,"SNR":15.5,"Status":"Lost"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::StarLost { frame, snr, .. } => {
                assert_eq!(frame, 10);
                assert_eq!(snr, 15.5);
            }
            _ => panic!("Expected StarLost event"),
        }
    }

    #[test]
    fn test_settle_done_event_success() {
        let json = r#"{"Event":"SettleDone","Status":0}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::SettleDone { status, error } => {
                assert_eq!(status, 0);
                assert!(error.is_none());
            }
            _ => panic!("Expected SettleDone event"),
        }
    }

    #[test]
    fn test_settle_done_event_failure() {
        let json = r#"{"Event":"SettleDone","Status":1,"Error":"Star lost during settle"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::SettleDone { status, error } => {
                assert_eq!(status, 1);
                assert_eq!(error.unwrap(), "Star lost during settle");
            }
            _ => panic!("Expected SettleDone event"),
        }
    }

    #[test]
    fn test_guiding_dithered_event() {
        let json = r#"{"Event":"GuidingDithered","dx":2.5,"dy":-1.3}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::GuidingDithered { dx, dy } => {
                assert_eq!(dx, 2.5);
                assert_eq!(dy, -1.3);
            }
            _ => panic!("Expected GuidingDithered event"),
        }
    }

    #[test]
    fn test_settling_event() {
        let json =
            r#"{"Event":"Settling","Distance":1.2,"Time":3.5,"SettleTime":10.0,"StarLocked":true}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::Settling {
                distance,
                time,
                settle_time,
                star_locked,
            } => {
                assert_eq!(distance, 1.2);
                assert_eq!(time, 3.5);
                assert_eq!(settle_time, 10.0);
                assert!(star_locked);
            }
            _ => panic!("Expected Settling event"),
        }
    }

    #[test]
    fn test_paused_event() {
        let json = r#"{"Event":"Paused"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        assert!(matches!(event, Phd2Event::Paused));
    }

    #[test]
    fn test_resumed_event() {
        let json = r#"{"Event":"Resumed"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        assert!(matches!(event, Phd2Event::Resumed));
    }

    #[test]
    fn test_start_guiding_event() {
        let json = r#"{"Event":"StartGuiding"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        assert!(matches!(event, Phd2Event::StartGuiding));
    }

    #[test]
    fn test_guiding_stopped_event() {
        let json = r#"{"Event":"GuidingStopped"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        assert!(matches!(event, Phd2Event::GuidingStopped));
    }

    #[test]
    fn test_looping_exposures_event() {
        let json = r#"{"Event":"LoopingExposures","Frame":42}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::LoopingExposures { frame } => {
                assert_eq!(frame, 42);
            }
            _ => panic!("Expected LoopingExposures event"),
        }
    }

    #[test]
    fn test_looping_exposures_stopped_event() {
        let json = r#"{"Event":"LoopingExposuresStopped"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        assert!(matches!(event, Phd2Event::LoopingExposuresStopped));
    }
}
