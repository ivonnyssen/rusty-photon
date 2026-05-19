//! Error types for the pa-falcon-rotator driver

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};

/// Errors that can occur when interacting with the Pegasus Falcon Rotator
#[derive(Debug, thiserror::Error)]
pub enum FalconRotatorError {
    #[error("Not connected to Falcon Rotator")]
    NotConnected,

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Serial port error: {0}")]
    SerialPort(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Invalid value: {0}")]
    InvalidValue(String),

    #[error("Device communication error: {0}")]
    Communication(String),
}

impl FalconRotatorError {
    /// Convert this driver error into the ASCOM error code the design doc pins
    /// in the [Error Model](../../../docs/services/falcon-rotator.md#error-model).
    pub fn to_ascom_error(self) -> ASCOMError {
        match self {
            FalconRotatorError::NotConnected => {
                ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, self.to_string())
            }
            FalconRotatorError::InvalidValue(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, self.to_string())
            }
            _ => ASCOMError::invalid_operation(self.to_string()),
        }
    }
}

/// Result type alias for pa-falcon-rotator operations
pub type Result<T> = std::result::Result<T, FalconRotatorError>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_not_connected() {
        let err = FalconRotatorError::NotConnected;
        assert_eq!(format!("{}", err), "Not connected to Falcon Rotator");
    }

    #[test]
    fn test_error_display_connection_failed() {
        let err = FalconRotatorError::ConnectionFailed("handshake".to_string());
        assert_eq!(format!("{}", err), "Connection failed: handshake");
    }

    #[test]
    fn test_error_display_serial_port() {
        let err = FalconRotatorError::SerialPort("no such device".to_string());
        assert_eq!(format!("{}", err), "Serial port error: no such device");
    }

    #[test]
    fn test_error_display_timeout() {
        let err = FalconRotatorError::Timeout("read".to_string());
        assert_eq!(format!("{}", err), "Timeout: read");
    }

    #[test]
    fn test_error_display_io_via_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken");
        let err: FalconRotatorError = io_err.into();
        assert_eq!(format!("{}", err), "IO error: broken");
    }

    #[test]
    fn test_error_display_invalid_response() {
        let err = FalconRotatorError::InvalidResponse("bad echo".to_string());
        assert_eq!(format!("{}", err), "Invalid response: bad echo");
    }

    #[test]
    fn test_error_display_parse_error() {
        let err = FalconRotatorError::ParseError("not a float".to_string());
        assert_eq!(format!("{}", err), "Parse error: not a float");
    }

    #[test]
    fn test_error_display_invalid_value() {
        let err = FalconRotatorError::InvalidValue("nan".to_string());
        assert_eq!(format!("{}", err), "Invalid value: nan");
    }

    #[test]
    fn test_error_display_communication() {
        let err = FalconRotatorError::Communication("port closed".to_string());
        assert_eq!(
            format!("{}", err),
            "Device communication error: port closed"
        );
    }

    #[test]
    fn test_error_debug_formatting() {
        let err = FalconRotatorError::NotConnected;
        assert!(format!("{:?}", err).contains("NotConnected"));

        let err = FalconRotatorError::InvalidValue("nan".to_string());
        assert!(format!("{:?}", err).contains("InvalidValue"));
    }

    #[test]
    fn test_to_ascom_error_not_connected() {
        let err = FalconRotatorError::NotConnected;
        let ascom_err = err.to_ascom_error();
        assert_eq!(ascom_err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[test]
    fn test_to_ascom_error_invalid_value() {
        let err = FalconRotatorError::InvalidValue("non-finite".to_string());
        let ascom_err = err.to_ascom_error();
        assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[test]
    fn test_to_ascom_error_connection_failed_collapses_to_invalid_operation() {
        let err = FalconRotatorError::ConnectionFailed("handshake".to_string());
        let ascom_err = err.to_ascom_error();
        assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn test_to_ascom_error_serial_port_collapses_to_invalid_operation() {
        let err = FalconRotatorError::SerialPort("open".to_string());
        let ascom_err = err.to_ascom_error();
        assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn test_to_ascom_error_timeout_collapses_to_invalid_operation() {
        let err = FalconRotatorError::Timeout("read".to_string());
        let ascom_err = err.to_ascom_error();
        assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn test_to_ascom_error_io_collapses_to_invalid_operation() {
        let err = FalconRotatorError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "eof",
        ));
        let ascom_err = err.to_ascom_error();
        assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn test_to_ascom_error_invalid_response_collapses_to_invalid_operation() {
        let err = FalconRotatorError::InvalidResponse("echo".to_string());
        let ascom_err = err.to_ascom_error();
        assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn test_to_ascom_error_parse_error_collapses_to_invalid_operation() {
        let err = FalconRotatorError::ParseError("bad".to_string());
        let ascom_err = err.to_ascom_error();
        assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn test_to_ascom_error_communication_collapses_to_invalid_operation() {
        let err = FalconRotatorError::Communication("dropped".to_string());
        let ascom_err = err.to_ascom_error();
        assert_eq!(ascom_err.code, ASCOMErrorCode::INVALID_OPERATION);
    }
}
