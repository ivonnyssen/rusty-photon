#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid Authorization header: {0}")]
    InvalidHeader(String),

    #[error("invalid credentials")]
    InvalidCredentials,

    #[error("password hashing error: {0}")]
    HashingError(String),
}

pub type Result<T> = std::result::Result<T, AuthError>;
