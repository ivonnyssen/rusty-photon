//! BDD step definitions for filemonitor HTTP Basic Auth

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::{FilemonitorWorld, ParsingRuleConfig};

const AUTH_USERNAME: &str = "observatory";
const AUTH_PASSWORD: &str = "test-password";

#[given(expr = "a monitored file containing {string}")]
fn monitored_file_containing(world: &mut FilemonitorWorld, content: String) {
    world.create_temp_file(&content);
}

#[given(
    expr = "filemonitor is configured with TLS and auth enabled and a contains rule {string} as safe"
)]
fn filemonitor_configured_with_tls_auth_and_rule(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRuleConfig {
        rule_type: "contains".to_string(),
        pattern,
        safe: true,
    });

    let hash = rp_auth::credentials::hash_password(AUTH_PASSWORD).unwrap();
    world.auth_password = Some(AUTH_PASSWORD.to_string());

    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let certs_dir = pki_dir.join("certs");

    let mut config = world.build_config_json();
    config["server"]["tls"] = serde_json::json!({
        "cert": certs_dir.join("filemonitor.pem").to_string_lossy().to_string(),
        "key": certs_dir.join("filemonitor-key.pem").to_string_lossy().to_string()
    });
    config["server"]["auth"] = serde_json::json!({
        "username": AUTH_USERNAME,
        "password_hash": hash
    });

    // Store config in temp dir for the When step to pick up
    let dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = dir.path().join("filemonitor_auth_config.json");
    std::fs::write(&config_path, config.to_string()).unwrap();
}

#[given(expr = "filemonitor is configured without auth and a contains rule {string} as safe")]
fn filemonitor_configured_without_auth_and_rule(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRuleConfig {
        rule_type: "contains".to_string(),
        pattern,
        safe: true,
    });

    let config = world.build_config_json();

    let dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = dir.path().join("filemonitor_noauth_config.json");
    std::fs::write(&config_path, config.to_string()).unwrap();
}

#[when("filemonitor is started with TLS and auth")]
async fn filemonitor_started_with_tls_and_auth(world: &mut FilemonitorWorld) {
    let dir = world.temp_dir.as_ref().expect("temp dir not created");
    let config_path = dir.path().join("filemonitor_auth_config.json");
    let content = std::fs::read_to_string(&config_path).expect("auth config not written");
    let config: serde_json::Value = serde_json::from_str(&content).unwrap();
    world.start_filemonitor_direct(&config).await;
}

#[when("filemonitor is started without auth")]
async fn filemonitor_started_without_auth(world: &mut FilemonitorWorld) {
    let dir = world.temp_dir.as_ref().expect("temp dir not created");
    let config_path = dir.path().join("filemonitor_noauth_config.json");
    let content = std::fs::read_to_string(&config_path).expect("noauth config not written");
    let config: serde_json::Value = serde_json::from_str(&content).unwrap();
    world.start_filemonitor_direct(&config).await;
}

#[then("the Alpaca management endpoint should respond with valid credentials")]
async fn alpaca_management_responds_with_auth(world: &mut FilemonitorWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.port.expect("filemonitor not started");
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
async fn alpaca_rejects_wrong_credentials(world: &mut FilemonitorWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.port.expect("filemonitor not started");
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
async fn alpaca_rejects_missing_credentials(world: &mut FilemonitorWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.port.expect("filemonitor not started");
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
async fn response_includes_www_authenticate(world: &mut FilemonitorWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.port.expect("filemonitor not started");
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
async fn alpaca_responds_without_credentials(world: &mut FilemonitorWorld) {
    let client = reqwest::Client::new();
    let port = world.port.expect("filemonitor not started");
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
