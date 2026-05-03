use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkySurveyCameraError {
    #[error("config I/O: {0}")]
    ConfigIo(#[from] std::io::Error),
    #[error("config parse: {0}")]
    ConfigParse(#[from] serde_json::Error),
    #[error("server bind: {0}")]
    Bind(String),
    #[error("server: {0}")]
    Server(String),
    #[error("mount client: {0}")]
    MountClient(String),
}

/// Outcome of a single Telescope read in follow mode. Surfaced via the
/// camera's `last_error` and ASCOM `UNSPECIFIED_ERROR` per F2.
#[derive(Debug, Error)]
pub enum MountReadError {
    #[error("mount read timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("mount transport error: {0}")]
    Transport(String),
    #[error("ASCOM error: {0}")]
    Ascom(String),
    #[error("mount device {device_number} not found on Alpaca server")]
    DeviceNotFound { device_number: u32 },
}
