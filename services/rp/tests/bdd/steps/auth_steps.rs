//! BDD step definitions for rp HTTP Basic Auth and hash-password CLI

use std::path::PathBuf;

use cucumber::{given, then, when};

use bdd_infra::ServiceHandle;

use crate::world::RpWorld;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

fn pki_dir(world: &RpWorld) -> PathBuf {
    world
        .tls_pki_dir
        .as_ref()
        .expect("pki_dir not set")
        .path()
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// Auth config steps
// ---------------------------------------------------------------------------

#[given("rp is configured with TLS and auth enabled")]
fn rp_configured_with_tls_and_auth(_world: &mut RpWorld) {
    // Marker — config is built in the When step
}

#[when("rp is started with auth")]
async fn rp_started_with_auth(world: &mut RpWorld) {
    let dir = pki_dir(world);
    let certs_dir = dir.join("certs");

    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();

    let mut config = world.build_config();
    config["server"]["tls"] = serde_json::json!({
        "cert": certs_dir.join("rp.pem").to_string_lossy().to_string(),
        "key": certs_dir.join("rp-key.pem").to_string_lossy().to_string()
    });
    config["server"]["auth"] = serde_json::json!({
        "username": AUTH_USERNAME,
        "password_hash": hash
    });

    let config_path = std::env::temp_dir()
        .join(format!(
            "rp-auth-test-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .to_string();
    tokio::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap())
        .await
        .unwrap();

    let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await;

    world.rp = Some(handle);

    // Wait for healthy over HTTPS with auth
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.rp.as_ref().unwrap().port;
    let url = format!("https://localhost:{}/health", port);

    let mut healthy = false;
    for _ in 0..120 {
        if let Ok(resp) = client
            .get(&url)
            .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
            .send()
            .await
        {
            if resp.status().as_u16() == 200 {
                healthy = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        healthy,
        "rp did not become healthy with auth within timeout"
    );
}

// ---------------------------------------------------------------------------
// Auth validation steps
// ---------------------------------------------------------------------------

#[then("the health endpoint should respond with valid credentials")]
async fn health_responds_with_auth(world: &mut RpWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.rp.as_ref().expect("rp not started").port;
    let url = format!("https://localhost:{}/health", port);

    let resp = client
        .get(&url)
        .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

#[then("the health endpoint should reject wrong credentials with 401")]
async fn health_rejects_wrong_credentials(world: &mut RpWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.rp.as_ref().expect("rp not started").port;
    let url = format!("https://localhost:{}/health", port);

    let resp = client
        .get(&url)
        .basic_auth(AUTH_USERNAME, Some("wrong-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the health endpoint should reject missing credentials with 401")]
async fn health_rejects_missing_credentials(world: &mut RpWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.rp.as_ref().expect("rp not started").port;
    let url = format!("https://localhost:{}/health", port);

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the rp 401 response should include a WWW-Authenticate header")]
async fn rp_401_includes_www_authenticate(world: &mut RpWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.rp.as_ref().expect("rp not started").port;
    let url = format!("https://localhost:{}/health", port);

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

// ---------------------------------------------------------------------------
// hash-password CLI steps
// ---------------------------------------------------------------------------

#[when("rp hash-password is executed with a test password via stdin")]
fn hash_password_executed(world: &mut RpWorld) {
    let output = bdd_infra::run_once(
        env!("CARGO_PKG_NAME"),
        &["hash-password", "--stdin"],
        Some(b"test-password\n"),
    );

    assert!(
        output.status.success(),
        "hash-password failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let hash = String::from_utf8(output.stdout).unwrap().trim().to_string();
    world.auth_hash_output = Some(hash);
    world.auth_password = Some("test-password".to_string());
}

#[then("the output should be a valid Argon2id hash string")]
fn output_is_argon2id(world: &mut RpWorld) {
    let hash = world
        .auth_hash_output
        .as_ref()
        .expect("hash output not captured");
    assert!(
        hash.starts_with("$argon2id$"),
        "expected Argon2id hash, got: {hash}"
    );
}

#[then("the hash should verify against the original password")]
fn hash_verifies(world: &mut RpWorld) {
    let hash = world
        .auth_hash_output
        .as_ref()
        .expect("hash output not captured");
    let password = world.auth_password.as_ref().expect("password not stored");
    assert!(
        rp_auth::credentials::verify_password(password, hash),
        "hash did not verify against original password"
    );
}
