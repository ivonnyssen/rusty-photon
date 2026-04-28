//! BDD step definitions for sentinel TLS connectivity

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::SentinelWorld;

fn pki_dir(world: &SentinelWorld) -> std::path::PathBuf {
    world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf()
}

#[given("generated TLS certificates for sentinel")]
fn generate_tls_certs(world: &mut SentinelWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "sentinel", &[], &certs_dir).unwrap();
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "filemonitor", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("sentinel is configured with dashboard TLS enabled")]
fn sentinel_configured_dashboard_tls(_world: &mut SentinelWorld) {
    // Marker — config is built in When step
}

#[when("sentinel is started")]
async fn sentinel_started_with_tls(world: &mut SentinelWorld) {
    let dir = pki_dir(world);
    let certs_dir = dir.join("certs");

    let mut config = world.build_sentinel_config();
    config["dashboard"]["tls"] = serde_json::json!({
        "cert": certs_dir.join("sentinel.pem").to_string_lossy().to_string(),
        "key": certs_dir.join("sentinel-key.pem").to_string_lossy().to_string()
    });

    let temp_dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = temp_dir.path().join("sentinel_tls_config.json");
    std::fs::write(&config_path, config.to_string()).unwrap();

    let handle =
        bdd_infra::ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap())
            .await;

    world.sentinel = Some(handle);
}

#[then("the dashboard health endpoint should respond over HTTPS")]
async fn dashboard_health_https(world: &mut SentinelWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.sentinel.as_ref().expect("sentinel not started").port;
    let url = format!("https://localhost:{}/health", port);

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
    assert!(ok, "dashboard did not respond over HTTPS within timeout");
}

// --- Cross-service TLS scenario ---

#[given(expr = "filemonitor is running with TLS enabled and a contains rule {string} as safe")]
async fn filemonitor_running_with_tls(world: &mut SentinelWorld, pattern: String) {
    let dir = pki_dir(world);
    let certs_dir = dir.join("certs");

    world.fm_rules.push(serde_json::json!({
        "type": "contains",
        "pattern": pattern,
        "safe": true
    }));

    let mut config = world.build_filemonitor_config();
    config["server"]["tls"] = serde_json::json!({
        "cert": certs_dir.join("filemonitor.pem").to_string_lossy().to_string(),
        "key": certs_dir.join("filemonitor-key.pem").to_string_lossy().to_string()
    });

    let temp_dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = temp_dir.path().join("filemonitor_tls_config.json");
    std::fs::write(&config_path, config.to_string()).unwrap();

    let handle =
        bdd_infra::ServiceHandle::start("filemonitor", config_path.to_str().unwrap()).await;

    world.filemonitor = Some(handle);
}

#[given("sentinel is configured with CA certificate and HTTPS monitor scheme")]
fn sentinel_configured_with_ca(world: &mut SentinelWorld) {
    let dir = pki_dir(world);
    let fm = world.filemonitor.as_ref().expect("filemonitor not started");

    world.sentinel_monitors.push(serde_json::json!({
        "type": "alpaca_safety_monitor",
        "name": "Roof Monitor",
        "host": "localhost",
        "port": fm.port,
        "device_number": 0,
        "polling_interval_secs": 1,
        "scheme": "https"
    }));

    // Store CA path — will be added to sentinel config in start step
    world.sentinel_monitor_name = "Roof Monitor".to_string();

    // The sentinel config needs ca_cert
    // We'll add it when building the config in the "sentinel is running" step
    // Store the dir path for later use — it's already in tls_pki_dir
    let _ = dir;
}

#[given("sentinel is running with CA trust")]
async fn sentinel_running_with_ca(world: &mut SentinelWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");

    let mut config = world.build_sentinel_config();
    config["ca_cert"] = serde_json::json!(ca_path.to_string_lossy().to_string());

    let temp_dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = temp_dir.path().join("sentinel_ca_config.json");
    std::fs::write(&config_path, config.to_string()).unwrap();

    let handle =
        bdd_infra::ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap())
            .await;

    world.sentinel = Some(handle);
}
