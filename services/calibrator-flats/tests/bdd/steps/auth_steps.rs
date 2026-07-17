//! BDD step definitions for the calibrator-flats TLS + HTTP Basic Auth
//! smoke test. Unlike the workflow suite, this scenario spawns ONLY
//! calibrator-flats itself, with a temp config — no OmniSim, no rp.

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::CalibratorFlatsWorld;
use bdd_infra::rp_harness::{build_calibrator_flats_config, write_temp_config_file};
use bdd_infra::ServiceHandle;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given("generated TLS certificates for calibrator-flats")]
fn generate_tls_certs(world: &mut CalibratorFlatsWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "calibrator-flats", &[], &certs_dir)
        .unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("calibrator-flats is configured with TLS and auth enabled")]
fn configured_with_tls_and_auth(world: &mut CalibratorFlatsWorld) {
    let certs_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("certs");
    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();

    // The suite's usual plan, plus the shared `server` block: an ephemeral
    // port with TLS and auth. The plan is never invoked — the scenario
    // only probes /health.
    let mut config = build_calibrator_flats_config(&[("Luminance".to_string(), 1)]);
    config["server"] = serde_json::json!({
        "port": 0,
        "tls": {
            "cert": certs_dir.join("calibrator-flats.pem").to_string_lossy().to_string(),
            "key": certs_dir.join("calibrator-flats-key.pem").to_string_lossy().to_string()
        },
        "auth": { "username": AUTH_USERNAME, "password_hash": hash }
    });
    world.pending_config = Some(config);
}

#[when("calibrator-flats is started with TLS and auth")]
async fn started_with_tls_and_auth(world: &mut CalibratorFlatsWorld) {
    let config = world.pending_config.take().expect("config not staged");
    let config_path = write_temp_config_file("calibrator-flats-auth-config", &config).await;

    world.calibrator_flats = Some(ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await);
}

/// Build an HTTPS client trusting the scenario's generated CA.
fn https_client(world: &CalibratorFlatsWorld) -> reqwest::Client {
    let ca_path = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("ca.pem");
    rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap()
}

fn health_url(world: &CalibratorFlatsWorld) -> String {
    let port = world
        .calibrator_flats
        .as_ref()
        .expect("service not started")
        .port;
    format!("https://localhost:{port}/health")
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

#[then("the health endpoint should reject missing credentials with 401")]
async fn rejects_missing_credentials(world: &mut CalibratorFlatsWorld) {
    let client = https_client(world);
    let url = health_url(world);
    wait_until_ready(&client, &url).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the health endpoint should respond with valid credentials")]
async fn responds_with_valid_credentials(world: &mut CalibratorFlatsWorld) {
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
