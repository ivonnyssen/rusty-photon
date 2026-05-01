use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FitsError {
    #[error("FITS I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("invalid keyword: {0}")]
    InvalidKeyword(String),

    #[error("dimension mismatch: pixel count {got} does not match {width}x{height} (expected {expected})")]
    DimensionMismatch {
        got: usize,
        width: u32,
        height: u32,
        expected: usize,
    },

    #[error("malformed FITS header: {0}")]
    MalformedHeader(String),

    #[error("unsupported FITS feature: {0}")]
    Unsupported(String),

    #[error("FITS parse error: {0}")]
    Parse(String),

    #[error("missing required FITS keyword: {0}")]
    MissingKeyword(&'static str),

    #[error("FITS keyword has wrong type: {key} (expected {expected})")]
    KeywordTypeMismatch { key: String, expected: &'static str },
}
