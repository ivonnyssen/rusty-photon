//! Tests for the PpbaError type
//!
//! Covers Display formatting, From<io::Error> conversion, and Debug formatting
//! for all error variants.

use ppba_driver::error::PpbaError;

#[test]
fn test_error_display_not_connected() {
    let err = PpbaError::NotConnected;
    assert_eq!(format!("{}", err), "Not connected to PPBA");
}

#[test]
fn test_error_display_connection_failed() {
    let err = PpbaError::ConnectionFailed("port busy".to_string());
    assert_eq!(format!("{}", err), "Connection failed: port busy");
}

#[test]
fn test_error_display_serial_port() {
    let err = PpbaError::SerialPort("no such device".to_string());
    assert_eq!(format!("{}", err), "Serial port error: no such device");
}

#[test]
fn test_error_display_timeout() {
    let err = PpbaError::Timeout("read timed out".to_string());
    assert_eq!(format!("{}", err), "Timeout: read timed out");
}

#[test]
fn test_error_display_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe");
    let err = PpbaError::Io(io_err);
    assert_eq!(format!("{}", err), "IO error: broken pipe");
}

#[test]
fn test_error_display_invalid_response() {
    let err = PpbaError::InvalidResponse("bad format".to_string());
    assert_eq!(format!("{}", err), "Invalid response: bad format");
}

#[test]
fn test_error_display_parse_error() {
    let err = PpbaError::ParseError("not a number".to_string());
    assert_eq!(format!("{}", err), "Parse error: not a number");
}

#[test]
fn test_error_display_invalid_switch_id() {
    let err = PpbaError::InvalidSwitchId(99);
    assert_eq!(format!("{}", err), "Invalid switch ID: 99");
}

#[test]
fn test_error_display_switch_not_writable() {
    let err = PpbaError::SwitchNotWritable(10);
    assert_eq!(format!("{}", err), "Switch not writable: 10");
}

#[test]
fn test_error_display_auto_dew_enabled() {
    let err = PpbaError::AutoDewEnabled(2);
    assert_eq!(
        format!("{}", err),
        "Cannot write to switch 2 while auto-dew is enabled. Disable auto-dew (switch 5) first."
    );
}

#[test]
fn test_error_display_invalid_value() {
    let err = PpbaError::InvalidValue("out of range".to_string());
    assert_eq!(format!("{}", err), "Invalid value: out of range");
}

#[test]
fn test_error_display_communication() {
    let err = PpbaError::Communication("connection reset".to_string());
    assert_eq!(
        format!("{}", err),
        "Device communication error: connection reset"
    );
}

#[test]
fn test_error_from_io_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let ppba_err: PpbaError = io_err.into();

    match ppba_err {
        PpbaError::Io(_) => {} // Expected
        other => panic!("Expected Io variant, got {:?}", other),
    }
}

#[test]
fn test_error_debug_formatting() {
    let err = PpbaError::NotConnected;
    let debug_str = format!("{:?}", err);
    assert!(debug_str.contains("NotConnected"));

    let err = PpbaError::InvalidSwitchId(5);
    let debug_str = format!("{:?}", err);
    assert!(debug_str.contains("InvalidSwitchId"));
    assert!(debug_str.contains("5"));
}
