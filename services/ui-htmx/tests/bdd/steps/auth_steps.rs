//! BDD step definitions for the ui-htmx TLS + HTTP Basic Auth suites
//! (`auth.feature`, `tls.feature`). The BFF is spawned with a generated CA +
//! service certificate and probed at `/health` — auth wraps the whole router,
//! so the liveness route is enough to prove the credential gate.

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::UiWorld;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given("generated TLS certificates for ui-htmx")]
fn generate_tls_certs(world: &mut UiWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "ui-htmx", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

/// The staged `server.tls` block pointing at the generated service cert.
fn tls_block(world: &UiWorld) -> serde_json::Value {
    let certs_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("certs");
    serde_json::json!({
        "cert": certs_dir.join("ui-htmx.pem").to_string_lossy().to_string(),
        "key": certs_dir.join("ui-htmx-key.pem").to_string_lossy().to_string()
    })
}

#[given("ui-htmx is configured with TLS and auth enabled")]
fn configured_with_tls_and_auth(world: &mut UiWorld) {
    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();
    world.pending_config = Some(serde_json::json!({
        "server": {
            "port": 0,
            "tls": tls_block(world),
            "auth": { "username": AUTH_USERNAME, "password_hash": hash }
        },
        "drivers": {}
    }));
}

#[given("ui-htmx is configured with TLS enabled and no auth")]
fn configured_with_tls_only(world: &mut UiWorld) {
    world.pending_config = Some(serde_json::json!({
        "server": { "port": 0, "tls": tls_block(world) },
        "drivers": {}
    }));
}

#[given("ui-htmx is configured without TLS or auth")]
fn configured_plain(world: &mut UiWorld) {
    world.pending_config = Some(serde_json::json!({
        "server": { "port": 0, "bind_address": "127.0.0.1" },
        "drivers": {}
    }));
}

#[when("ui-htmx is started with TLS and auth")]
async fn started_with_tls_and_auth(world: &mut UiWorld) {
    world.start_bff_from_pending_config().await;
}

#[when("ui-htmx is started with TLS")]
async fn started_with_tls(world: &mut UiWorld) {
    world.start_bff_from_pending_config().await;
}

#[when("ui-htmx is started without TLS or auth")]
async fn started_plain(world: &mut UiWorld) {
    world.start_bff_from_pending_config().await;
}

/// Build an HTTPS client trusting the scenario's generated CA.
fn https_client(world: &UiWorld) -> reqwest::Client {
    let ca_path = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("ca.pem");
    rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap()
}

fn health_url(world: &UiWorld, scheme: &str) -> String {
    let port = world.ui.as_ref().expect("BFF not started").port;
    format!("{scheme}://localhost:{port}/health")
}

/// Poll `/health` until the freshly spawned server answers 200 — with Basic
/// credentials when `with_credentials`, bare otherwise.
async fn wait_until_ready(client: &reqwest::Client, url: &str, with_credentials: bool) {
    for _ in 0..60 {
        let mut request = client.get(url);
        if with_credentials {
            request = request.basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD));
        }
        if let Ok(resp) = request.send().await {
            if resp.status().as_u16() == 200 {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("BFF did not answer 200 at {url} (with_credentials={with_credentials})");
}

#[then("the health endpoint should respond with valid credentials")]
async fn responds_with_valid_credentials(world: &mut UiWorld) {
    let client = https_client(world);
    let url = health_url(world, "https");
    wait_until_ready(&client, &url, true).await;

    let resp = client
        .get(&url)
        .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

#[then("the health endpoint should reject wrong credentials with 401")]
async fn rejects_wrong_credentials(world: &mut UiWorld) {
    let client = https_client(world);
    let url = health_url(world, "https");
    wait_until_ready(&client, &url, true).await;

    let resp = client
        .get(&url)
        .basic_auth(AUTH_USERNAME, Some("wrong-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the health endpoint should reject missing credentials with 401")]
async fn rejects_missing_credentials(world: &mut UiWorld) {
    let client = https_client(world);
    let url = health_url(world, "https");
    wait_until_ready(&client, &url, true).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the 401 response should include a WWW-Authenticate header")]
async fn response_includes_www_authenticate(world: &mut UiWorld) {
    let client = https_client(world);
    let url = health_url(world, "https");
    wait_until_ready(&client, &url, true).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let www_auth = resp
        .headers()
        .get("www-authenticate")
        .expect("missing WWW-Authenticate header")
        .to_str()
        .unwrap();
    assert_eq!(www_auth, "Basic realm=\"Rusty Photon\"");
}

#[then("the health endpoint should respond without credentials")]
async fn responds_without_credentials(world: &mut UiWorld) {
    let client = reqwest::Client::new();
    let port = world.ui.as_ref().expect("BFF not started").port;
    let url = format!("http://127.0.0.1:{port}/health");
    wait_until_ready(&client, &url, false).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

#[then("the health endpoint should answer over HTTPS without credentials")]
async fn answers_over_https_without_credentials(world: &mut UiWorld) {
    let client = https_client(world);
    let url = health_url(world, "https");
    wait_until_ready(&client, &url, false).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}
