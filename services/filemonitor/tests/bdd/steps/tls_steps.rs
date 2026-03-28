//! BDD step definitions for filemonitor TLS connectivity

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::steps::infrastructure::ServiceHandle;
use crate::world::FilemonitorWorld;

#[given("generated TLS certificates for filemonitor")]
fn generate_tls_certs(world: &mut FilemonitorWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "filemonitor", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("filemonitor is configured with TLS enabled")]
fn filemonitor_configured_with_tls(_world: &mut FilemonitorWorld) {
    // Marker — config is built in the When step
}

#[when("filemonitor is started with TLS")]
async fn filemonitor_started_with_tls(world: &mut FilemonitorWorld) {
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

    let dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = dir.path().join("filemonitor_tls_config.json");
    std::fs::write(&config_path, config.to_string()).unwrap();

    let handle = ServiceHandle::start(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        config_path.to_str().unwrap(),
    )
    .await;

    world.filemonitor = Some(handle);
}

#[then("the Alpaca management endpoint should respond over HTTPS")]
async fn alpaca_management_responds_https(world: &mut FilemonitorWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world
        .filemonitor
        .as_ref()
        .expect("filemonitor not started")
        .port;
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
