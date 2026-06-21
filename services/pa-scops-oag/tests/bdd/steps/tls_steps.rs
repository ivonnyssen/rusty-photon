//! BDD step definitions for pa-scops-oag TLS connectivity

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::world::ScopsWorld;
use bdd_infra::ServiceHandle;

#[given("generated TLS certificates for pa-scops-oag")]
fn generate_tls_certs(world: &mut ScopsWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "pa-scops-oag", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("pa-scops-oag is configured with TLS enabled and mock serial")]
fn configured_with_tls(world: &mut ScopsWorld) {
    let certs_dir = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("certs");

    world.config = Some(pa_scops_oag::Config {
        serial: pa_scops_oag::SerialConfig {
            port: "/dev/mock".to_string(),
            polling_interval: std::time::Duration::from_secs(60),
            ..Default::default()
        },
        server: pa_scops_oag::ServerConfig {
            port: 0,
            discovery_port: None,
            tls: Some(rp_tls::config::TlsConfig {
                cert: certs_dir
                    .join("pa-scops-oag.pem")
                    .to_string_lossy()
                    .into_owned(),
                key: certs_dir
                    .join("pa-scops-oag-key.pem")
                    .to_string_lossy()
                    .into_owned(),
            }),
            auth: None,
        },
        focuser: pa_scops_oag::FocuserConfig {
            enabled: true,
            ..Default::default()
        },
    });
}

#[when("pa-scops-oag is started with TLS")]
async fn started_with_tls(world: &mut ScopsWorld) {
    let config = world.config.as_ref().expect("config not set");
    let dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let config_path = dir.path().join("scops_tls_config.json");
    std::fs::write(&config_path, serde_json::to_string_pretty(config).unwrap()).unwrap();

    let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;
    world.focuser_handle = Some(handle);
}

#[then("the Alpaca management endpoint should respond over HTTPS")]
async fn management_responds_https(world: &mut ScopsWorld) {
    let ca_path = world
        .tls_pki_dir
        .as_ref()
        .expect("TLS certs not generated")
        .path()
        .join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world
        .focuser_handle
        .as_ref()
        .expect("pa-scops-oag not started")
        .port;
    let url = format!("https://localhost:{port}/management/v1/configureddevices");

    for _ in 0..60 {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().as_u16() == 200 {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("Alpaca management endpoint did not respond over HTTPS");
}
