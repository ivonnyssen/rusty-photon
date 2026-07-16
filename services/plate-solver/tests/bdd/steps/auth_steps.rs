//! BDD step definitions for the plate-solver TLS + HTTP Basic Auth smoke test.

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::PlateSolverWorld;
use bdd_infra::ServiceHandle;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given("generated TLS certificates for plate-solver")]
fn generate_tls_certs(world: &mut PlateSolverWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "plate-solver", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("plate-solver is configured with TLS and auth enabled and mock astap")]
fn configured_with_tls_and_auth(world: &mut PlateSolverWorld) {
    let certs_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("certs");
    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();

    // Reuse the suite's mock_astap + fake db dir fixtures so startup
    // validation passes without a real ASTAP install.
    let mock_path = PlateSolverWorld::mock_astap_path();
    let dir = world.temp_dir_path();
    let db_dir = dir.join("db");
    std::fs::create_dir_all(&db_dir).expect("mkdir db");

    let config = serde_json::json!({
        "server": {
            "port": 0,
            "tls": {
                "cert": certs_dir.join("plate-solver.pem").to_string_lossy().to_string(),
                "key": certs_dir.join("plate-solver-key.pem").to_string_lossy().to_string()
            },
            "auth": { "username": AUTH_USERNAME, "password_hash": hash }
        },
        "astap_binary_path": mock_path.to_string_lossy(),
        "astap_db_directory": db_dir.to_string_lossy(),
    });
    world.pending_config = config
        .as_object()
        .expect("config JSON is an object")
        .clone();
}

#[when("plate-solver is started with TLS and auth")]
async fn started_with_tls_and_auth(world: &mut PlateSolverWorld) {
    let dir = world.temp_dir_path();
    let config_path = dir.join("auth-smoke-config.json");
    let body = serde_json::Value::Object(world.pending_config.clone()).to_string();
    std::fs::write(&config_path, body).expect("failed to write config");

    let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;
    world.service_handle = Some(handle);
}

/// Build an HTTPS client trusting the scenario's generated CA.
fn https_client(world: &PlateSolverWorld) -> reqwest::Client {
    let ca_path = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("ca.pem");
    rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap()
}

fn health_url(world: &PlateSolverWorld) -> String {
    let port = world
        .service_handle
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
async fn rejects_missing_credentials(world: &mut PlateSolverWorld) {
    let client = https_client(world);
    let url = health_url(world);
    wait_until_ready(&client, &url).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the health endpoint should respond with valid credentials")]
async fn responds_with_valid_credentials(world: &mut PlateSolverWorld) {
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
