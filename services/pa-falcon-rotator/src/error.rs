//! Error types for the pa-falcon-rotator driver

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};
use rusty_photon_shared_transport::TransportError;

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

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
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

/// `?` and `Into::into` sugar for the same conversion as
/// [`FalconRotatorError::to_ascom_error`]. The method is kept as the
/// explicit form; this impl lets idiomatic Rust call sites convert
/// implicitly.
impl From<FalconRotatorError> for ASCOMError {
    fn from(err: FalconRotatorError) -> Self {
        err.to_ascom_error()
    }
}

/// Direct conversion from a shared-transport [`TransportError`].
///
/// Lets the device layer write `.map_err(FalconRotatorError::from)?` on a
/// `Session::close()` (which returns `Result<_, TransportError>`)
/// instead of synthetically wrapping the `TransportError` in a
/// `SessionError::Transport(...)` just to reuse the existing
/// `From<SessionError<…>>` mapping. The codec-layer
/// `From<SessionError<FalconCodecError>> for FalconRotatorError` impl
/// also routes its transport arms through this conversion so a
/// connect-time timeout surfaced through the handshake hook gets the
/// same classification as a steady-state timeout.
impl From<TransportError> for FalconRotatorError {
    fn from(t: TransportError) -> Self {
        match t {
            TransportError::Open(e) => FalconRotatorError::ConnectionFailed(format!("open: {e}")),
            TransportError::Io(e) => FalconRotatorError::Io(e),
            TransportError::Timeout(d) => {
                FalconRotatorError::Timeout(format!("transport timeout after {d:?}"))
            }
            TransportError::Eof => {
                FalconRotatorError::Communication("Connection closed".to_string())
            }
            TransportError::Framing(s) => {
                FalconRotatorError::Communication(format!("framing: {s}"))
            }
            TransportError::Reconnecting => {
                FalconRotatorError::Communication("transport is reconnecting".to_string())
            }
        }
    }
}

/// Result type alias for pa-falcon-rotator operations
pub type Result<T> = std::result::Result<T, FalconRotatorError>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
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

    #[test]
    fn from_falcon_rotator_error_forwards_to_to_ascom_error() {
        // `?` and `Into::into` both route through this `From` impl. The
        // device-layer call sites use `?` after a `.map_err(FalconRotatorError
        // ::from)`, but those paths fire only on the error branch — which
        // none of the happy-path BDD scenarios or manager tests reach. An
        // explicit `.into()` test keeps the impl covered unconditionally.
        let ascom: ASCOMError = FalconRotatorError::NotConnected.into();
        assert_eq!(ascom.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    // ============================================================================
    // From<TransportError> for FalconRotatorError — the device-layer disconnect
    // path relies on this to map Session::close()'s TransportError directly
    // without synthesizing a SessionError. Test each TransportError variant
    // routes to its expected FalconRotatorError arm.
    // ============================================================================

    #[test]
    fn from_transport_error_open_maps_to_connection_failed() {
        let err: FalconRotatorError = TransportError::Open(std::io::Error::other("busy")).into();
        assert!(matches!(err, FalconRotatorError::ConnectionFailed(s) if s.contains("busy")));
    }

    #[test]
    fn from_transport_error_io_preserves_io_kind() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken");
        let err: FalconRotatorError = TransportError::Io(io_err).into();
        match err {
            FalconRotatorError::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn from_transport_error_timeout_maps_to_timeout() {
        let err: FalconRotatorError =
            TransportError::Timeout(std::time::Duration::from_secs(2)).into();
        assert!(matches!(err, FalconRotatorError::Timeout(s) if s.contains('2')));
    }

    #[test]
    fn from_transport_error_eof_maps_to_communication() {
        let err: FalconRotatorError = TransportError::Eof.into();
        assert!(
            matches!(err, FalconRotatorError::Communication(s) if s.contains("Connection closed"))
        );
    }

    #[test]
    fn from_transport_error_framing_maps_to_communication() {
        let err: FalconRotatorError = TransportError::Framing("too big".to_string()).into();
        assert!(matches!(err, FalconRotatorError::Communication(s) if s.contains("too big")));
    }
}
