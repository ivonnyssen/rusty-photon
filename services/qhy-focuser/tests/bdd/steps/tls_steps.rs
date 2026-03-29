//! BDD step definitions for qhy-focuser TLS connectivity

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::QhyFocuserWorld;
use bdd_infra::ServiceHandle;

#[given("generated TLS certificates for qhy-focuser")]
fn generate_tls_certs(world: &mut QhyFocuserWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "qhy-focuser", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("qhy-focuser is configured with TLS enabled and mock serial")]
fn qhy_configured_with_tls(world: &mut QhyFocuserWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let certs_dir = pki_dir.join("certs");

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
}

#[when("qhy-focuser is started with TLS")]
async fn qhy_started_with_tls(world: &mut QhyFocuserWorld) {
    let config = world.config.as_ref().expect("config not set");
    let dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = dir.path().join("qhy_tls_config.json");
    std::fs::write(&config_path, serde_json::to_string_pretty(config).unwrap()).unwrap();

    let handle = ServiceHandle::start(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        config_path.to_str().unwrap(),
    )
    .await;

    world.focuser_handle = Some(handle);
}

#[then("the Alpaca management endpoint should respond over HTTPS")]
async fn alpaca_management_responds_https(world: &mut QhyFocuserWorld) {
    let pki_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .to_path_buf();
    let ca_path = pki_dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world
        .focuser_handle
        .as_ref()
        .expect("qhy-focuser not started")
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
