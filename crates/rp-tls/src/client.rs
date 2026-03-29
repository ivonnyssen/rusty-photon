use std::path::Path;

use tracing::debug;

use crate::error::{Result, TlsError};

/// Build a `reqwest::Client` with optional CA certificate trust.
///
/// When `ca_cert_path` is `Some`, the PEM-encoded CA certificate at that path
/// is added as a trusted root. This allows the client to connect to services
/// using certificates signed by the Rusty Photon CA.
///
/// When `ca_cert_path` is `None`, returns a default client.
pub fn build_reqwest_client(ca_cert_path: Option<&Path>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();

    if let Some(ca_path) = ca_cert_path {
        debug!("Loading CA certificate from {}", ca_path.display());
        let ca_pem = std::fs::read(ca_path)?;
        let ca_cert = reqwest::Certificate::from_pem(&ca_pem)
            .map_err(|e| TlsError::Pem(format!("failed to parse CA certificate: {e}")))?;
        // Use tls_certs_only to disable built-in/platform root certs.
        // Without this, the platform verifier (e.g. macOS Security framework)
        // rejects our self-signed CA because it is not in the system keychain.
        builder = builder.tls_certs_only([ca_cert]);
    }

    let client = builder
        .build()
        .map_err(|e| TlsError::Other(format!("failed to build reqwest client: {e}")))?;

    Ok(client)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn build_client_without_ca() {
        let client = build_reqwest_client(None).unwrap();
        // Just verify we get a valid client
        drop(client);
    }

    #[test]
    fn build_client_with_valid_ca() {
        // Generate a CA cert first
        let dir = tempfile::tempdir().unwrap();
        crate::cert::generate_ca(dir.path()).unwrap();

        let ca_path = dir.path().join("ca.pem");
        let client = build_reqwest_client(Some(&ca_path)).unwrap();
        drop(client);
    }

    #[test]
    fn build_client_with_missing_ca_returns_error() {
        let result = build_reqwest_client(Some(Path::new("/nonexistent/ca.pem")));
        assert!(result.is_err());
    }
}
