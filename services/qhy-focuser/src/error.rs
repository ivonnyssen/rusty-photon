//! Error types for the QHY Q-Focuser driver

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};
use rusty_photon_shared_transport::TransportError;

/// Errors that can occur when interacting with the QHY Q-Focuser
#[derive(Debug, thiserror::Error)]
pub enum QhyFocuserError {
    #[error("Not connected to QHY Q-Focuser")]
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

    #[error("Move failed: {0}")]
    MoveFailed(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl QhyFocuserError {
    /// Convert this error to an ASCOM error
    pub fn to_ascom_error(self) -> ASCOMError {
        match self {
            QhyFocuserError::NotConnected => {
                ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, self.to_string())
            }
            QhyFocuserError::InvalidValue(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, self.to_string())
            }
            _ => ASCOMError::invalid_operation(self.to_string()),
        }
    }
}

/// `?` and `Into::into` sugar for the same conversion as
/// [`QhyFocuserError::to_ascom_error`]. The method is kept as the
/// explicit form; this impl lets idiomatic Rust call sites convert
/// implicitly.
impl From<QhyFocuserError> for ASCOMError {
    fn from(err: QhyFocuserError) -> Self {
        err.to_ascom_error()
    }
}

/// Direct conversion from a shared-transport [`TransportError`].
///
/// Lets the device layer write `.map_err(QhyFocuserError::from)?` on a
/// `Session::close()` (which returns `Result<_, TransportError>`)
/// instead of synthetically wrapping the `TransportError` in a
/// `SessionError::Transport(...)` just to reuse the existing
/// `From<SessionError<…>>` mapping. The codec-layer
/// `From<SessionError<QhyCodecError>> for QhyFocuserError` impl also
/// routes its transport arms through this conversion.
impl From<TransportError> for QhyFocuserError {
    fn from(t: TransportError) -> Self {
        match t {
            TransportError::Open(e) => QhyFocuserError::ConnectionFailed(e.to_string()),
            TransportError::Io(e) => QhyFocuserError::Io(e),
            TransportError::Timeout(d) => {
                QhyFocuserError::Timeout(format!("transport timeout after {d:?}"))
            }
            TransportError::Eof => QhyFocuserError::Communication("Connection closed".to_string()),
            TransportError::Framing(s) => QhyFocuserError::Communication(format!("framing: {s}")),
            TransportError::Reconnecting => {
                QhyFocuserError::Communication("transport is reconnecting".to_string())
            }
        }
    }
}

/// Result type alias for QHY Q-Focuser operations
pub type Result<T> = std::result::Result<T, QhyFocuserError>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

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

    #[test]
    fn from_qhy_focuser_error_forwards_to_to_ascom_error() {
        // `?` and `Into::into` both route through this `From` impl. The
        // device-layer call sites use `?` after a `.map_err(QhyFocuserError
        // ::from)`, but those paths fire only on the error branch — which
        // none of the happy-path BDD scenarios or manager tests reach. An
        // explicit `.into()` test keeps the impl covered unconditionally.
        let ascom: ASCOMError = QhyFocuserError::NotConnected.into();
        assert_eq!(ascom.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    // ============================================================================
    // From<TransportError> for QhyFocuserError — the device-layer disconnect
    // path relies on this to map Session::close()'s TransportError directly
    // without synthesizing a SessionError. Test each TransportError variant
    // routes to its expected QhyFocuserError arm.
    // ============================================================================

    #[test]
    fn from_transport_error_open_maps_to_connection_failed() {
        let err: QhyFocuserError = TransportError::Open(std::io::Error::other("busy")).into();
        assert!(matches!(err, QhyFocuserError::ConnectionFailed(s) if s.contains("busy")));
    }

    #[test]
    fn from_transport_error_io_preserves_io_kind() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken");
        let err: QhyFocuserError = TransportError::Io(io_err).into();
        match err {
            QhyFocuserError::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn from_transport_error_timeout_maps_to_timeout() {
        let err: QhyFocuserError =
            TransportError::Timeout(std::time::Duration::from_secs(2)).into();
        assert!(matches!(err, QhyFocuserError::Timeout(s) if s.contains('2')));
    }

    #[test]
    fn from_transport_error_eof_maps_to_communication() {
        let err: QhyFocuserError = TransportError::Eof.into();
        assert!(
            matches!(err, QhyFocuserError::Communication(s) if s.contains("Connection closed"))
        );
    }

    #[test]
    fn from_transport_error_framing_maps_to_communication() {
        let err: QhyFocuserError = TransportError::Framing("too big".to_string()).into();
        assert!(matches!(err, QhyFocuserError::Communication(s) if s.contains("too big")));
    }
}
