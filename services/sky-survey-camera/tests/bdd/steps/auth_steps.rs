//! BDD step definitions for the sky-survey-camera TLS + HTTP Basic Auth
//! smoke test.

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::SkySurveyCameraWorld;
use bdd_infra::ServiceHandle;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given("generated TLS certificates for sky-survey-camera")]
fn generate_tls_certs(world: &mut SkySurveyCameraWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "sky-survey-camera", &[], &certs_dir)
        .unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("sky-survey-camera is configured with TLS and auth enabled and a stub survey backend")]
async fn configured_with_tls_and_auth(world: &mut SkySurveyCameraWorld) {
    // Point the survey endpoint at a local stub so the config never
    // references the real SkyView URL (the scenario never fetches).
    world.spawn_skyview_stub().await;

    let certs_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("certs");
    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();

    // Reuse the World's config template (default optics, stubbed
    // survey, port 0) and switch its server block to TLS + auth.
    let mut config = world.build_config_json();
    config["server"] = serde_json::json!({
        "port": 0,
        "tls": {
            "cert": certs_dir.join("sky-survey-camera.pem").to_string_lossy().to_string(),
            "key": certs_dir.join("sky-survey-camera-key.pem").to_string_lossy().to_string()
        },
        "auth": { "username": AUTH_USERNAME, "password_hash": hash }
    });
    world.pending_config = Some(config);
}

#[when("sky-survey-camera is started with TLS and auth")]
async fn started_with_tls_and_auth(world: &mut SkySurveyCameraWorld) {
    let config = world.pending_config.take().expect("config not staged");
    let config_path = {
        let dir = world.temp_dir();
        let path = dir.path().join("auth-smoke-config.json");
        std::fs::write(&path, config.to_string()).expect("failed to write config");
        path
    };

    let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;
    world.config_path = Some(config_path);
    world.service = Some(handle);
}

/// Build an HTTPS client trusting the scenario's generated CA.
fn https_client(world: &SkySurveyCameraWorld) -> reqwest::Client {
    let ca_path = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("ca.pem");
    rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap()
}

fn management_url(world: &SkySurveyCameraWorld) -> String {
    let port = world.service.as_ref().expect("service not started").port;
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
async fn rejects_missing_credentials(world: &mut SkySurveyCameraWorld) {
    let client = https_client(world);
    let url = management_url(world);
    wait_until_ready(&client, &url).await;

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the Alpaca management endpoint should respond with valid credentials")]
async fn responds_with_valid_credentials(world: &mut SkySurveyCameraWorld) {
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
