//! BDD step definitions for the phd2-guider TLS + HTTP Basic Auth smoke test.

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::GuiderWorld;
use bdd_infra::ServiceHandle;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given("generated TLS certificates for phd2-guider")]
fn generate_tls_certs(world: &mut GuiderWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "phd2-guider", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("phd2-guider is configured with TLS and auth enabled pointing at the mock PHD2")]
fn configured_with_tls_and_auth(world: &mut GuiderWorld) {
    let certs_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("certs");
    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();
    let phd2_port = world.mock.as_ref().expect("mock PHD2 not started").port;

    // Same shape as `GuiderWorld::start_service`'s config, plus the
    // shared server config's `tls` and `auth` blocks.
    world.pending_config = Some(serde_json::json!({
        "server": {
            "port": 0,
            "tls": {
                "cert": certs_dir.join("phd2-guider.pem").to_string_lossy().to_string(),
                "key": certs_dir.join("phd2-guider-key.pem").to_string_lossy().to_string()
            },
            "auth": { "username": AUTH_USERNAME, "password_hash": hash }
        },
        "stop_timeout": "10s",
        "phd2": {
            "host": "127.0.0.1",
            "port": phd2_port,
            "connection_timeout": "2s",
            "command_timeout": "5s",
            "reconnect": { "enabled": true, "interval": "200ms" }
        },
        "settling": { "pixels": 0.5, "time": "10s", "timeout": "60s" }
    }));
}

#[when("phd2-guider is started with TLS and auth")]
async fn started_with_tls_and_auth(world: &mut GuiderWorld) {
    let config = world.pending_config.take().expect("config not staged");
    let dir = world.temp_dir_path();
    let config_path = dir.join("auth-smoke-config.json");
    std::fs::write(&config_path, config.to_string()).expect("failed to write config");
    let config_str = config_path.to_string_lossy().into_owned();

    let handle =
        ServiceHandle::start_with_args("phd2-guider", &["--config", &config_str, "serve"]).await;
    world.service_handle = Some(handle);
}

/// Build an HTTPS client trusting the scenario's generated CA.
fn https_client(world: &GuiderWorld) -> reqwest::Client {
    let ca_path = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("ca.pem");
    rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap()
}

fn health_url(world: &GuiderWorld) -> String {
    let port = world
        .service_handle
        .as_ref()
        .expect("service not started")
        .port;
    format!("https://localhost:{port}/health")
}

/// Poll with valid credentials until the freshly spawned server answers
/// 200 (which also requires the PHD2 connection to be established).
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

#[then("the health endpoint should reject missing credentials with 401")]
async fn rejects_missing_credentials(world: &mut GuiderWorld) {
    let client = https_client(world);
    let url = health_url(world);
    wait_until_ready(&client, &url).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the health endpoint should respond with valid credentials")]
async fn responds_with_valid_credentials(world: &mut GuiderWorld) {
    let client = https_client(world);
    let url = health_url(world);

    let resp = client
        .get(&url)
        .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}
