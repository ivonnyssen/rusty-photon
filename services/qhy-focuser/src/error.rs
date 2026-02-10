//! Error types for the QHY Q-Focuser driver

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};

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

/// Result type alias for QHY Q-Focuser operations
pub type Result<T> = std::result::Result<T, QhyFocuserError>;
