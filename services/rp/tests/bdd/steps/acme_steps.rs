//! BDD step definitions for ACME certificate setup

use cucumber::{then, when};
use tempfile::TempDir;

use crate::world::RpWorld;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run `rp init-tls` with the given arguments and capture the output.
fn run_init_tls_raw(args: &[&str]) -> std::process::Output {
    bdd_infra::run_once(env!("CARGO_PKG_NAME"), args, None)
}

// ---------------------------------------------------------------------------
// When steps — ACME flag validation
// ---------------------------------------------------------------------------

#[when("rp init-tls is run with --acme but no --domain")]
fn acme_without_domain(world: &mut RpWorld) {
    let output = run_init_tls_raw(&["init-tls", "--acme"]);
    world.last_command_output = Some(output);
}

#[when("rp init-tls is run with --acme --domain but no --dns-provider")]
fn acme_without_dns_provider(world: &mut RpWorld) {
    let output = run_init_tls_raw(&["init-tls", "--acme", "--domain", "test.example.com"]);
    world.last_command_output = Some(output);
}

#[when("rp init-tls is run with --acme --domain --dns-provider but no --email")]
fn acme_without_email(world: &mut RpWorld) {
    let output = run_init_tls_raw(&[
        "init-tls",
        "--acme",
        "--domain",
        "test.example.com",
        "--dns-provider",
        "cloudflare",
        "--dns-token",
        "fake-token",
    ]);
    world.last_command_output = Some(output);
}

#[when("rp init-tls is run with --acme and all required flags pointing to staging")]
fn acme_with_all_flags_staging(world: &mut RpWorld) {
    let dir = TempDir::new().unwrap();
    let output_dir = dir.path().to_str().unwrap().to_string();

    // This will fail at the DNS provider step (no real Cloudflare token),
    // but it should successfully save the acme.json config first.
    let output = run_init_tls_raw(&[
        "init-tls",
        "--acme",
        "--staging",
        "--domain",
        "observatory.example.com",
        "--dns-provider",
        "cloudflare",
        "--dns-token",
        "test-token-for-bdd",
        "--email",
        "test@example.com",
        "--output-dir",
        &output_dir,
    ]);

    world.last_command_output = Some(output);
    world.tls_pki_dir = Some(dir);
}

#[when("rp init-tls is run without --acme")]
fn init_tls_without_acme(world: &mut RpWorld) {
    let dir = TempDir::new().unwrap();
    let output_dir = dir.path().to_str().unwrap().to_string();
    let output = run_init_tls_raw(&["init-tls", "--output-dir", &output_dir]);
    world.last_command_output = Some(output);
    world.tls_pki_dir = Some(dir);
}

// ---------------------------------------------------------------------------
// Then steps — exit status and stderr
// ---------------------------------------------------------------------------

#[then("the command exits with a non-zero status")]
fn command_failed(world: &mut RpWorld) {
    let output = world
        .last_command_output
        .as_ref()
        .expect("no command output captured");
    assert!(
        !output.status.success(),
        "expected non-zero exit status but got success"
    );
}

#[then(expr = "stderr contains {string}")]
fn stderr_contains(world: &mut RpWorld, expected: String) {
    let output = world
        .last_command_output
        .as_ref()
        .expect("no command output captured");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_lowercase().contains(&expected.to_lowercase()),
        "stderr should contain '{expected}' but was: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Then steps — ACME config file validation
// ---------------------------------------------------------------------------

#[then("acme.json exists in the output directory")]
fn acme_json_exists(world: &mut RpWorld) {
    let dir = world
        .tls_pki_dir
        .as_ref()
        .expect("output directory not set");
    let acme_path = dir.path().join("acme.json");
    assert!(
        acme_path.exists(),
        "acme.json should exist at {}",
        acme_path.display()
    );
}

#[then("acme.json contains the provided domain")]
fn acme_json_has_domain(world: &mut RpWorld) {
    let config = load_acme_json(world);
    assert_eq!(
        config["domain"].as_str().unwrap(),
        "observatory.example.com"
    );
}

#[then("acme.json contains the provided email")]
fn acme_json_has_email(world: &mut RpWorld) {
    let config = load_acme_json(world);
    assert_eq!(config["email"].as_str().unwrap(), "test@example.com");
}

#[then("acme.json contains the DNS provider name")]
fn acme_json_has_dns_provider(world: &mut RpWorld) {
    let config = load_acme_json(world);
    assert_eq!(config["dns_provider"].as_str().unwrap(), "cloudflare");
}

#[then("acme.json has staging set to true")]
fn acme_json_staging_true(world: &mut RpWorld) {
    let config = load_acme_json(world);
    assert!(config["staging"].as_bool().unwrap());
}

// ---------------------------------------------------------------------------
// Then steps — self-signed CA fallback
// ---------------------------------------------------------------------------

#[then("ca.pem exists in the output directory")]
fn ca_pem_exists(world: &mut RpWorld) {
    let dir = world
        .tls_pki_dir
        .as_ref()
        .expect("output directory not set");
    assert!(dir.path().join("ca.pem").exists());
}

#[then("service certificates exist for default services")]
fn default_service_certs_exist(world: &mut RpWorld) {
    let dir = world
        .tls_pki_dir
        .as_ref()
        .expect("output directory not set");
    for svc in &[
        "filemonitor",
        "ppba-driver",
        "qhy-focuser",
        "rp",
        "sentinel",
    ] {
        assert!(
            dir.path().join("certs").join(format!("{svc}.pem")).exists(),
            "missing cert for {svc}"
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_acme_json(world: &RpWorld) -> serde_json::Value {
    let dir = world
        .tls_pki_dir
        .as_ref()
        .expect("output directory not set");
    let path = dir.path().join("acme.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    serde_json::from_str(&content).expect("acme.json should be valid JSON")
}
