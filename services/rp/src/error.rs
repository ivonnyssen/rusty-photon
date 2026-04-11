use thiserror::Error;

pub type Result<T> = std::result::Result<T, RpError>;

#[derive(Debug, Error)]
pub enum RpError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("alpaca error: {0}")]
    Alpaca(String),

    #[error("equipment not found: {0}")]
    EquipmentNotFound(String),

    #[error("session error: {0}")]
    Session(String),

    #[error("server error: {0}")]
    Server(String),

    #[error("imaging error: {0}")]
    Imaging(String),
}
