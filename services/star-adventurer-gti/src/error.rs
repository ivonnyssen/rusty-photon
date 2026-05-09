//! Error types for the star-adventurer-gti driver.

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};

/// Errors that can arise inside the driver.
#[derive(Debug, thiserror::Error)]
pub enum StarAdvError {
    #[error("not connected to mount")]
    NotConnected,

    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("protocol error: {0}")]
    Protocol(#[from] skywatcher_motor_protocol::ProtocolError),

    #[error("invalid value: {0}")]
    InvalidValue(String),

    #[error("invalid operation: {0}")]
    InvalidOperation(String),

    #[error("parked")]
    Parked,

    #[error("config error: {0}")]
    Config(String),
}

impl StarAdvError {
    /// Map a driver error to the closest ASCOM error code.
    pub fn to_ascom_error(self) -> ASCOMError {
        match self {
            Self::NotConnected => ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, self.to_string()),
            Self::InvalidValue(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, self.to_string())
            }
            Self::Parked => ASCOMError::new(ASCOMErrorCode::INVALID_WHILE_PARKED, self.to_string()),
            _ => ASCOMError::invalid_operation(self.to_string()),
        }
    }
}

/// Driver result alias.
pub type Result<T> = std::result::Result<T, StarAdvError>;
