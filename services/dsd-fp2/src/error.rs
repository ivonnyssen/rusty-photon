//! Error types for the Deep Sky Dad FP2 driver

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};
use rusty_photon_shared_transport::{SessionError, TransportError};

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

/// `?` and `Into::into` sugar for the same conversion as
/// [`DsdFp2Error::to_ascom_error`]. Keeps the method as the explicit form
/// and lets idiomatic call sites convert implicitly.
impl From<DsdFp2Error> for ASCOMError {
    fn from(err: DsdFp2Error) -> Self {
        err.to_ascom_error()
    }
}

/// Direct conversion from a shared-transport [`TransportError`].
///
/// Lets the device layer write `.map_err(DsdFp2Error::from)?` on a
/// `Session::close()` (which returns `Result<_, TransportError>`)
/// instead of synthetically wrapping the `TransportError` in a
/// `SessionError::Transport(...)` just to reuse the existing
/// `From<SessionError<…>>` mapping. The `From<SessionError<DsdFp2Error>>`
/// impl below also routes its transport arms through this conversion so
/// a transport-level failure surfaced through the handshake hook gets
/// the same classification as one that surfaces on a steady-state
/// request.
impl From<TransportError> for DsdFp2Error {
    fn from(t: TransportError) -> Self {
        match t {
            TransportError::Open(io) => DsdFp2Error::SerialPort(io.to_string()),
            TransportError::Io(io) => DsdFp2Error::Io(io),
            TransportError::Timeout(d) => DsdFp2Error::Timeout(format!("{d:?}")),
            TransportError::Eof => DsdFp2Error::Communication("transport reached EOF".to_string()),
            TransportError::Framing(msg) => DsdFp2Error::Communication(format!("framing: {msg}")),
        }
    }
}

/// Flatten a [`SessionError<DsdFp2Error>`] to a [`DsdFp2Error`].
///
/// The `Fp2Codec::Error` type is `DsdFp2Error` itself, so the `Codec(inner)`
/// arm passes the inner error through verbatim. The `Transport(t)` arm
/// routes through [`From<TransportError>`], and `SkipExhausted(n)` becomes
/// a `Communication` variant.
impl From<SessionError<DsdFp2Error>> for DsdFp2Error {
    fn from(err: SessionError<DsdFp2Error>) -> Self {
        match err {
            SessionError::Codec(inner) => inner,
            SessionError::Transport(t) => t.into(),
            SessionError::SkipExhausted(n) => {
                DsdFp2Error::Communication(format!("skip exhausted ({n} frames)"))
            }
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

    #[test]
    fn from_dsd_fp2_error_forwards_to_to_ascom_error() {
        // `?` and `Into::into` both route through this `From` impl. The
        // device-layer call sites use `?` after `.map_err(DsdFp2Error::from)`,
        // but those paths fire only on the error branch — none of the
        // happy-path tests reach them. An explicit `.into()` test keeps the
        // impl covered unconditionally.
        let ascom: ASCOMError = DsdFp2Error::NotConnected.into();
        assert_eq!(ascom.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    // ============================================================================
    // From<TransportError> for DsdFp2Error — the device-layer disconnect
    // path relies on this to map Session::close()'s TransportError directly
    // without synthesising a SessionError. Test each TransportError variant
    // routes to its expected DsdFp2Error arm.
    // ============================================================================

    #[test]
    fn from_transport_error_open_maps_to_serial_port() {
        let err: DsdFp2Error = TransportError::Open(std::io::Error::other("busy")).into();
        assert!(matches!(err, DsdFp2Error::SerialPort(s) if s.contains("busy")));
    }

    #[test]
    fn from_transport_error_io_preserves_io_kind() {
        let io = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken");
        let err: DsdFp2Error = TransportError::Io(io).into();
        match err {
            DsdFp2Error::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn from_transport_error_timeout_maps_to_timeout() {
        let err: DsdFp2Error = TransportError::Timeout(std::time::Duration::from_secs(2)).into();
        assert!(matches!(err, DsdFp2Error::Timeout(s) if s.contains('2')));
    }

    #[test]
    fn from_transport_error_eof_maps_to_communication() {
        let err: DsdFp2Error = TransportError::Eof.into();
        assert!(matches!(err, DsdFp2Error::Communication(s) if s.contains("EOF")));
    }

    #[test]
    fn from_transport_error_framing_maps_to_communication() {
        let err: DsdFp2Error = TransportError::Framing("too big".to_string()).into();
        assert!(matches!(err, DsdFp2Error::Communication(s) if s.contains("too big")));
    }

    // ============================================================================
    // From<SessionError<DsdFp2Error>> for DsdFp2Error — flattening helper
    // used at every `.request().await.map_err(DsdFp2Error::from)?` site. The
    // Codec arm just unwraps; the Transport arm routes through
    // `From<TransportError>`; SkipExhausted becomes a Communication error.
    // ============================================================================

    #[test]
    fn from_session_error_codec_passes_through() {
        let wrapped =
            SessionError::<DsdFp2Error>::Codec(DsdFp2Error::MalformedResponse("x".to_string()));
        let flat: DsdFp2Error = wrapped.into();
        assert!(matches!(flat, DsdFp2Error::MalformedResponse(_)));
    }

    #[test]
    fn from_session_error_transport_timeout_becomes_timeout() {
        let wrapped = SessionError::<DsdFp2Error>::Transport(TransportError::Timeout(
            std::time::Duration::from_secs(3),
        ));
        let flat: DsdFp2Error = wrapped.into();
        assert!(matches!(flat, DsdFp2Error::Timeout(_)));
    }

    #[test]
    fn from_session_error_transport_eof_becomes_communication() {
        let wrapped = SessionError::<DsdFp2Error>::Transport(TransportError::Eof);
        let flat: DsdFp2Error = wrapped.into();
        assert!(matches!(flat, DsdFp2Error::Communication(_)));
    }

    #[test]
    fn from_session_error_skip_exhausted_becomes_communication() {
        let wrapped = SessionError::<DsdFp2Error>::SkipExhausted(2);
        let flat: DsdFp2Error = wrapped.into();
        assert!(matches!(flat, DsdFp2Error::Communication(s) if s.contains('2')));
    }
}
