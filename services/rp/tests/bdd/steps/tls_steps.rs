//! BDD step definitions for TLS certificate management and TLS connectivity

use std::net::SocketAddr;
use std::path::PathBuf;

use cucumber::{given, then, when};
use tempfile::TempDir;

use crate::steps::infrastructure::ServiceHandle;
use crate::world::RpWorld;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_init_tls(output_dir: &str, extra_args: &[&str]) {
    let binary = find_rp_binary();
    let mut cmd = std::process::Command::new(&binary);
    cmd.args(["init-tls", "--output-dir", output_dir]);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to run rp init-tls: {e}"));
    assert!(
        output.status.success(),
        "rp init-tls failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn find_rp_binary() -> String {
    if let Ok(path) = std::env::var("RP_BINARY") {
        return path;
    }
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .or_else(|_| std::env::var("CARGO_LLVM_COV_TARGET_DIR"))
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("target")
                .to_string_lossy()
                .to_string()
        });
    let binary = PathBuf::from(&target_dir).join("debug").join("rp");
    if binary.exists() {
        return binary.to_string_lossy().to_string();
    }
    panic!("Could not find rp binary. Set RP_BINARY env var or build first.");
}

fn pki_dir(world: &RpWorld) -> PathBuf {
    world
        .tls_pki_dir
        .as_ref()
        .expect("pki_dir not set — run init-tls first")
        .path()
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// init-tls steps
// ---------------------------------------------------------------------------

#[when("rp init-tls is executed with a temporary output directory")]
fn init_tls_default(world: &mut RpWorld) {
    let dir = TempDir::new().unwrap();
    run_init_tls(dir.path().to_str().unwrap(), &[]);
    world.tls_pki_dir = Some(dir);
}

#[given("rp init-tls has been executed once")]
fn init_tls_executed_once(world: &mut RpWorld) {
    let dir = TempDir::new().unwrap();
    run_init_tls(dir.path().to_str().unwrap(), &[]);
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).expect("ca.pem should exist");
    world.tls_ca_cert_pem = Some(ca_pem);
    world.tls_pki_dir = Some(dir);
}

#[given("rp init-tls has been executed")]
fn init_tls_executed(world: &mut RpWorld) {
    let dir = TempDir::new().unwrap();
    run_init_tls(dir.path().to_str().unwrap(), &[]);
    world.tls_pki_dir = Some(dir);
}

#[when("rp init-tls is executed again with the same output directory")]
fn init_tls_rerun(world: &mut RpWorld) {
    let path = pki_dir(world);
    run_init_tls(path.to_str().unwrap(), &[]);
}

#[when(expr = "rp init-tls is executed with services {string} and {string}")]
fn init_tls_with_services(world: &mut RpWorld, svc1: String, svc2: String) {
    let dir = TempDir::new().unwrap();
    run_init_tls(
        dir.path().to_str().unwrap(),
        &["--services", &svc1, "--services", &svc2],
    );
    world.tls_pki_dir = Some(dir);
}

#[then("the CA certificate should exist")]
fn ca_cert_exists(world: &mut RpWorld) {
    assert!(pki_dir(world).join("ca.pem").exists());
}

#[then("the CA private key should exist")]
fn ca_key_exists(world: &mut RpWorld) {
    assert!(pki_dir(world).join("ca-key.pem").exists());
}

#[then(expr = "certificates should exist for {string}")]
fn certs_exist_for(world: &mut RpWorld, service: String) {
    let dir = pki_dir(world);
    assert!(dir.join("certs").join(format!("{service}.pem")).exists());
    assert!(dir
        .join("certs")
        .join(format!("{service}-key.pem"))
        .exists());
}

#[then(expr = "certificates should not exist for {string}")]
fn certs_not_exist_for(world: &mut RpWorld, service: String) {
    let dir = pki_dir(world);
    assert!(!dir.join("certs").join(format!("{service}.pem")).exists());
}

#[then("the CA certificate should be unchanged")]
fn ca_unchanged(world: &mut RpWorld) {
    let current = std::fs::read_to_string(pki_dir(world).join("ca.pem")).unwrap();
    let original = world
        .tls_ca_cert_pem
        .as_ref()
        .expect("original CA cert not stored");
    assert_eq!(&current, original, "CA should be preserved on re-run");
}

// ---------------------------------------------------------------------------
// TLS roundtrip validation steps
// ---------------------------------------------------------------------------

#[when("a test HTTPS server is started with the rp certificate")]
async fn start_test_https_server(_world: &mut RpWorld) {
    // Combined with the next step
}

#[when("a client connects using the generated CA certificate")]
async fn client_connects_with_ca(world: &mut RpWorld) {
    let dir = pki_dir(world);
    let tls_config = rp_tls::config::TlsConfig {
        cert: dir.join("certs/rp.pem").to_string_lossy().into_owned(),
        key: dir.join("certs/rp-key.pem").to_string_lossy().into_owned(),
    };

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = rp_tls::server::bind_dual_stack_tokio(addr).await.unwrap();
    let bound_addr = listener.local_addr().unwrap();

    let router = axum::Router::new().route("/health", axum::routing::get(|| async { "ok" }));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        rp_tls::server::serve_tls(listener, router, &tls_config, async {
            shutdown_rx.await.ok();
        })
        .await
        .unwrap();
    });

    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let url = format!("https://localhost:{}/health", bound_addr.port());

    let response = client.get(&url).send().await.unwrap();
    world.tls_https_status = Some(response.status().as_u16());

    shutdown_tx.send(()).ok();
}

#[then("the HTTPS connection should succeed")]
fn https_connection_succeeded(world: &mut RpWorld) {
    let status = world.tls_https_status.expect("no HTTPS response captured");
    assert_eq!(status, 200, "HTTPS request should return 200 OK");
}

// ---------------------------------------------------------------------------
// rp TLS connectivity steps
// ---------------------------------------------------------------------------

#[given("generated TLS certificates")]
fn generate_tls_certs(world: &mut RpWorld) {
    let dir = TempDir::new().unwrap();
    rp_tls::cert::generate_ca(dir.path()).unwrap();
    let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
    let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
    let certs_dir = dir.path().join("certs");
    rp_tls::cert::generate_service_cert(&ca_pem, &ca_key, "rp", &[], &certs_dir).unwrap();
    world.tls_pki_dir = Some(dir);
}

#[given("rp is configured with TLS enabled")]
fn rp_configured_with_tls(_world: &mut RpWorld) {
    // Marker — config is built in the When step
}

#[when("rp is started")]
async fn rp_started_with_tls(world: &mut RpWorld) {
    let dir = pki_dir(world);
    let certs_dir = dir.join("certs");

    let mut config = world.build_config();
    config["server"]["tls"] = serde_json::json!({
        "cert": certs_dir.join("rp.pem").to_string_lossy().to_string(),
        "key": certs_dir.join("rp-key.pem").to_string_lossy().to_string()
    });

    let config_path = std::env::temp_dir()
        .join(format!(
            "rp-tls-test-{}.json",
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

    world.rp = Some(
        ServiceHandle::start(
            env!("CARGO_MANIFEST_DIR"),
            env!("CARGO_PKG_NAME"),
            &config_path,
        )
        .await,
    );

    // Wait for healthy over HTTPS
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.rp.as_ref().unwrap().port;
    let url = format!("https://localhost:{}/health", port);

    let mut healthy = false;
    for _ in 0..120 {
        if client.get(&url).send().await.is_ok() {
            healthy = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        healthy,
        "rp did not become healthy over HTTPS within timeout"
    );
}

#[then("the health endpoint should respond over HTTPS")]
async fn health_responds_https(world: &mut RpWorld) {
    let dir = pki_dir(world);
    let ca_path = dir.join("ca.pem");
    let client = rp_tls::client::build_reqwest_client(Some(&ca_path)).unwrap();
    let port = world.rp.as_ref().expect("rp not started").port;
    let url = format!("https://localhost:{}/health", port);

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "Hello World, I am healthy!");
}

#[when("rp is started without TLS")]
async fn rp_started_without_tls(world: &mut RpWorld) {
    let config = world.build_config();
    let config_path = std::env::temp_dir()
        .join(format!(
            "rp-notls-test-{}.json",
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

    world.rp = Some(
        ServiceHandle::start(
            env!("CARGO_MANIFEST_DIR"),
            env!("CARGO_PKG_NAME"),
            &config_path,
        )
        .await,
    );

    assert!(
        world.wait_for_rp_healthy().await,
        "rp did not become healthy within timeout"
    );
}

#[then("the health endpoint should respond over HTTP")]
async fn health_responds_http(world: &mut RpWorld) {
    let client = reqwest::Client::new();
    let url = format!("{}/health", world.rp_url());

    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "Hello World, I am healthy!");
}
