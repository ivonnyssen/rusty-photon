//! BDD step definitions for qhy-focuser HTTP Basic Auth

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::QhyFocuserWorld;
use bdd_infra::ServiceHandle;

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

fn pki_dir(world: &QhyFocuserWorld) -> std::path::PathBuf {
    world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf()
}

#[given("qhy-focuser is configured with TLS and auth enabled and mock serial")]
fn qhy_configured_with_tls_and_auth(world: &mut QhyFocuserWorld) {
    let pki_dir = pki_dir(world);
    let certs_dir = pki_dir.join("certs");

    // Hash is generated in the When step when writing the config JSON
    world.config = Some(qhy_focuser::Config {
        serial: qhy_focuser::SerialConfig {
            port: "/dev/mock".to_string(),
            polling_interval_ms: 60000,
            ..Default::default()
        },
        server: qhy_focuser::ServerConfig {
            port: 0,
            discovery_port: None,
            tls: Some(rp_tls::config::TlsConfig {
                cert: certs_dir
                    .join("qhy-focuser.pem")
                    .to_string_lossy()
                    .into_owned(),
                key: certs_dir
                    .join("qhy-focuser-key.pem")
                    .to_string_lossy()
                    .into_owned(),
            }),
            auth: None,
        },
        focuser: qhy_focuser::FocuserConfig {
            enabled: true,
            ..Default::default()
        },
    });

    world.auth_password = Some(AUTH_PASSWORD.to_string());
}

#[given("qhy-focuser is configured without auth and with mock serial")]
fn qhy_configured_without_auth(world: &mut QhyFocuserWorld) {
    world.config = Some(qhy_focuser::Config {
        serial: qhy_focuser::SerialConfig {
            port: "/dev/mock".to_string(),
            polling_interval_ms: 60000,
            ..Default::default()
        },
        server: qhy_focuser::ServerConfig {
            port: 0,
            discovery_port: None,
            tls: None,
            auth: None,
        },
        focuser: qhy_focuser::FocuserConfig {
            enabled: true,
            ..Default::default()
        },
    });
}

#[when("qhy-focuser is started with TLS and auth")]
async fn qhy_started_with_tls_and_auth(world: &mut QhyFocuserWorld) {
    let config = world.config.as_ref().expect("config not set");

    // Serialize the typed config to JSON, then inject the auth section
    let mut config_json: serde_json::Value =
        serde_json::to_value(config).expect("failed to serialize config");

    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();
    config_json["server"]["auth"] = serde_json::json!({
        "username": AUTH_USERNAME,
        "password_hash": hash
    });

    let dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = dir.path().join("qhy_auth_config.json");
    std::fs::write(&config_path, config_json.to_string()).unwrap();

    let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;

    world.focuser_handle = Some(handle);
}

#[when("qhy-focuser is started without auth")]
async fn qhy_started_without_auth(world: &mut QhyFocuserWorld) {
    let config = world.config.as_ref().expect("config not set");
    let dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = dir.path().join("qhy_noauth_config.json");
    std::fs::write(&config_path, serde_json::to_string_pretty(config).unwrap()).unwrap();

    let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;

    world.focuser_handle = Some(handle);
}

#[then("the Alpaca management endpoint should respond with valid credentials")]
async fn alpaca_management_responds_with_auth(world: &mut QhyFocuserWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world
        .focuser_handle
        .as_ref()
        .expect("qhy-focuser not started")
        .port;
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
async fn alpaca_rejects_wrong_credentials(world: &mut QhyFocuserWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world
        .focuser_handle
        .as_ref()
        .expect("qhy-focuser not started")
        .port;
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
async fn alpaca_rejects_missing_credentials(world: &mut QhyFocuserWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world
        .focuser_handle
        .as_ref()
        .expect("qhy-focuser not started")
        .port;
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
async fn response_includes_www_authenticate(world: &mut QhyFocuserWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world
        .focuser_handle
        .as_ref()
        .expect("qhy-focuser not started")
        .port;
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
async fn alpaca_responds_without_credentials(world: &mut QhyFocuserWorld) {
    let client = reqwest::Client::new();
    let port = world
        .focuser_handle
        .as_ref()
        .expect("qhy-focuser not started")
        .port;
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
