//! BDD step definitions for ppba-driver HTTP Basic Auth

use cucumber::{given, then, when};

use crate::steps::infrastructure::ServiceHandle;
use crate::world::PpbaWorld;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given("ppba-driver is configured with TLS and auth enabled and mock serial")]
fn ppba_configured_with_tls_and_auth(world: &mut PpbaWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let certs_dir = pki_dir.join("certs");

    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();

    world.config = serde_json::json!({
        "serial": { "port": "/dev/mock", "baud_rate": 9600, "polling_interval_ms": 60000, "timeout_seconds": 2 },
        "server": {
            "port": 0,
            "tls": {
                "cert": certs_dir.join("ppba-driver.pem").to_string_lossy().to_string(),
                "key": certs_dir.join("ppba-driver-key.pem").to_string_lossy().to_string()
            },
            "auth": {
                "username": AUTH_USERNAME,
                "password_hash": hash
            }
        },
        "switch": { "name": "Test Switch", "unique_id": "test-switch", "description": "Test", "device_number": 0, "enabled": true },
        "observingconditions": { "name": "Test OC", "unique_id": "test-oc", "description": "Test", "device_number": 0, "enabled": false }
    });

    world.auth_password = Some(AUTH_PASSWORD.to_string());
}

#[given("ppba-driver is configured without auth and with mock serial")]
fn ppba_configured_without_auth(world: &mut PpbaWorld) {
    world.config = serde_json::json!({
        "serial": { "port": "/dev/mock", "baud_rate": 9600, "polling_interval_ms": 60000, "timeout_seconds": 2 },
        "server": { "port": 0 },
        "switch": { "name": "Test Switch", "unique_id": "test-switch", "description": "Test", "device_number": 0, "enabled": true },
        "observingconditions": { "name": "Test OC", "unique_id": "test-oc", "description": "Test", "device_number": 0, "enabled": false }
    });
}

#[when("ppba-driver is started with TLS and auth")]
async fn ppba_started_with_tls_and_auth(world: &mut PpbaWorld) {
    let config_path = std::env::temp_dir()
        .join(format!(
            "ppba-auth-test-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .to_string();
    tokio::fs::write(
        &config_path,
        serde_json::to_string_pretty(&world.config).unwrap(),
    )
    .await
    .unwrap();

    let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await;

    world.base_url = Some(format!("https://localhost:{}", handle.port));
    world.ppba = Some(handle);
}

#[when("ppba-driver is started without auth")]
async fn ppba_started_without_auth(world: &mut PpbaWorld) {
    let config_path = std::env::temp_dir()
        .join(format!(
            "ppba-noauth-test-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .to_string();
    tokio::fs::write(
        &config_path,
        serde_json::to_string_pretty(&world.config).unwrap(),
    )
    .await
    .unwrap();

    let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await;

    world.base_url = Some(format!("http://127.0.0.1:{}", handle.port));
    world.ppba = Some(handle);
}

#[then("the Alpaca management endpoint should respond with valid credentials")]
async fn alpaca_management_responds_with_auth(world: &mut PpbaWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.ppba.as_ref().expect("ppba-driver not started").port;
    let url = format!("https://localhost:{}/management/v1/configureddevices", port);

    let mut ok = false;
    for _ in 0..60 {
        if let Ok(resp) = client
            .get(&url)
            .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
            .send()
            .await
        {
            if resp.status().as_u16() == 200 {
                ok = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        ok,
        "Alpaca management endpoint did not respond with valid credentials"
    );
}

#[then("the Alpaca management endpoint should reject wrong credentials with 401")]
async fn alpaca_rejects_wrong_credentials(world: &mut PpbaWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.ppba.as_ref().expect("ppba-driver not started").port;
    let url = format!("https://localhost:{}/management/v1/configureddevices", port);

    // First wait for the server to be ready with correct credentials
    let mut ready = false;
    for _ in 0..60 {
        if let Ok(resp) = client
            .get(&url)
            .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
            .send()
            .await
        {
            if resp.status().as_u16() == 200 {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(ready, "Server did not become ready");

    // Now test with wrong credentials
    let resp = client
        .get(&url)
        .basic_auth(AUTH_USERNAME, Some("wrong-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the Alpaca management endpoint should reject missing credentials with 401")]
async fn alpaca_rejects_missing_credentials(world: &mut PpbaWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.ppba.as_ref().expect("ppba-driver not started").port;
    let url = format!("https://localhost:{}/management/v1/configureddevices", port);

    // First wait for server readiness
    let mut ready = false;
    for _ in 0..60 {
        if let Ok(resp) = client
            .get(&url)
            .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
            .send()
            .await
        {
            if resp.status().as_u16() == 200 {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(ready, "Server did not become ready");

    // Now test without credentials
    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[then("the 401 response should include a WWW-Authenticate header")]
async fn response_includes_www_authenticate(world: &mut PpbaWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.ppba.as_ref().expect("ppba-driver not started").port;
    let url = format!("https://localhost:{}/management/v1/configureddevices", port);

    // Wait for server readiness
    let mut ready = false;
    for _ in 0..60 {
        if let Ok(resp) = client
            .get(&url)
            .basic_auth(AUTH_USERNAME, Some(AUTH_PASSWORD))
            .send()
            .await
        {
            if resp.status().as_u16() == 200 {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(ready, "Server did not become ready");

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

#[then("the Alpaca management endpoint should respond without credentials")]
async fn alpaca_responds_without_credentials(world: &mut PpbaWorld) {
    let client = reqwest::Client::new();
    let port = world.ppba.as_ref().expect("ppba-driver not started").port;
    let url = format!("http://127.0.0.1:{}/management/v1/configureddevices", port);

    let mut ok = false;
    for _ in 0..60 {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().as_u16() == 200 {
                ok = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        ok,
        "Alpaca management endpoint did not respond without auth"
    );
}
