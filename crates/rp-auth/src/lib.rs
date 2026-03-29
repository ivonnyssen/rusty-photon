#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! HTTP Basic Auth utilities for Rusty Photon services.
//!
//! Provides Argon2id credential hashing/verification, axum tower middleware,
//! and shared configuration types for opt-in authentication across all services.

pub mod config;
pub mod credentials;
pub mod error;
pub mod middleware;

use axum::Router;
use config::AuthConfig;

/// Wrap a router with HTTP Basic Auth middleware.
///
/// All requests must include a valid `Authorization: Basic` header.
/// Requests with missing or invalid credentials receive `401 Unauthorized`
/// with a `WWW-Authenticate: Basic realm="Rusty Photon"` header.
pub fn layer(router: Router, config: &AuthConfig) -> Router {
    middleware::apply(router, config)
}
