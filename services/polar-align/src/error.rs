use thiserror::Error;

pub type Result<T> = std::result::Result<T, PolarAlignError>;

#[derive(Debug, Error)]
pub enum PolarAlignError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("MCP tool call failed: {0}")]
    ToolCall(String),

    #[error("geometry error: {0}")]
    Geometry(String),

    #[error("ephemeris error: {0}")]
    Ephemeris(String),

    #[error("workflow error: {0}")]
    Workflow(String),

    #[error("server error: {0}")]
    Server(String),
}
