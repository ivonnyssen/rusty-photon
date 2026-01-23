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
