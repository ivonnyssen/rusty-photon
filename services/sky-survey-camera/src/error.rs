use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkySurveyCameraError {
    #[error("config I/O: {0}")]
    ConfigIo(#[from] std::io::Error),
    #[error("config parse: {0}")]
    ConfigParse(#[from] serde_json::Error),
    #[error("server: {0}")]
    Server(String),
}
