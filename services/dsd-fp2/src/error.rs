//! Error types for the Deep Sky Dad FP2 driver

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};

/// Errors that can occur when interacting with the Deep Sky Dad FP2.
#[derive(Debug, thiserror::Error)]
pub enum DsdFp2Error {
    #[error("Not connected to Deep Sky Dad FP2")]
    NotConnected,

    #[error("Serial port error: {0}")]
    SerialPort(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Malformed response: {0}")]
    MalformedResponse(String),

    #[error("Invalid value: {0}")]
    InvalidValue(String),

    #[error("Device communication error: {0}")]
    Communication(String),

    #[error("Handshake failed: {0}")]
    HandshakeFailed(String),
}

impl DsdFp2Error {
    /// Convert this error to an ASCOM error
    pub fn to_ascom_error(self) -> ASCOMError {
        match self {
            DsdFp2Error::NotConnected | DsdFp2Error::HandshakeFailed(_) => {
                ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, self.to_string())
            }
            DsdFp2Error::InvalidValue(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, self.to_string())
            }
            _ => ASCOMError::invalid_operation(self.to_string()),
        }
    }
}

/// Result type alias for FP2 driver operations.
pub type Result<T> = std::result::Result<T, DsdFp2Error>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn display_not_connected() {
        let err = DsdFp2Error::NotConnected;
        assert_eq!(format!("{}", err), "Not connected to Deep Sky Dad FP2");
    }

    #[test]
    fn display_serial_port() {
        let err = DsdFp2Error::SerialPort("no such device".to_string());
        assert_eq!(format!("{}", err), "Serial port error: no such device");
    }

    #[test]
    fn display_timeout() {
        let err = DsdFp2Error::Timeout("read timed out".to_string());
        assert_eq!(format!("{}", err), "Timeout: read timed out");
    }

    #[test]
    fn display_io() {
        let io = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe");
        let err = DsdFp2Error::Io(io);
        assert_eq!(format!("{}", err), "IO error: broken pipe");
    }

    #[test]
    fn display_malformed_response() {
        let err = DsdFp2Error::MalformedResponse("()".to_string());
        assert_eq!(format!("{}", err), "Malformed response: ()");
    }

    #[test]
    fn display_invalid_value() {
        let err = DsdFp2Error::InvalidValue("brightness 9000".to_string());
        assert_eq!(format!("{}", err), "Invalid value: brightness 9000");
    }

    #[test]
    fn display_communication() {
        let err = DsdFp2Error::Communication("link down".to_string());
        assert_eq!(format!("{}", err), "Device communication error: link down");
    }

    #[test]
    fn display_handshake_failed() {
        let err = DsdFp2Error::HandshakeFailed("wrong board id".to_string());
        assert_eq!(format!("{}", err), "Handshake failed: wrong board id");
    }

    #[test]
    fn from_io_error() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: DsdFp2Error = io.into();
        assert!(matches!(err, DsdFp2Error::Io(_)));
    }

    #[test]
    fn ascom_not_connected() {
        let err = DsdFp2Error::NotConnected.to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[test]
    fn ascom_handshake_maps_to_not_connected() {
        let err = DsdFp2Error::HandshakeFailed("nope".to_string()).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[test]
    fn ascom_invalid_value() {
        let err = DsdFp2Error::InvalidValue("range".to_string()).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[test]
    fn ascom_communication_maps_to_invalid_operation() {
        let err = DsdFp2Error::Communication("garbled".to_string()).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn ascom_malformed_response_maps_to_invalid_operation() {
        let err = DsdFp2Error::MalformedResponse("()".to_string()).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn ascom_timeout_maps_to_invalid_operation() {
        let err = DsdFp2Error::Timeout("3s".to_string()).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }
}
