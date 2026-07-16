//! BDD step definitions for the session-runner TLS + HTTP Basic Auth smoke
//! test. Unlike the workflow suites, these scenarios spawn ONLY
//! session-runner itself, with a temp config — no OmniSim, no rp.

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::SessionRunnerWorld;
use bdd_infra::rp_harness::write_temp_config_file;
use bdd_infra::ServiceHandle;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given("generated TLS certificates for session-runner")]
fn generate_tls_certs(world: &mut SessionRunnerWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "session-runner", &[], &certs_dir)
        .unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("session-runner is configured with TLS and auth enabled")]
fn configured_with_tls_and_auth(world: &mut SessionRunnerWorld) {
    let certs_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("certs");
    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();

    // The service only needs directories that exist; the smoke scenario
    // never invokes a workflow, so both stay empty.
    let workflows_dir = TempDir::new().expect("cannot create a workflows_dir");
    let state_dir = TempDir::new().expect("cannot create a state_dir");

    world.pending_config = Some(serde_json::json!({
        "server": {
            "port": 0,
            "tls": {
                "cert": certs_dir.join("session-runner.pem").to_string_lossy().to_string(),
                "key": certs_dir.join("session-runner-key.pem").to_string_lossy().to_string()
            },
            "auth": { "username": AUTH_USERNAME, "password_hash": hash }
        },
        "workflows_dir": workflows_dir.path(),
        "state_dir": state_dir.path()
    }));
    world.workflows_dir = Some(workflows_dir);
    world.state_dir = Some(state_dir);
}

#[when("session-runner is started with TLS and auth")]
async fn started_with_tls_and_auth(world: &mut SessionRunnerWorld) {
    let config = world.pending_config.take().expect("config not staged");
    let config_path = write_temp_config_file("session-runner-auth-config", &config).await;

    world.session_runner = Some(ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await);
}

/// Build an HTTPS client trusting the scenario's generated CA.
fn https_client(world: &SessionRunnerWorld) -> reqwest::Client {
    let ca_path = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("ca.pem");
    rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap()
}

fn health_url(world: &SessionRunnerWorld) -> String {
    let port = world
        .session_runner
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
async fn rejects_missing_credentials(world: &mut SessionRunnerWorld) {
    let client = https_client(world);
    let url = health_url(world);
    wait_until_ready(&client, &url).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the health endpoint should respond with valid credentials")]
async fn responds_with_valid_credentials(world: &mut SessionRunnerWorld) {
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
