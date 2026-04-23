#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! TLS utilities for Rusty Photon services.
//!
//! Provides certificate generation, TLS server helpers, client CA trust,
//! and shared configuration types for opt-in HTTPS across all services.

pub mod acme;
pub mod acme_config;
pub mod cert;
pub mod client;
pub mod config;
pub mod dns;
pub mod error;
pub mod permissions;
pub mod server;

/// Install `aws-lc-rs` as the process-wide default rustls `CryptoProvider`.
///
/// Required because both `aws-lc-rs` and `ring` end up feature-activated on
/// `rustls` via our transitive deps (reqwest 0.13 + reqwest 0.12 via cloudflare
/// rustls-tls), which defeats rustls's automatic provider selection.
/// Idempotent: subsequent calls after the first one are no-ops.
pub fn install_default_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}
