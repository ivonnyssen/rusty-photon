//! Error types for the PPBA Switch driver

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};

/// Errors that can occur when interacting with the PPBA device
#[derive(Debug, thiserror::Error)]
pub enum PpbaError {
    #[error("Not connected to PPBA")]
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

    #[error("Invalid switch ID: {0}")]
    InvalidSwitchId(usize),

    #[error("Switch not writable: {0}")]
    SwitchNotWritable(usize),

    #[error(
        "Cannot write to switch {0} while auto-dew is enabled. Disable auto-dew (switch 5) first."
    )]
    AutoDewEnabled(usize),

    #[error("Invalid value: {0}")]
    InvalidValue(String),

    #[error("Device communication error: {0}")]
    Communication(String),
}

impl PpbaError {
    /// Map this error to the matching ASCOM error code + message.
    ///
    /// Centralised here so both `PpbaSwitchDevice` and
    /// `PpbaObservingConditionsDevice` (and any future device on this
    /// transport) get identical classification. The variants are the
    /// union of what either device can emit; the switch-specific arms
    /// (`InvalidSwitchId`, `SwitchNotWritable`, `AutoDewEnabled`) are
    /// no-ops for the OC device's call paths but live here so the
    /// mapping is closed.
    pub fn to_ascom_error(self) -> ASCOMError {
        match self {
            PpbaError::NotConnected => {
                ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, self.to_string())
            }
            PpbaError::InvalidSwitchId(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, self.to_string())
            }
            PpbaError::SwitchNotWritable(_) => {
                ASCOMError::new(ASCOMErrorCode::NOT_IMPLEMENTED, self.to_string())
            }
            PpbaError::AutoDewEnabled(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_OPERATION, self.to_string())
            }
            PpbaError::InvalidValue(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, self.to_string())
            }
            _ => ASCOMError::invalid_operation(self.to_string()),
        }
    }
}

/// `?` and `Into::into` sugar for the same conversion as
/// [`PpbaError::to_ascom_error`]. The method is kept as the explicit
/// form; this impl lets idiomatic Rust call sites convert implicitly.
impl From<PpbaError> for ASCOMError {
    fn from(err: PpbaError) -> Self {
        err.to_ascom_error()
    }
}

/// Result type alias for PPBA operations
pub type Result<T> = std::result::Result<T, PpbaError>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

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

    // ============================================================================
    // to_ascom_error / From<PpbaError> for ASCOMError: the canonical mapping
    // both ASCOM devices share. The two impls are kept in lockstep — From
    // forwards to to_ascom_error — so tests below exercise the method form
    // and the From impl picks up the same coverage transitively.
    // ============================================================================

    #[test]
    fn to_ascom_error_not_connected_maps_to_not_connected() {
        let err = PpbaError::NotConnected.to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[test]
    fn to_ascom_error_invalid_switch_id_maps_to_invalid_value() {
        let err = PpbaError::InvalidSwitchId(99).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[test]
    fn to_ascom_error_switch_not_writable_maps_to_not_implemented() {
        let err = PpbaError::SwitchNotWritable(10).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);
    }

    #[test]
    fn to_ascom_error_auto_dew_enabled_maps_to_invalid_operation() {
        let err = PpbaError::AutoDewEnabled(3).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn to_ascom_error_invalid_value_maps_to_invalid_value() {
        let err = PpbaError::InvalidValue("oob".to_string()).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[test]
    fn to_ascom_error_communication_falls_to_invalid_operation() {
        let err = PpbaError::Communication("boom".to_string()).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn to_ascom_error_connection_failed_falls_to_invalid_operation() {
        let err = PpbaError::ConnectionFailed("nope".to_string()).to_ascom_error();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    #[test]
    fn from_ppba_error_forwards_to_to_ascom_error() {
        // ? and Into::into both route through this From impl.
        let ascom: ASCOMError = PpbaError::NotConnected.into();
        assert_eq!(ascom.code, ASCOMErrorCode::NOT_CONNECTED);
    }
}
