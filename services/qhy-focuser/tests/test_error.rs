//! Tests for the QhyFocuserError type

use ascom_alpaca::ASCOMErrorCode;
use qhy_focuser::error::QhyFocuserError;

#[test]
fn test_error_display_not_connected() {
    let err = QhyFocuserError::NotConnected;
    assert_eq!(format!("{}", err), "Not connected to QHY Q-Focuser");
}

#[test]
fn test_error_display_connection_failed() {
    let err = QhyFocuserError::ConnectionFailed("port busy".to_string());
    assert_eq!(format!("{}", err), "Connection failed: port busy");
}

#[test]
fn test_error_display_serial_port() {
    let err = QhyFocuserError::SerialPort("no such device".to_string());
    assert_eq!(format!("{}", err), "Serial port error: no such device");
}

#[test]
fn test_error_display_timeout() {
    let err = QhyFocuserError::Timeout("read timed out".to_string());
    assert_eq!(format!("{}", err), "Timeout: read timed out");
}

#[test]
fn test_error_display_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe");
    let err = QhyFocuserError::Io(io_err);
    assert_eq!(format!("{}", err), "IO error: broken pipe");
}

#[test]
fn test_error_display_invalid_response() {
    let err = QhyFocuserError::InvalidResponse("bad format".to_string());
    assert_eq!(format!("{}", err), "Invalid response: bad format");
}

#[test]
fn test_error_display_parse_error() {
    let err = QhyFocuserError::ParseError("not a number".to_string());
    assert_eq!(format!("{}", err), "Parse error: not a number");
}

#[test]
fn test_error_display_invalid_value() {
    let err = QhyFocuserError::InvalidValue("out of range".to_string());
    assert_eq!(format!("{}", err), "Invalid value: out of range");
}

#[test]
fn test_error_display_communication() {
    let err = QhyFocuserError::Communication("connection reset".to_string());
    assert_eq!(
        format!("{}", err),
        "Device communication error: connection reset"
    );
}

#[test]
fn test_error_display_move_failed() {
    let err = QhyFocuserError::MoveFailed("stalled".to_string());
    assert_eq!(format!("{}", err), "Move failed: stalled");
}

#[test]
fn test_error_from_io_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let focuser_err: QhyFocuserError = io_err.into();

    match focuser_err {
        QhyFocuserError::Io(_) => {}
        other => panic!("Expected Io variant, got {:?}", other),
    }
}

#[test]
fn test_error_debug_formatting() {
    let err = QhyFocuserError::NotConnected;
    let debug_str = format!("{:?}", err);
    assert!(debug_str.contains("NotConnected"));

    let err = QhyFocuserError::InvalidValue("test".to_string());
    let debug_str = format!("{:?}", err);
    assert!(debug_str.contains("InvalidValue"));
}

#[test]
fn test_to_ascom_error_not_connected() {
    let err = QhyFocuserError::NotConnected;
    let ascom_err = err.to_ascom_error();
    assert_eq!(ascom_err.code, ASCOMErrorCode::NOT_CONNECTED);
}

#[test]
fn test_to_ascom_error_invalid_value() {
    let err = QhyFocuserError::InvalidValue("out of range".to_string());
    let ascom_err = err.to_ascom_error();
    assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_VALUE);
}

#[test]
fn test_to_ascom_error_communication() {
    let err = QhyFocuserError::Communication("timeout".to_string());
    let ascom_err = err.to_ascom_error();
    assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_OPERATION);
}

#[test]
fn test_to_ascom_error_move_failed() {
    let err = QhyFocuserError::MoveFailed("stalled".to_string());
    let ascom_err = err.to_ascom_error();
    assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_OPERATION);
}
