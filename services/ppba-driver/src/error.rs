//! Error types for the PPBA Switch driver

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

/// Result type alias for PPBA operations
pub type Result<T> = std::result::Result<T, PpbaError>;
