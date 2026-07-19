//! Assertions over --fix: what was applied, what was written, and what a
//! read-only run left alone.

use cucumber::then;
use serde_json::Value;

use crate::world::DoctorWorld;

fn fixes_applied(world: &DoctorWorld) -> Vec<Value> {
    world
        .report()
        .get("fixes_applied")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default()
}

#[then(expr = "the report records an applied fix for check {string} on service {string}")]
fn records_applied_fix(world: &mut DoctorWorld, check: String, service: String) {
    let applied = fixes_applied(world);
    assert!(
        applied
            .iter()
            .any(|f| f["check"] == check.as_str() && f["op"]["service"] == service.as_str()),
        "no applied fix for {check} on {service} in: {applied:?}"
    );
}

#[then("the report records no applied fixes")]
fn records_no_applied_fixes(world: &mut DoctorWorld) {
    let applied = fixes_applied(world);
    assert!(applied.is_empty(), "unexpected applied fixes: {applied:?}");
}

#[then(expr = "the config file {string} has the number {int} at {string}")]
fn config_has_number(world: &mut DoctorWorld, name: String, expected: i64, pointer: String) {
    let value = world.config_value(&name);
    assert_eq!(
        value.pointer(&pointer),
        Some(&Value::from(expected)),
        "{name} at {pointer}: {value}"
    );
}

#[then(expr = "the config file {string} has the string {string} at {string}")]
fn config_has_string(world: &mut DoctorWorld, name: String, expected: String, pointer: String) {
    let expected = world.expand(&expected);
    let value = world.config_value(&name);
    assert_eq!(
        value.pointer(&pointer),
        Some(&Value::from(expected.as_str())),
        "{name} at {pointer}: {value}"
    );
}

#[then(expr = "the config file {string} has JSON true at {string}")]
fn config_has_true(world: &mut DoctorWorld, name: String, pointer: String) {
    let value = world.config_value(&name);
    assert_eq!(
        value.pointer(&pointer),
        Some(&Value::Bool(true)),
        "{name} at {pointer}: {value}"
    );
}

#[then(expr = "the config file {string} has no value at {string}")]
fn config_has_no_value(world: &mut DoctorWorld, name: String, pointer: String) {
    let value = world.config_value(&name);
    assert_eq!(
        value.pointer(&pointer),
        None,
        "{name} still has a value at {pointer}: {value}"
    );
}

#[then(expr = "the config file {string} is unchanged from what was staged")]
fn config_unchanged(world: &mut DoctorWorld, name: String) {
    let staged = world
        .staged
        .get(&name)
        .unwrap_or_else(|| panic!("{name} was never staged"));
    let on_disk = std::fs::read_to_string(world.config_dir().join(&name))
        .unwrap_or_else(|e| panic!("reading {name}: {e}"));
    assert_eq!(&on_disk, staged, "{name} was rewritten");
}

#[then(expr = "that check's fix plan removes the key at {string}")]
fn check_fix_plan_removes(world: &mut DoctorWorld, pointer: String) {
    let check = world.last_check.as_ref().expect("no check matched yet");
    let fixes = check["fixes"].as_array().cloned().unwrap_or_default();
    assert!(
        fixes
            .iter()
            .any(|f| f["op"] == "remove-key" && f["pointer"] == pointer.as_str()),
        "no remove-key fix at {pointer} in: {fixes:?}"
    );
}
