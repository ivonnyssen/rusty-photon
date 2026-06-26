//! Error type for the touptek-camera service.

use thiserror::Error;

/// Errors raised while building or running the touptek-camera server.
#[derive(Debug, Error)]
pub enum TouptekCameraError {
    /// An error surfaced by the `touptek-rs` SDK wrapper (enumeration, open, …).
    #[error(transparent)]
    Sdk(#[from] touptek_rs::Error),

    /// The on-disk configuration could not be read or parsed.
    #[error("config error: {0}")]
    Config(String),

    /// Binding the Alpaca listener failed.
    #[error("failed to bind {addr}: {source}")]
    Bind {
        /// The address we tried to bind.
        addr: String,
        /// The underlying error from the dual-stack bind helper.
        #[source]
        source: rp_tls::error::TlsError,
    },

    /// The blocking enumeration task panicked or was cancelled.
    #[error("camera enumeration task failed: {0}")]
    Join(#[from] tokio::task::JoinError),

    /// The HTTP server stopped with an error.
    #[error("server error: {0}")]
    Server(String),
}
