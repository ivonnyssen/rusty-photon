use thiserror::Error;

pub type Result<T> = std::result::Result<T, CalibratorFlatsError>;

#[derive(Debug, Error)]
pub enum CalibratorFlatsError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("MCP tool call failed: {0}")]
    ToolCall(String),

    #[error("workflow error: {0}")]
    Workflow(String),

    #[error("server error: {0}")]
    Server(String),
}
