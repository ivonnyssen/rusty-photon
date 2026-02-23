//! Error types for the sentinel service

/// Errors that can occur in the sentinel service
#[derive(Debug, thiserror::Error)]
pub enum SentinelError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("HTTP request failed: {0}")]
    Http(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Monitor error: {0}")]
    Monitor(String),

    #[error("Notifier error: {0}")]
    Notifier(String),

    #[error("Dashboard error: {0}")]
    Dashboard(String),
}

/// Result type alias for sentinel operations
pub type Result<T> = std::result::Result<T, SentinelError>;
