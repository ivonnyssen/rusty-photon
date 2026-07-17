//! BDD step definitions for the zwo-focuser TLS + HTTP Basic Auth smoke test.

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::FocuserWorld;
use bdd_infra::ServiceHandle;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given("generated TLS certificates for zwo-focuser")]
fn generate_tls_certs(world: &mut FocuserWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "zwo-focuser", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("zwo-focuser is configured with TLS and auth enabled on the simulation backend")]
fn configured_with_tls_and_auth(world: &mut FocuserWorld) {
    let certs_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("certs");
    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();

    world.pending_config = Some(serde_json::json!({
        "devices": {},
        "server": {
            "port": 0,
            "tls": {
                "cert": certs_dir.join("zwo-focuser.pem").to_string_lossy().to_string(),
                "key": certs_dir.join("zwo-focuser-key.pem").to_string_lossy().to_string()
            },
            "auth": { "username": AUTH_USERNAME, "password_hash": hash }
        }
    }));
}

#[when("zwo-focuser is started with TLS and auth")]
async fn started_with_tls_and_auth(world: &mut FocuserWorld) {
    let config = world.pending_config.take().expect("config not staged");
    let dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
    let config_path = dir.path().join("auth-smoke-config.json");
    std::fs::write(&config_path, config.to_string()).expect("failed to write config");

    let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;
    world.handle = Some(handle);
}

/// Build an HTTPS client trusting the scenario's generated CA.
fn https_client(world: &FocuserWorld) -> reqwest::Client {
    let ca_path = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("ca.pem");
    rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap()
}

fn management_url(world: &FocuserWorld) -> String {
    let port = world.handle.as_ref().expect("service not started").port;
    format!("https://localhost:{port}/management/v1/configureddevices")
}

/// Poll with valid credentials until the freshly spawned server answers 200.
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
async fn rejects_missing_credentials(world: &mut FocuserWorld) {
    let client = https_client(world);
    let url = management_url(world);
    wait_until_ready(&client, &url).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the Alpaca management endpoint should respond with valid credentials")]
async fn responds_with_valid_credentials(world: &mut FocuserWorld) {
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
