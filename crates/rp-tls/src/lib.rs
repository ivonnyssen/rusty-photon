#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! TLS utilities for Rusty Photon services.
//!
//! Provides certificate generation, TLS server helpers, client CA trust,
//! and shared configuration types for opt-in HTTPS across all services.

pub mod cert;
pub mod client;
pub mod config;
pub mod error;
pub mod server;
