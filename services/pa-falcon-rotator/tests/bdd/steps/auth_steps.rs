//! BDD step definitions for the pa-falcon-rotator TLS + HTTP Basic Auth
//! smoke test.
//!
//! Mirrors the service's in-process BDD pattern: the scenario builds a
//! `BoundServer` on an ephemeral port with the mock serial factory, but —
//! unlike `World::start_service`, which clears `tls` / `auth` — keeps the
//! staged TLS + auth config and probes the server over HTTPS.

use std::net::SocketAddr;
use std::sync::Arc;

use cucumber::{given, then, when};
use pa_falcon_rotator::{AlpacaServerConfig, Config, MockFalconTransportFactory, ServerBuilder};
use rusty_photon_shared_transport::TransportFactory;
use tempfile::TempDir;

use crate::world::FalconRotatorWorld;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given("generated TLS certificates for pa-falcon-rotator")]
fn generate_tls_certs(world: &mut FalconRotatorWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "pa-falcon-rotator", &[], &certs_dir)
        .unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("pa-falcon-rotator is configured with TLS and auth enabled and mock serial")]
fn configured_with_tls_and_auth(world: &mut FalconRotatorWorld) {
    let certs_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("certs");
    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();

    let mut config = Config::default();
    config.serial.port = "/dev/mock".to_string();
    // Ephemeral port so concurrent scenarios never collide.
    let mut server = AlpacaServerConfig::new(0);
    server.tls = Some(rp_tls::config::TlsConfig {
        cert: certs_dir
            .join("pa-falcon-rotator.pem")
            .to_string_lossy()
            .to_string(),
        key: certs_dir
            .join("pa-falcon-rotator-key.pem")
            .to_string_lossy()
            .to_string(),
    });
    server.auth = Some(rp_auth::config::AuthConfig {
        username: AUTH_USERNAME.to_string(),
        password_hash: hash,
    });
    config.server = server;
    world.pending_config = Some(config);
}

#[when("pa-falcon-rotator is started with TLS and auth")]
async fn started_with_tls_and_auth(world: &mut FalconRotatorWorld) {
    let config = world.pending_config.take().expect("config not staged");

    let mock = Arc::new(MockFalconTransportFactory::default());
    let factory: Arc<dyn TransportFactory> = Arc::clone(&mock) as _;
    let bound = ServerBuilder::new()
        .with_config(config)
        .with_factory(factory)
        .build()
        .await
        .expect("build in-process Alpaca server with TLS + auth");
    let local_addr = bound.listen_addr();

    let server_handle = tokio::spawn(async move {
        let _ = bound.start(std::future::pending::<()>()).await;
    });
    world.mock = Some(mock);
    world.server_handle = Some(server_handle);
    world.server_addr = Some(SocketAddr::from(([127, 0, 0, 1], local_addr.port())));
}

/// Build an HTTPS client trusting the scenario's generated CA.
fn https_client(world: &FalconRotatorWorld) -> reqwest::Client {
    let ca_path = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("ca.pem");
    rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap()
}

fn management_url(world: &FalconRotatorWorld) -> String {
    let port = world.server_addr.expect("server not started").port();
    format!("https://localhost:{port}/management/v1/configureddevices")
}

/// Poll with valid credentials until the freshly started server answers 200.
async fn wait_until_ready(client: &reqwest::Client, url: &str) {
    for _ in 0..60 {
        if let Ok(resp) = client
            .get(url)
            .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
            .send()
            .await
        {
            if resp.status().as_u16() == 200 {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("server did not become ready over HTTPS with valid credentials");
}

#[then("the Alpaca management endpoint should reject missing credentials with 401")]
async fn rejects_missing_credentials(world: &mut FalconRotatorWorld) {
    let client = https_client(world);
    let url = management_url(world);
    wait_until_ready(&client, &url).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the Alpaca management endpoint should respond with valid credentials")]
async fn responds_with_valid_credentials(world: &mut FalconRotatorWorld) {
    let client = https_client(world);
    let url = management_url(world);

    let resp = client
        .get(&url)
        .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}
