use std::io;

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("certificate generation error: {0}")]
    CertGen(#[from] rcgen::Error),

    #[error("TLS configuration error: {0}")]
    Rustls(#[from] rustls::Error),

    #[error("PEM parsing error: {0}")]
    Pem(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, TlsError>;
