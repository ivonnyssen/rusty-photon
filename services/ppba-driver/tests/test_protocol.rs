//! Protocol parsing tests for PPBA Switch driver

use ppba_driver::protocol::{
    parse_power_stats_response, parse_status_response, validate_ping_response,
    validate_set_response, PpbaCommand,
};

mod status_parsing {
    use super::*;

    #[test]
    fn parses_valid_status_response() {
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
    fn parses_status_with_trailing_newline() {
        let response = "PPBA:12.0:2.0:20.0:50:10.0:0:1:0:255:0:1:12\n";
        let status = parse_status_response(response).unwrap();

        assert_eq!(status.voltage, 12.0);
        assert!(!status.quad_12v);
        assert!(status.adjustable_output);
        assert_eq!(status.dew_b, 255);
        assert!(status.power_warning);
        assert_eq!(status.power_adj, 12);
    }

    #[test]
    fn parses_status_with_negative_temperature() {
        let response = "PPBA:11.8:1.5:-5.0:80:-10.2:1:1:100:100:1:0:5";
        let status = parse_status_response(response).unwrap();

        assert_eq!(status.temperature, -5.0);
        assert_eq!(status.dewpoint, -10.2);
    }

    #[test]
    fn rejects_invalid_prefix() {
        let response = "INVALID:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0";
        let result = parse_status_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_too_few_fields() {
        let response = "PPBA:12.5:3.2:25.0";
        let result = parse_status_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_invalid_float_field() {
        let response = "PPBA:invalid:3.2:25.0:60:15.5:1:0:128:64:1:0:0";
        let result = parse_status_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_invalid_boolean_field() {
        let response = "PPBA:12.5:3.2:25.0:60:15.5:2:0:128:64:1:0:0";
        let result = parse_status_response(response);
        assert!(result.is_err());
    }
}

mod power_stats_parsing {
    use super::*;

    #[test]
    fn parses_valid_power_stats() {
        let response = "PS:2.5:10.5:126.0:3600000";
        let stats = parse_power_stats_response(response).unwrap();

        assert_eq!(stats.average_amps, 2.5);
        assert_eq!(stats.amp_hours, 10.5);
        assert_eq!(stats.watt_hours, 126.0);
        assert_eq!(stats.uptime_ms, 3600000);
    }

    #[test]
    fn calculates_uptime_hours_correctly() {
        let response = "PS:1.0:5.0:60.0:7200000"; // 2 hours
        let stats = parse_power_stats_response(response).unwrap();

        assert_eq!(stats.uptime_hours(), 2.0);
    }

    #[test]
    fn parses_power_stats_with_newline() {
        let response = "PS:0.5:1.0:12.0:1800000\n";
        let stats = parse_power_stats_response(response).unwrap();

        assert_eq!(stats.average_amps, 0.5);
        assert_eq!(stats.uptime_hours(), 0.5);
    }

    #[test]
    fn rejects_invalid_prefix() {
        let response = "INVALID:2.5:10.5:126.0:3600000";
        let result = parse_power_stats_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_too_few_fields() {
        let response = "PS:2.5:10.5";
        let result = parse_power_stats_response(response);
        assert!(result.is_err());
    }
}

mod ping_validation {
    use super::*;

    #[test]
    fn accepts_valid_ping_response() {
        assert!(validate_ping_response("PPBA_OK").is_ok());
    }

    #[test]
    fn accepts_ping_with_newline() {
        assert!(validate_ping_response("PPBA_OK\n").is_ok());
    }

    #[test]
    fn rejects_invalid_ping_response() {
        assert!(validate_ping_response("INVALID").is_err());
        assert!(validate_ping_response("").is_err());
        assert!(validate_ping_response("PPBA_ERROR").is_err());
    }
}

mod command_serialization {
    use super::*;

    #[test]
    fn serializes_ping_command() {
        assert_eq!(PpbaCommand::Ping.to_command_string(), "P#");
    }

    #[test]
    fn serializes_firmware_version_command() {
        assert_eq!(PpbaCommand::FirmwareVersion.to_command_string(), "PV");
    }

    #[test]
    fn serializes_status_command() {
        assert_eq!(PpbaCommand::Status.to_command_string(), "PA");
    }

    #[test]
    fn serializes_power_stats_command() {
        assert_eq!(PpbaCommand::PowerStats.to_command_string(), "PS");
    }

    #[test]
    fn serializes_set_quad_12v_on() {
        assert_eq!(PpbaCommand::SetQuad12V(true).to_command_string(), "P1:1");
    }

    #[test]
    fn serializes_set_quad_12v_off() {
        assert_eq!(PpbaCommand::SetQuad12V(false).to_command_string(), "P1:0");
    }

    #[test]
    fn serializes_set_adjustable() {
        assert_eq!(PpbaCommand::SetAdjustable(true).to_command_string(), "P2:1");
        assert_eq!(
            PpbaCommand::SetAdjustable(false).to_command_string(),
            "P2:0"
        );
    }

    #[test]
    fn serializes_set_dew_a_pwm() {
        assert_eq!(PpbaCommand::SetDewA(0).to_command_string(), "P3:0");
        assert_eq!(PpbaCommand::SetDewA(128).to_command_string(), "P3:128");
        assert_eq!(PpbaCommand::SetDewA(255).to_command_string(), "P3:255");
    }

    #[test]
    fn serializes_set_dew_b_pwm() {
        assert_eq!(PpbaCommand::SetDewB(0).to_command_string(), "P4:0");
        assert_eq!(PpbaCommand::SetDewB(128).to_command_string(), "P4:128");
        assert_eq!(PpbaCommand::SetDewB(255).to_command_string(), "P4:255");
    }

    #[test]
    fn serializes_set_usb_hub() {
        assert_eq!(PpbaCommand::SetUsbHub(true).to_command_string(), "PU:1");
        assert_eq!(PpbaCommand::SetUsbHub(false).to_command_string(), "PU:0");
    }

    #[test]
    fn serializes_set_auto_dew() {
        assert_eq!(PpbaCommand::SetAutoDew(true).to_command_string(), "PD:1");
        assert_eq!(PpbaCommand::SetAutoDew(false).to_command_string(), "PD:0");
    }
}

mod set_response_validation {
    use super::*;

    #[test]
    fn validates_matching_response() {
        let cmd = PpbaCommand::SetQuad12V(true);
        assert!(validate_set_response(&cmd, "P1:1").is_ok());
    }

    #[test]
    fn validates_response_with_newline() {
        let cmd = PpbaCommand::SetDewA(128);
        assert!(validate_set_response(&cmd, "P3:128\n").is_ok());
    }

    #[test]
    fn rejects_mismatched_response() {
        let cmd = PpbaCommand::SetQuad12V(true);
        assert!(validate_set_response(&cmd, "P1:0").is_err());
    }
}
