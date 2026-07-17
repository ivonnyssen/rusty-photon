//! BDD step definitions for rp HTTP Basic Auth and hash-password CLI.
//! The auth scenarios build on the shared bdd-infra PKI + credentials
//! fixture (see `world.pki()`) and probe `/health` over HTTPS.

use cucumber::{given, then, when};

use bdd_infra::tls_auth::wait_until_ready;
use bdd_infra::ServiceHandle;

use crate::world::RpWorld;

// ---------------------------------------------------------------------------
// Auth config steps
// ---------------------------------------------------------------------------

#[given("rp is configured with TLS and auth enabled")]
fn rp_configured_with_tls_and_auth(_world: &mut RpWorld) {
    // Marker — config is built in the When step
}

#[when("rp is started with auth")]
async fn rp_started_with_auth(world: &mut RpWorld) {
    let mut config = world.build_config();
    config["server"]["tls"] = world.pki().tls_block();
    config["server"]["auth"] = world.pki().auth_block();

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

    // Wait for healthy over HTTPS with valid credentials.
    let pki = world.pki();
    let client = pki.https_client();
    let port = world.rp.as_ref().unwrap().port;
    let url = format!("https://localhost:{}/health", port);
    wait_until_ready(&client, &url, pki.username(), pki.password()).await;
}

// ---------------------------------------------------------------------------
// Auth validation steps
// ---------------------------------------------------------------------------

#[then("the health endpoint should respond with valid credentials")]
async fn health_responds_with_auth(world: &mut RpWorld) {
    let pki = world.pki();
    let client = pki.https_client();
    let port = world.rp.as_ref().expect("rp not started").port;
    let url = format!("https://localhost:{}/health", port);

    let resp = client
        .get(&url)
        .basic_auth(pki.username(), Some(pki.password()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

#[then("the health endpoint should reject wrong credentials with 401")]
async fn health_rejects_wrong_credentials(world: &mut RpWorld) {
    let pki = world.pki();
    let client = pki.https_client();
    let port = world.rp.as_ref().expect("rp not started").port;
    let url = format!("https://localhost:{}/health", port);

    let resp = client
        .get(&url)
        .basic_auth(pki.username(), Some("wrong-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the health endpoint should reject missing credentials with 401")]
async fn health_rejects_missing_credentials(world: &mut RpWorld) {
    let client = world.pki().https_client();
    let port = world.rp.as_ref().expect("rp not started").port;
    let url = format!("https://localhost:{}/health", port);

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the rp 401 response should include a WWW-Authenticate header")]
async fn rp_401_includes_www_authenticate(world: &mut RpWorld) {
    let client = world.pki().https_client();
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
