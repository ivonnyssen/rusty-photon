//! Steps for the aggregation scenarios (docs/services/doctor.md
//! §Aggregation): stub management endpoints for the active-unit HTTP probe
//! and stub per-service binaries for the inactive-unit shell-out probe.

use std::path::{Path, PathBuf};

use axum::http::{HeaderMap, StatusCode};
use cucumber::given;

use crate::world::DoctorWorld;

/// The plaintext the "staged observatory credential" given writes to
/// `pki/credential`, and the Basic header the authenticated stub demands.
const STUB_PASSWORD: &str = "stub-password";
/// `base64("observatory:stub-password")` — what reqwest's `basic_auth`
/// produces for the staged credential.
const STUB_BASIC_HEADER: &str = "Basic b2JzZXJ2YXRvcnk6c3R1Yi1wYXNzd29yZA==";

const DEVICES_JSON: &str = r#"{ "Value": [
    { "DeviceName": "Stub Camera", "DeviceType": "Camera", "DeviceNumber": 0 },
    { "DeviceName": "Stub Wheel", "DeviceType": "FilterWheel", "DeviceNumber": 1 }
] }"#;

async fn management_response(require_auth: bool, headers: HeaderMap) -> (StatusCode, String) {
    if require_auth {
        let authorized = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == STUB_BASIC_HEADER);
        if !authorized {
            return (StatusCode::UNAUTHORIZED, String::new());
        }
    }
    (StatusCode::OK, DEVICES_JSON.to_string())
}

fn stub_router(require_auth: bool) -> axum::Router {
    axum::Router::new().route(
        "/management/v1/configureddevices",
        axum::routing::get(move |headers: HeaderMap| management_response(require_auth, headers)),
    )
}

/// Start a plain-HTTP stub management endpoint; the bound port lands in
/// `world.stub_port` for the config-staging steps.
async fn start_http_stub(world: &mut DoctorWorld, require_auth: bool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("stub endpoint bind");
    world.stub_port = Some(listener.local_addr().expect("stub addr").port());
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    world.stub_shutdowns.push(shutdown_tx);
    let router = stub_router(require_auth);
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                shutdown_rx.await.ok();
            })
            .await
            .expect("stub endpoint serve");
    });
}

#[given("a stub management endpoint serving two configured devices")]
async fn stub_endpoint(world: &mut DoctorWorld) {
    start_http_stub(world, false).await;
}

#[given("a stub management endpoint that requires authentication")]
async fn stub_endpoint_authenticated(world: &mut DoctorWorld) {
    start_http_stub(world, true).await;
}

/// An HTTPS stub serving the pki tree's issued pair for the service — the
/// same trust chain a provisioned rig runs, so the probe must present
/// doctor's CA as its root. Also (re)writes the service's config: the tls
/// block pointing at the issued pair, the port at the stub.
#[given(expr = "an HTTPS stub management endpoint for {string} serving two configured devices")]
async fn stub_endpoint_https(world: &mut DoctorWorld, service: String) {
    let pki = world.pki_dir();
    let cert = pki.join(format!("{service}.pem"));
    let key = pki.join(format!("{service}-key.pem"));
    assert!(
        cert.is_file() && key.is_file(),
        "no issued pair for {service} — missing the `doctor tls issue` given?"
    );
    let tls_config = rusty_photon_tls::config::TlsConfig {
        cert: cert.to_string_lossy().into_owned(),
        key: key.to_string_lossy().into_owned(),
    };

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().expect("stub addr literal");
    let listener = rusty_photon_tls::server::bind_dual_stack_tokio(addr)
        .await
        .expect("stub endpoint bind");
    let port = listener.local_addr().expect("stub addr").port();
    world.stub_port = Some(port);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    world.stub_shutdowns.push(shutdown_tx);
    let router = stub_router(false);
    tokio::spawn(async move {
        rusty_photon_tls::server::serve_tls(listener, router, &tls_config, async {
            shutdown_rx.await.ok();
        })
        .await
        .expect("stub endpoint serve");
    });

    let config = serde_json::json!({ "server": {
        "port": port,
        "tls": { "cert": cert.to_string_lossy(), "key": key.to_string_lossy() },
    } });
    world.write_config(&format!("{service}.json"), &config.to_string());
}

#[given(expr = "a config file {string} pointing at the stub endpoint")]
fn config_at_stub(world: &mut DoctorWorld, file: String) {
    let port = world.stub_port.expect("no stub endpoint started yet");
    world.write_config(&file, &format!(r#"{{ "server": {{ "port": {port} }} }}"#));
}

#[given(expr = "a config file {string} pointing at the stub endpoint with auth enabled")]
fn config_at_stub_with_auth(world: &mut DoctorWorld, file: String) {
    let port = world.stub_port.expect("no stub endpoint started yet");
    // The hash is never verified by the probe (the stub checks the
    // plaintext header) — the block's presence is what routes the credential.
    world.write_config(
        &file,
        &format!(
            r#"{{ "server": {{ "port": {port},
                 "auth": {{ "username": "observatory", "password_hash": "$argon2id$stub" }} }} }}"#
        ),
    );
}

#[given(expr = "a config file {string} declaring a port nothing listens on")]
async fn config_at_dead_port(world: &mut DoctorWorld, file: String) {
    // Bind-then-drop: the freed port is as close to guaranteed-closed as a
    // test can stage without racing other suites for a fixed number.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("probe port bind");
    let port = listener.local_addr().expect("probe addr").port();
    drop(listener);
    world.write_config(&file, &format!(r#"{{ "server": {{ "port": {port} }} }}"#));
}

#[given("a staged observatory credential")]
fn staged_credential(world: &mut DoctorWorld) {
    let pki = world.pki_dir();
    std::fs::create_dir_all(&pki).expect("pki dir");
    std::fs::write(pki.join("credential"), format!("{STUB_PASSWORD}\n")).expect("credential file");
}

// ---------------------------------------------------------------------------
// Stub per-service binaries for the shell-out probe
// ---------------------------------------------------------------------------

/// Write an executable stub script. Windows gets a `.cmd` (the SCM-recorded
/// image is an `.exe`, but the spawn path exercised is the same); elsewhere
/// a `chmod +x` shell script.
fn write_stub_script(dir: &Path, name: &str, unix_body: &str, windows_body: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = unix_body;
        let path = dir.join(format!("{name}.cmd"));
        std::fs::write(&path, windows_body).expect("stub script");
        path
    }
    #[cfg(not(windows))]
    {
        let _ = windows_body;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, unix_body).expect("stub script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("stub script mode");
        path
    }
}

#[given(expr = "a stub per-service doctor for {string} whose report has a failing {string} check")]
fn stub_doctor_binary(world: &mut DoctorWorld, service: String, check: String) {
    let report = serde_json::json!({
        "schema_version": 1,
        "doctor_version": "9.9.9",
        "mode": "service",
        "service": service,
        "config_dir": format!("/etc/rusty-photon/{service}.json"),
        "checks": [ {
            "name": check,
            "status": "fail",
            "detail": "unknown field `typo_key` at line 3",
        } ],
    });
    let report_file = format!("{service}-doctor-report.json");
    std::fs::write(
        world.temp.path().join(&report_file),
        serde_json::to_string(&report).expect("stub report"),
    )
    .expect("stub report file");
    world.stub_binary = Some(write_stub_script(
        world.temp.path(),
        &format!("stub-{service}"),
        &format!("#!/bin/sh\ncat \"$(dirname \"$0\")/{report_file}\"\n"),
        &format!("@echo off\r\ntype \"%~dp0{report_file}\"\r\n"),
    ));
}

#[given(expr = "a stub per-service binary for {string} that does not know the doctor subcommand")]
fn stub_predates_subcommand(world: &mut DoctorWorld, service: String) {
    world.stub_binary = Some(write_stub_script(
        world.temp.path(),
        &format!("stub-{service}"),
        "#!/bin/sh\necho \"error: unrecognized subcommand 'doctor'\" >&2\nexit 2\n",
        "@echo off\r\necho error: unrecognized subcommand 'doctor' 1>&2\r\nexit /b 2\r\n",
    ));
}

// ---------------------------------------------------------------------------
// Unit run-state staging
// ---------------------------------------------------------------------------

#[given(expr = "platform facts where unit {string} is installed and active")]
fn unit_active(world: &mut DoctorWorld, unit: String) {
    world.add_unit(&unit);
    world.set_unit_probe_facts(&unit, true, None);
}

#[given(expr = "platform facts where unit {string} is installed but stopped, with the stub binary")]
fn unit_stopped_with_stub(world: &mut DoctorWorld, unit: String) {
    let binary = world.stub_binary.clone().expect("no stub binary staged");
    world.add_unit(&unit);
    world.set_unit_probe_facts(&unit, false, Some(binary));
}

#[given(
    expr = "platform facts where unit {string} is installed but stopped, with no known binary path"
)]
fn unit_stopped_without_binary(world: &mut DoctorWorld, unit: String) {
    world.add_unit(&unit);
    world.set_unit_probe_facts(&unit, false, None);
}
