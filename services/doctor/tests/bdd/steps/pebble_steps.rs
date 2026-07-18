//! Steps for the `@pebble` scenarios: the real instant-acme order flow
//! against a per-scenario Pebble directory (tests/bdd/pebble.rs).

use cucumber::{given, then, when};
use serde_json::Value;

use crate::pebble::PebbleHandle;
use crate::world::DoctorWorld;

#[given(expr = "a local ACME directory issuing certificates valid for {int} days")]
async fn pebble_valid_days(world: &mut DoctorWorld, days: u64) {
    world.pebble = Some(PebbleHandle::start(days * 24 * 3600).await);
}

#[given(expr = "a local ACME directory issuing certificates valid for {int} hour")]
async fn pebble_valid_hours(world: &mut DoctorWorld, hours: u64) {
    world.pebble = Some(PebbleHandle::start(hours * 3600).await);
}

/// The full `tls issue --acme` invocation against the scenario's Pebble:
/// its directory URL, its minted CA as the ACME trust root, and the
/// challtestsrv management URL riding in `--dns-token`.
fn issue_acme_args(world: &DoctorWorld, domain: &str) -> Vec<String> {
    let pebble = world
        .pebble
        .as_ref()
        .expect("start the local ACME directory first");
    [
        "tls",
        "issue",
        "--acme",
        "--domain",
        domain,
        "--dns-provider",
        "challtestsrv",
        "--dns-token",
        &pebble.management_url,
        "--email",
        "ops@observatory.test",
        "--directory-url",
        &pebble.directory_url,
        "--acme-root",
        &pebble.ca_pem.to_string_lossy(),
        "--dns-propagation-seconds",
        "0",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn run_issue_acme(world: &mut DoctorWorld, domain: &str) {
    let args = issue_acme_args(world, domain);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    world.run_doctor_subcommand(&refs, None);
}

#[when(expr = "I run doctor tls issue --acme against the local directory for domain {string}")]
fn when_issue_acme(world: &mut DoctorWorld, domain: String) {
    run_issue_acme(world, &domain);
}

#[given(expr = "doctor tls issue --acme has already run against it for domain {string}")]
fn issue_acme_already_ran(world: &mut DoctorWorld, domain: String) {
    run_issue_acme(world, &domain);
    let output = world.output.as_ref().expect("tls issue --acme ran");
    assert!(
        output.status.success(),
        "priming tls issue --acme failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    world.snapshot_pki();
}

#[then(expr = "the certificate {string} covers {string}")]
fn certificate_covers(world: &mut DoctorWorld, name: String, san: String) {
    let path = world.pki_dir().join(&name);
    let pem = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let sans = doctor::provision::expiry::dns_sans(&pem);
    assert!(sans.contains(&san), "{name} SANs {sans:?} lack {san:?}");
}

fn acme_json(world: &DoctorWorld) -> Value {
    let path = world.config_dir().join("acme.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&content).expect("acme.json is valid JSON")
}

#[then(expr = "{string} records the local directory URL")]
fn acme_json_records_directory(world: &mut DoctorWorld, _name: String) {
    let expected = world
        .pebble
        .as_ref()
        .expect("pebble is running")
        .directory_url
        .clone();
    let acme = acme_json(world);
    assert_eq!(
        acme.get("directory_url"),
        Some(&Value::from(expected.as_str())),
        "acme.json: {acme}"
    );
}

fn amend_hooks(world: &DoctorWorld, hooks: Vec<String>) {
    let path = world.config_dir().join("acme.json");
    let mut value = acme_json(world);
    value["post_renewal_hooks"] = serde_json::json!(hooks);
    std::fs::write(&path, serde_json::to_string_pretty(&value).expect("json"))
        .unwrap_or_else(|e| panic!("amending {}: {e}", path.display()));
}

#[given("acme.json is amended with a post-renewal hook that writes a marker file")]
fn amend_marker_hook(world: &mut DoctorWorld) {
    let marker = world.temp.path().join("post-renewal-marker");
    // One command line, valid under both `sh -c` and `cmd /C`.
    let hook = format!("echo renewed > \"{}\"", marker.display());
    amend_hooks(world, vec![hook]);
    world.pebble_marker = Some(marker);
}

#[given("acme.json is amended with a post-renewal hook that fails")]
fn amend_failing_hook(world: &mut DoctorWorld) {
    amend_hooks(world, vec!["exit 1".to_string()]);
}

#[then("the post-renewal marker file exists")]
fn marker_file_exists(world: &mut DoctorWorld) {
    let marker = world.pebble_marker.as_ref().expect("a marker hook staged");
    assert!(
        marker.is_file(),
        "the post-renewal hook did not write {}",
        marker.display()
    );
}
