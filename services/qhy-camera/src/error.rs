//! Error types for the QHY Camera driver

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};

/// Errors that can occur when interacting with QHYCCD cameras and filter wheels
#[derive(Debug, thiserror::Error)]
pub enum QhyCameraError {
    #[error("Not connected to device")]
    NotConnected,

    #[error("SDK error: {0}")]
    SdkError(String),

    #[error("Image transform error: {0}")]
    ImageTransform(String),

    #[error("Invalid value: {0}")]
    InvalidValue(String),

    #[error("Control not available: {0}")]
    ControlNotAvailable(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl QhyCameraError {
    /// Convert this error to an ASCOM error
    pub fn to_ascom_error(self) -> ASCOMError {
        match self {
            QhyCameraError::NotConnected => {
                ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, self.to_string())
            }
            QhyCameraError::InvalidValue(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, self.to_string())
            }
            QhyCameraError::ControlNotAvailable(_) => {
                ASCOMError::new(ASCOMErrorCode::NOT_IMPLEMENTED, self.to_string())
            }
            _ => ASCOMError::invalid_operation(self.to_string()),
        }
    }
}

/// Result type alias for QHY Camera operations
pub type Result<T> = std::result::Result<T, QhyCameraError>;
