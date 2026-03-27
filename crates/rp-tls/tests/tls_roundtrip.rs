//! Integration test: verify HTTPS roundtrip with generated certs.

use std::net::SocketAddr;

use axum::{routing::get, Router};

#[tokio::test]
async fn https_roundtrip_with_generated_certs() {
    // Generate CA + service cert
    let pki_dir = tempfile::tempdir().unwrap();
    let certs_dir = pki_dir.path().join("certs");

    rp_tls::cert::generate_ca(pki_dir.path()).unwrap();

    let ca_cert_pem = std::fs::read_to_string(pki_dir.path().join("ca.pem")).unwrap();
    let ca_key_pem = std::fs::read_to_string(pki_dir.path().join("ca-key.pem")).unwrap();

    rp_tls::cert::generate_service_cert(&ca_cert_pem, &ca_key_pem, "test-service", &[], &certs_dir)
        .unwrap();

    // Build TLS config
    let tls_config = rp_tls::config::TlsConfig {
        cert: certs_dir
            .join("test-service.pem")
            .to_string_lossy()
            .into_owned(),
        key: certs_dir
            .join("test-service-key.pem")
            .to_string_lossy()
            .into_owned(),
    };

    // Start TLS server
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = rp_tls::server::bind_dual_stack_tokio(addr).await.unwrap();
    let bound_addr = listener.local_addr().unwrap();

    let router = Router::new().route("/health", get(|| async { "ok" }));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        rp_tls::server::serve_tls(listener, router, &tls_config, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    // Build client with CA trust
    let client =
        rp_tls::client::build_reqwest_client(Some(&pki_dir.path().join("ca.pem"))).unwrap();

    // Make HTTPS request
    let url = format!("https://localhost:{}/health", bound_addr.port());
    let response = client.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "ok");

    // Shutdown
    shutdown_tx.send(()).ok();
    server_handle.await.ok();
}

#[tokio::test]
async fn client_without_ca_rejects_self_signed() {
    // Generate CA + service cert
    let pki_dir = tempfile::tempdir().unwrap();
    let certs_dir = pki_dir.path().join("certs");

    rp_tls::cert::generate_ca(pki_dir.path()).unwrap();

    let ca_cert_pem = std::fs::read_to_string(pki_dir.path().join("ca.pem")).unwrap();
    let ca_key_pem = std::fs::read_to_string(pki_dir.path().join("ca-key.pem")).unwrap();

    rp_tls::cert::generate_service_cert(&ca_cert_pem, &ca_key_pem, "test-service", &[], &certs_dir)
        .unwrap();

    let tls_config = rp_tls::config::TlsConfig {
        cert: certs_dir
            .join("test-service.pem")
            .to_string_lossy()
            .into_owned(),
        key: certs_dir
            .join("test-service-key.pem")
            .to_string_lossy()
            .into_owned(),
    };

    // Start TLS server
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = rp_tls::server::bind_dual_stack_tokio(addr).await.unwrap();
    let bound_addr = listener.local_addr().unwrap();

    let router = Router::new().route("/health", get(|| async { "ok" }));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        rp_tls::server::serve_tls(listener, router, &tls_config, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    // Build client WITHOUT CA trust — should reject the self-signed cert
    let client = rp_tls::client::build_reqwest_client(None).unwrap();

    let url = format!("https://localhost:{}/health", bound_addr.port());
    let result = client.get(&url).send().await;
    assert!(result.is_err(), "should reject untrusted certificate");

    // Shutdown
    shutdown_tx.send(()).ok();
    server_handle.await.ok();
}

#[tokio::test]
async fn plain_http_roundtrip() {
    // Start plain HTTP server
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = rp_tls::server::bind_dual_stack_tokio(addr).await.unwrap();
    let bound_addr = listener.local_addr().unwrap();

    let router = Router::new().route("/health", get(|| async { "ok" }));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        rp_tls::server::serve_plain(listener, router, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    // Plain HTTP client
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/health", bound_addr.port());
    let response = client.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "ok");

    // Shutdown
    shutdown_tx.send(()).ok();
    server_handle.await.ok();
}
