//! BDD step definitions for ppba-driver TLS connectivity

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::steps::infrastructure::ServiceHandle;
use crate::world::PpbaWorld;

#[given("generated TLS certificates for ppba-driver")]
fn generate_tls_certs(world: &mut PpbaWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "ppba-driver", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("ppba-driver is configured with TLS enabled and mock serial")]
fn ppba_configured_with_tls(world: &mut PpbaWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let certs_dir = pki_dir.join("certs");

    world.config = serde_json::json!({
        "serial": { "port": "/dev/mock", "baud_rate": 9600, "polling_interval": "60s", "timeout": "2s" },
        "server": {
            "port": 0,
            "tls": {
                "cert": certs_dir.join("ppba-driver.pem").to_string_lossy().to_string(),
                "key": certs_dir.join("ppba-driver-key.pem").to_string_lossy().to_string()
            }
        },
        "switch": { "name": "Test Switch", "unique_id": "test-switch", "description": "Test", "enabled": true },
        "observingconditions": { "name": "Test OC", "unique_id": "test-oc", "description": "Test", "enabled": false }
    });
}

#[when("ppba-driver is started with TLS")]
async fn ppba_started_with_tls(world: &mut PpbaWorld) {
    let config_path = std::env::temp_dir()
        .join(format!(
            "ppba-tls-test-{}.json",
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

#[then("the Alpaca management endpoint should respond over HTTPS")]
async fn alpaca_management_responds_https(world: &mut PpbaWorld) {
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
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().as_u16() == 200 {
                ok = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(ok, "Alpaca management endpoint did not respond over HTTPS");
}
