//! Unit tests for PHD2 events

use phd2_guider::{AppState, Phd2Event};

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

#[test]
fn test_connection_lost_event() {
    let event = Phd2Event::ConnectionLost {
        reason: "Connection closed by remote".to_string(),
    };
    match event {
        Phd2Event::ConnectionLost { reason } => {
            assert_eq!(reason, "Connection closed by remote");
        }
        _ => panic!("Expected ConnectionLost event"),
    }
}

#[test]
fn test_reconnecting_event() {
    let event = Phd2Event::Reconnecting {
        attempt: 3,
        max_attempts: Some(5),
    };
    match event {
        Phd2Event::Reconnecting {
            attempt,
            max_attempts,
        } => {
            assert_eq!(attempt, 3);
            assert_eq!(max_attempts, Some(5));
        }
        _ => panic!("Expected Reconnecting event"),
    }
}

#[test]
fn test_reconnected_event() {
    let event = Phd2Event::Reconnected;
    assert!(matches!(event, Phd2Event::Reconnected));
}

#[test]
fn test_reconnect_failed_event() {
    let event = Phd2Event::ReconnectFailed {
        reason: "Max retries exceeded".to_string(),
    };
    match event {
        Phd2Event::ReconnectFailed { reason } => {
            assert_eq!(reason, "Max retries exceeded");
        }
        _ => panic!("Expected ReconnectFailed event"),
    }
}

// ============================================================================
// AppState Display Tests
// ============================================================================

#[test]
fn test_app_state_display() {
    assert_eq!(format!("{}", AppState::Stopped), "Stopped");
    assert_eq!(format!("{}", AppState::Selected), "Selected");
    assert_eq!(format!("{}", AppState::Calibrating), "Calibrating");
    assert_eq!(format!("{}", AppState::Guiding), "Guiding");
    assert_eq!(format!("{}", AppState::LostLock), "LostLock");
    assert_eq!(format!("{}", AppState::Paused), "Paused");
    assert_eq!(format!("{}", AppState::Looping), "Looping");
}

#[test]
fn test_app_state_from_str_all_states() {
    assert_eq!("Stopped".parse::<AppState>().unwrap(), AppState::Stopped);
    assert_eq!("Selected".parse::<AppState>().unwrap(), AppState::Selected);
    assert_eq!(
        "Calibrating".parse::<AppState>().unwrap(),
        AppState::Calibrating
    );
    assert_eq!("Guiding".parse::<AppState>().unwrap(), AppState::Guiding);
    assert_eq!("LostLock".parse::<AppState>().unwrap(), AppState::LostLock);
    assert_eq!("Paused".parse::<AppState>().unwrap(), AppState::Paused);
    assert_eq!("Looping".parse::<AppState>().unwrap(), AppState::Looping);
}

// ============================================================================
// Additional Event Parsing Tests
// ============================================================================

#[test]
fn test_star_selected_event() {
    let json = r#"{"Event":"StarSelected","X":256.5,"Y":512.3}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::StarSelected { x, y } => {
            assert_eq!(x, 256.5);
            assert_eq!(y, 512.3);
        }
        _ => panic!("Expected StarSelected event"),
    }
}

#[test]
fn test_lock_position_set_event() {
    let json = r#"{"Event":"LockPositionSet","X":100.0,"Y":200.0}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::LockPositionSet { x, y } => {
            assert_eq!(x, 100.0);
            assert_eq!(y, 200.0);
        }
        _ => panic!("Expected LockPositionSet event"),
    }
}

#[test]
fn test_lock_position_shift_limit_reached_event() {
    let json = r#"{"Event":"LockPositionShiftLimitReached"}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    assert!(matches!(event, Phd2Event::LockPositionShiftLimitReached));
}

#[test]
fn test_calibrating_event() {
    let json = r#"{"Event":"Calibrating","Mount":"Mount","dir":"West","dist":10.5,"dx":5.0,"dy":2.0,"pos":[100.0,200.0],"step":3,"State":"calibrating"}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::Calibrating {
            mount,
            dir,
            dist,
            dx,
            dy,
            pos,
            step,
            state,
        } => {
            assert_eq!(mount, "Mount");
            assert_eq!(dir, "West");
            assert_eq!(dist, 10.5);
            assert_eq!(dx, 5.0);
            assert_eq!(dy, 2.0);
            assert_eq!(pos, vec![100.0, 200.0]);
            assert_eq!(step, 3);
            assert_eq!(state, "calibrating");
        }
        _ => panic!("Expected Calibrating event"),
    }
}

#[test]
fn test_calibration_complete_event() {
    let json = r#"{"Event":"CalibrationComplete","Mount":"Mount"}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::CalibrationComplete { mount } => {
            assert_eq!(mount, "Mount");
        }
        _ => panic!("Expected CalibrationComplete event"),
    }
}

#[test]
fn test_calibration_failed_event() {
    let json = r#"{"Event":"CalibrationFailed","Reason":"Star lost during calibration"}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::CalibrationFailed { reason } => {
            assert_eq!(reason, "Star lost during calibration");
        }
        _ => panic!("Expected CalibrationFailed event"),
    }
}

#[test]
fn test_calibration_data_flipped_event() {
    let json = r#"{"Event":"CalibrationDataFlipped","Mount":"Mount"}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::CalibrationDataFlipped { mount } => {
            assert_eq!(mount, "Mount");
        }
        _ => panic!("Expected CalibrationDataFlipped event"),
    }
}

#[test]
fn test_guide_param_change_event() {
    let json = r#"{"Event":"GuideParamChange","Name":"Aggressiveness","Value":0.75}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::GuideParamChange { name, value } => {
            assert_eq!(name, "Aggressiveness");
            assert_eq!(value, serde_json::json!(0.75));
        }
        _ => panic!("Expected GuideParamChange event"),
    }
}

#[test]
fn test_configuration_change_event() {
    let json = r#"{"Event":"ConfigurationChange"}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    assert!(matches!(event, Phd2Event::ConfigurationChange));
}

#[test]
fn test_alert_event() {
    let json = r#"{"Event":"Alert","Msg":"PHD2 message","Type":"info"}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::Alert { msg, alert_type } => {
            assert_eq!(msg, "PHD2 message");
            assert_eq!(alert_type, "info");
        }
        _ => panic!("Expected Alert event"),
    }
}

#[test]
fn test_version_event_with_overlap_support() {
    let json = r#"{"Event":"Version","PHDVersion":"2.6.11","PHDSubver":"dev3","MsgVersion":1,"OverlapSupport":true}"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::Version {
            phd_version,
            phd_subver,
            msg_version,
            overlap_support,
        } => {
            assert_eq!(phd_version, "2.6.11");
            assert_eq!(phd_subver, Some("dev3".to_string()));
            assert_eq!(msg_version, Some(1));
            assert_eq!(overlap_support, Some(true));
        }
        _ => panic!("Expected Version event"),
    }
}

#[test]
fn test_guide_step_with_all_fields() {
    let json = r#"{
        "Event":"GuideStep",
        "Frame":100,
        "Time":5.5,
        "Mount":"Mount",
        "dx":0.5,
        "dy":-0.3,
        "RADistanceRaw":0.4,
        "DECDistanceRaw":-0.2,
        "RADistanceGuide":0.35,
        "DECDistanceGuide":-0.15,
        "RADuration":150,
        "RADirection":"West",
        "DECDuration":100,
        "DECDirection":"North",
        "StarMass":15000.0,
        "SNR":25.5,
        "HFD":3.2,
        "AvgDist":0.45,
        "RALimited":false,
        "DecLimited":false,
        "ErrorCode":0
    }"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::GuideStep(stats) => {
            assert_eq!(stats.frame, 100);
            assert_eq!(stats.time, 5.5);
            assert_eq!(stats.mount, "Mount");
            assert_eq!(stats.dx, 0.5);
            assert_eq!(stats.dy, -0.3);
            assert_eq!(stats.ra_distance_raw, Some(0.4));
            assert_eq!(stats.dec_distance_raw, Some(-0.2));
            assert_eq!(stats.ra_distance_guide, Some(0.35));
            assert_eq!(stats.dec_distance_guide, Some(-0.15));
            assert_eq!(stats.ra_duration, Some(150));
            assert_eq!(stats.ra_direction, Some("West".to_string()));
            assert_eq!(stats.dec_duration, Some(100));
            assert_eq!(stats.dec_direction, Some("North".to_string()));
            assert_eq!(stats.star_mass, Some(15000.0));
            assert_eq!(stats.snr, Some(25.5));
            assert_eq!(stats.hfd, Some(3.2));
            assert_eq!(stats.avg_dist, Some(0.45));
            assert_eq!(stats.ra_limited, Some(false));
            assert_eq!(stats.dec_limited, Some(false));
            assert_eq!(stats.error_code, Some(0));
        }
        _ => panic!("Expected GuideStep event"),
    }
}

#[test]
fn test_star_lost_with_all_fields() {
    let json = r#"{
        "Event":"StarLost",
        "Frame":50,
        "Time":10.0,
        "StarMass":5000.0,
        "SNR":5.0,
        "AvgDist":2.5,
        "ErrorCode":1,
        "Status":"LostLock"
    }"#;
    let event: Phd2Event = serde_json::from_str(json).unwrap();
    match event {
        Phd2Event::StarLost {
            frame,
            time,
            star_mass,
            snr,
            avg_dist,
            error_code,
            status,
        } => {
            assert_eq!(frame, 50);
            assert_eq!(time, 10.0);
            assert_eq!(star_mass, 5000.0);
            assert_eq!(snr, 5.0);
            assert_eq!(avg_dist, Some(2.5));
            assert_eq!(error_code, Some(1));
            assert_eq!(status, "LostLock");
        }
        _ => panic!("Expected StarLost event"),
    }
}
