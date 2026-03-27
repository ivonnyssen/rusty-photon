//! TLS utilities for Rusty Photon services.
//!
//! Provides certificate generation, TLS server helpers, client CA trust,
//! and shared configuration types for opt-in HTTPS across all services.

pub mod cert;
pub mod client;
pub mod config;
pub mod error;
pub mod server;

/// Install the `ring` crypto provider for rustls.
///
/// Must be called once before any rustls operation. Safe to call multiple
/// times — returns `Ok(())` if already installed, `Err` only if a
/// *different* provider was installed first.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
