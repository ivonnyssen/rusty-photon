//! Integration test: verify HTTPS roundtrip with generated certs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

use std::net::SocketAddr;

use axum::{routing::get, Router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn https_roundtrip_with_generated_certs() {
    // Generate CA + service cert
    let pki_dir = tempfile::tempdir().unwrap();
    let certs_dir = pki_dir.path().join("certs");

    rusty_photon_tls::test_cert::generate_ca(pki_dir.path()).unwrap();

    let ca_cert_pem = std::fs::read_to_string(pki_dir.path().join("ca.pem")).unwrap();
    let ca_key_pem = std::fs::read_to_string(pki_dir.path().join("ca-key.pem")).unwrap();

    rusty_photon_tls::test_cert::generate_service_cert(
        &ca_cert_pem,
        &ca_key_pem,
        "test-service",
        &certs_dir,
    )
    .unwrap();

    // Build TLS config
    let tls_config = rusty_photon_tls::config::TlsConfig {
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
    let listener = rusty_photon_tls::server::bind_dual_stack_tokio(addr)
        .await
        .unwrap();
    let bound_addr = listener.local_addr().unwrap();

    let router = Router::new().route("/health", get(|| async { "ok" }));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        rusty_photon_tls::server::serve_tls(listener, router, &tls_config, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    // Build client with CA trust
    let client =
        rusty_photon_tls::client::build_reqwest_client(Some(&pki_dir.path().join("ca.pem")))
            .unwrap();

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

    rusty_photon_tls::test_cert::generate_ca(pki_dir.path()).unwrap();

    let ca_cert_pem = std::fs::read_to_string(pki_dir.path().join("ca.pem")).unwrap();
    let ca_key_pem = std::fs::read_to_string(pki_dir.path().join("ca-key.pem")).unwrap();

    rusty_photon_tls::test_cert::generate_service_cert(
        &ca_cert_pem,
        &ca_key_pem,
        "test-service",
        &certs_dir,
    )
    .unwrap();

    let tls_config = rusty_photon_tls::config::TlsConfig {
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
    let listener = rusty_photon_tls::server::bind_dual_stack_tokio(addr)
        .await
        .unwrap();
    let bound_addr = listener.local_addr().unwrap();

    let router = Router::new().route("/health", get(|| async { "ok" }));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        rusty_photon_tls::server::serve_tls(listener, router, &tls_config, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    // Build client WITHOUT CA trust — should reject the self-signed cert
    let client = rusty_photon_tls::client::build_reqwest_client(None).unwrap();

    let url = format!("https://localhost:{}/health", bound_addr.port());
    let result = client.get(&url).send().await;
    assert!(result.is_err(), "should reject untrusted certificate");

    // Shutdown
    shutdown_tx.send(()).ok();
    server_handle.await.ok();
}

/// Handshake against `addr` trusting `ca_path`, returning the peer's leaf
/// certificate DER.
async fn peer_cert_der(addr: SocketAddr, ca_path: &std::path::Path) -> Vec<u8> {
    let ca_pem = std::fs::read_to_string(ca_path).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut ca_pem.as_bytes()) {
        roots.add(cert.unwrap()).unwrap();
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tls = connector.connect(name, tcp).await.unwrap();
    tls.get_ref().1.peer_certificates().unwrap()[0].to_vec()
}

#[tokio::test]
async fn swapped_pair_is_served_without_rebinding() {
    let pki_dir = tempfile::tempdir().unwrap();
    rusty_photon_tls::test_cert::generate_ca(pki_dir.path()).unwrap();
    let ca_cert_pem = std::fs::read_to_string(pki_dir.path().join("ca.pem")).unwrap();
    let ca_key_pem = std::fs::read_to_string(pki_dir.path().join("ca-key.pem")).unwrap();
    rusty_photon_tls::test_cert::generate_service_cert(
        &ca_cert_pem,
        &ca_key_pem,
        "test-service",
        pki_dir.path(),
    )
    .unwrap();
    let cert_path = pki_dir.path().join("test-service.pem");
    let key_path = pki_dir.path().join("test-service-key.pem");
    // Backdate the first pair so the rewrite below is a visible mtime change.
    for path in [&cert_path, &key_path] {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap();
    }

    let resolver = rusty_photon_tls::resolver::ReloadableCertResolver::load(&cert_path, &key_path)
        .unwrap()
        .with_check_interval(std::time::Duration::ZERO);
    let acceptor = rusty_photon_tls::server::acceptor_from_resolver(resolver);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = rusty_photon_tls::server::bind_dual_stack_tokio(addr)
        .await
        .unwrap();
    let bound_addr = listener.local_addr().unwrap();
    let router = Router::new().route("/health", get(|| async { "ok" }));
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        rusty_photon_tls::server::serve_tls_with_acceptor(listener, router, acceptor, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    let ca_path = pki_dir.path().join("ca.pem");
    let before = peer_cert_der(bound_addr, &ca_path).await;

    // Re-issue the pair from the same CA — new keypair, same paths.
    rusty_photon_tls::test_cert::generate_service_cert(
        &ca_cert_pem,
        &ca_key_pem,
        "test-service",
        pki_dir.path(),
    )
    .unwrap();

    let after = peer_cert_der(bound_addr, &ca_path).await;
    assert_ne!(before, after, "the new pair should be served in-process");

    // The swapped pair still serves real requests on the same listener.
    let client = rusty_photon_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let url = format!("https://localhost:{}/health", bound_addr.port());
    let response = client.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);

    shutdown_tx.send(()).ok();
    server_handle.await.ok();
}

/// Set up a TLS server (CA + service cert) on an OS-assigned port, returning
/// the pki dir (kept alive for its `TempDir` drop), the bound address, a
/// shutdown handle, and the server task's join handle.
async fn start_tls_server_with_health_route() -> (
    tempfile::TempDir,
    SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let pki_dir = tempfile::tempdir().unwrap();
    let certs_dir = pki_dir.path().join("certs");

    rusty_photon_tls::test_cert::generate_ca(pki_dir.path()).unwrap();
    let ca_cert_pem = std::fs::read_to_string(pki_dir.path().join("ca.pem")).unwrap();
    let ca_key_pem = std::fs::read_to_string(pki_dir.path().join("ca-key.pem")).unwrap();
    rusty_photon_tls::test_cert::generate_service_cert(
        &ca_cert_pem,
        &ca_key_pem,
        "test-service",
        &certs_dir,
    )
    .unwrap();

    let tls_config = rusty_photon_tls::config::TlsConfig {
        cert: certs_dir
            .join("test-service.pem")
            .to_string_lossy()
            .into_owned(),
        key: certs_dir
            .join("test-service-key.pem")
            .to_string_lossy()
            .into_owned(),
    };

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = rusty_photon_tls::server::bind_dual_stack_tokio(addr)
        .await
        .unwrap();
    let bound_addr = listener.local_addr().unwrap();
    let router = Router::new().route("/health", get(|| async { "ok" }));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        rusty_photon_tls::server::serve_tls(listener, router, &tls_config, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    (pki_dir, bound_addr, shutdown_tx, server_handle)
}

#[tokio::test]
async fn plaintext_http_request_is_redirected_to_https_same_port() {
    let (pki_dir, bound_addr, shutdown_tx, server_handle) =
        start_tls_server_with_health_route().await;

    // A plain HTTP client hitting the TLS port gets a 308 to https on the
    // same port — this is the misdirected-bookmark scenario from #610.
    let plain_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{}/health", bound_addr.port());
    let response = plain_client.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 308);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        location,
        format!("https://127.0.0.1:{}/health", bound_addr.port())
    );

    // A client that trusts the CA and follows redirects completes the round
    // trip entirely over the same port.
    let ca_path = pki_dir.path().join("ca.pem");
    let following_client = rusty_photon_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let response = following_client.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        response.url().as_str().starts_with("https://"),
        "{}",
        response.url()
    );
    assert_eq!(response.text().await.unwrap(), "ok");

    shutdown_tx.send(()).ok();
    server_handle.await.ok();
}

#[tokio::test]
async fn non_http_garbage_on_tls_port_is_dropped_without_a_response() {
    let (_pki_dir, bound_addr, shutdown_tx, server_handle) =
        start_tls_server_with_health_route().await;

    let mut socket = tokio::net::TcpStream::connect(bound_addr).await.unwrap();
    socket
        .write_all(b"not TLS and not an HTTP request, just garbage\n")
        .await
        .unwrap();
    socket.shutdown().await.ok();

    let mut buf = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        socket.read_to_end(&mut buf),
    )
    .await
    .expect("the connection should close well within the bound, not hang")
    .unwrap();
    assert!(
        buf.is_empty(),
        "no response bytes should be sent back for non-HTTP garbage: {buf:?}"
    );

    shutdown_tx.send(()).ok();
    server_handle.await.ok();
}

#[tokio::test]
async fn plain_http_roundtrip() {
    // Start plain HTTP server
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = rusty_photon_tls::server::bind_dual_stack_tokio(addr)
        .await
        .unwrap();
    let bound_addr = listener.local_addr().unwrap();

    let router = Router::new().route("/health", get(|| async { "ok" }));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        rusty_photon_tls::server::serve_plain(listener, router, async {
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

#[tokio::test(start_paused = true)]
async fn stalled_tls_handshake_is_dropped_after_the_timeout() {
    let (_pki_dir, bound_addr, shutdown_tx, server_handle) =
        start_tls_server_with_health_route().await;

    let mut socket = tokio::net::TcpStream::connect(bound_addr).await.unwrap();
    // The 0x16 first byte routes the server to the TLS acceptor; stalling
    // without completing the ClientHello must not hold the connection (and
    // its spawned task) open forever.
    socket.write_all(&[0x16]).await.unwrap();

    tokio::time::advance(std::time::Duration::from_secs(11)).await;

    let mut buf = [0u8; 1];
    let n = socket.read(&mut buf).await.unwrap();
    assert_eq!(
        n, 0,
        "the stalled handshake should be dropped, not held open"
    );

    shutdown_tx.send(()).ok();
    server_handle.await.ok();
}
