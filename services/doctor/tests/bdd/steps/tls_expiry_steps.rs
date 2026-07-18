//! Steps for the tls.expiry diagnosis: configs pointing at staged pki
//! pairs and report assertions in the feature's phrasing.

use cucumber::{given, then, when};
use serde_json::json;

use crate::world::DoctorWorld;

#[given(expr = "a config file {string} with a tls block pointing at the {string} pair")]
fn config_with_pki_tls_block(world: &mut DoctorWorld, name: String, service: String) {
    let config_service = name
        .strip_suffix(".json")
        .unwrap_or_else(|| panic!("not a config file name: {name}"));
    let port = doctor::catalog::entry(config_service)
        .unwrap_or_else(|| panic!("{config_service} is not in the catalog"))
        .default_port;
    let pki = world.pki_dir();
    let content = json!({
        "server": {
            "port": port,
            "tls": {
                "cert": pki.join(format!("{service}.pem")).to_string_lossy(),
                "key": pki.join(format!("{service}-key.pem")).to_string_lossy(),
            }
        }
    });
    world.write_config(
        &name,
        &serde_json::to_string_pretty(&content).expect("json"),
    );
}

#[given(expr = "the pki file {string} is overwritten with garbage")]
fn pki_file_garbage(world: &mut DoctorWorld, name: String) {
    let path = world.pki_dir().join(&name);
    std::fs::write(&path, "this is not a certificate\n")
        .unwrap_or_else(|e| panic!("overwriting {}: {e}", path.display()));
}

#[when("I run doctor")]
fn run_doctor(world: &mut DoctorWorld) {
    // --json under the hood so the report assertions have a subject; the
    // exit code is the same either way.
    world.run_doctor(true);
}

#[then(expr = "the report has a/an {string} check named {string} for service {string}")]
fn report_has_check(world: &mut DoctorWorld, status: String, name: String, service: String) {
    crate::steps::report_steps::find_check(world, &status, &name, Some(&service));
}

#[then(expr = "the {string} suggestion mentions {string}")]
fn named_check_suggestion_mentions(world: &mut DoctorWorld, check_name: String, needle: String) {
    let check = world
        .checks()
        .iter()
        .find(|c| c["name"] == check_name.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("no check named {check_name} in the report"));
    let suggestion = check["suggestion"]
        .as_str()
        .unwrap_or_else(|| panic!("{check_name} has no suggestion: {check}"));
    assert!(
        suggestion.contains(&needle),
        "{check_name} suggestion {suggestion:?} lacks {needle:?}"
    );
}
