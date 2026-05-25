//! Step definitions for config_page.feature.

use cucumber::{given, then, when};
use serde_json::{json, Value};

use ui_htmx::{ApplyStatus, ConfigApplyResponse, ConfigClientError, ConfigGetResponse, FieldError};

use crate::world::UiWorld;

/// A complete dsd-fp2 config blob the way `config.get` would return it.
fn full_config(serial_port: &str, max_brightness: u64) -> Value {
    json!({
        "serial": {
            "port": serial_port,
            "baud_rate": 115_200,
            "polling_interval": "500ms",
            "timeout": "3s"
        },
        "server": { "port": 11119, "discovery_port": 32227, "tls": null, "auth": null },
        "cover_calibrator": {
            "name": "Deep Sky Dad FP2",
            "unique_id": "dsd-fp2-001",
            "description": "Deep Sky Dad Flat Panel 2",
            "enabled": true,
            "max_brightness": max_brightness
        }
    })
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    serde_urlencoded::to_string(pairs).expect("form encode")
}

#[given(regex = r#"^the dsd-fp2 driver reports serial\.port "([^"]+)" and max_brightness (\d+)$"#)]
fn driver_reports(world: &mut UiWorld, port: String, max_brightness: u64) {
    world.set_get(Ok(ConfigGetResponse {
        config: full_config(&port, max_brightness),
        overrides: vec![],
    }));
}

#[given(
    regex = r#"^the dsd-fp2 driver reports serial\.port "([^"]+)" pinned by a command-line override$"#
)]
fn driver_reports_pinned(world: &mut UiWorld, port: String) {
    world.set_get(Ok(ConfigGetResponse {
        config: full_config(&port, 4096),
        overrides: vec!["serial.port".to_string()],
    }));
}

#[given(
    regex = r#"^the dsd-fp2 driver accepts config\.apply with status applying reloading "([^"]+)"$"#
)]
fn driver_accepts_applying(world: &mut UiWorld, reload_path: String) {
    world.set_apply(Ok(ConfigApplyResponse {
        status: ApplyStatus::Applying,
        applied: vec![],
        reload: vec![reload_path],
        restart_required: vec![],
        skipped_override: vec![],
        persisted_to: Some("/tmp/dsd-fp2.json".to_string()),
        errors: vec![],
    }));
}

#[given("the dsd-fp2 driver rejects config.apply with an invalid serial.baud_rate")]
fn driver_rejects_invalid(world: &mut UiWorld) {
    world.set_apply(Ok(ConfigApplyResponse {
        status: ApplyStatus::Invalid,
        applied: vec![],
        reload: vec![],
        restart_required: vec![],
        skipped_override: vec![],
        persisted_to: None,
        errors: vec![FieldError {
            path: "serial.baud_rate".to_string(),
            msg: "must be greater than 0".to_string(),
        }],
    }));
}

#[given("the dsd-fp2 driver is unreachable")]
fn driver_unreachable(world: &mut UiWorld) {
    world.set_get(Err(ConfigClientError::Transport(
        "connection refused".to_string(),
    )));
}

#[when("I open the dsd-fp2 config page")]
async fn open_page(world: &mut UiWorld) {
    world.get("/config/dsd-fp2").await;
}

#[when(regex = r"^I submit the config form setting max_brightness to (\d+)$")]
async fn submit_max_brightness(world: &mut UiWorld, value: String) {
    let blob = full_config("/dev/ttyACM0", 4096).to_string();
    let body = form_body(&[
        ("__config", &blob),
        ("__overrides", "[]"),
        ("cover_calibrator.max_brightness", &value),
    ]);
    world.post_form("/config/dsd-fp2", body).await;
}

#[when(regex = r"^I submit the config form setting baud_rate to (\d+)$")]
async fn submit_baud_rate(world: &mut UiWorld, value: String) {
    let blob = full_config("/dev/ttyACM0", 4096).to_string();
    let body = form_body(&[
        ("__config", &blob),
        ("__overrides", "[]"),
        ("serial.baud_rate", &value),
    ]);
    world.post_form("/config/dsd-fp2", body).await;
}

#[then(regex = r#"^the page shows the value "([^"]+)"$"#)]
fn page_shows_value(world: &mut UiWorld, expected: String) {
    assert!(
        world.last_body.contains(&expected),
        "expected page to contain {expected:?}, body was:\n{}",
        world.last_body
    );
}

#[then("the serial.port field is disabled")]
fn serial_port_disabled(world: &mut UiWorld) {
    let tag = world.input_tag("serial.port");
    assert!(tag.contains("disabled"), "serial.port not disabled: {tag}");
}

#[then("the page explains the field is pinned by a command-line override")]
fn explains_pinned(world: &mut UiWorld) {
    assert!(
        world
            .last_body
            .contains("Pinned by a command-line override"),
        "missing pinned explanation:\n{}",
        world.last_body
    );
}

#[then("the page reports the driver is reloading")]
fn reports_reloading(world: &mut UiWorld) {
    assert!(
        world.last_body.contains("reloading") || world.last_body.contains("Reconnecting"),
        "missing reloading state:\n{}",
        world.last_body
    );
}

#[then("the page polls /config/dsd-fp2/status every 1s for reconnection")]
fn polls_for_reconnection(world: &mut UiWorld) {
    assert!(
        world
            .last_body
            .contains(r#"hx-get="/config/dsd-fp2/status""#),
        "missing poll target:\n{}",
        world.last_body
    );
    assert!(
        world.last_body.contains(r#"hx-trigger="every 1s""#),
        "missing poll trigger:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^the form shows the validation error "([^"]+)" on serial\.baud_rate$"#)]
fn shows_validation_error(world: &mut UiWorld, message: String) {
    assert!(
        world.last_body.contains(&message),
        "missing error message {message:?}:\n{}",
        world.last_body
    );
    let tag = world.input_tag("serial.baud_rate");
    // The field's wrapper carries the `invalid` class; the input is inside it.
    assert!(
        world.last_body.contains("invalid"),
        "baud_rate field not flagged invalid:\n{tag}"
    );
}

#[then(regex = r"^the submitted baud_rate value (\d+) is preserved$")]
fn baud_rate_preserved(world: &mut UiWorld, value: String) {
    let tag = world.input_tag("serial.baud_rate");
    assert!(
        tag.contains(&format!("value=\"{value}\"")),
        "submitted baud_rate not preserved: {tag}"
    );
}

#[then("the page shows a driver error")]
fn shows_driver_error(world: &mut UiWorld) {
    assert!(
        world.last_body.contains("could not reach the driver"),
        "missing driver error:\n{}",
        world.last_body
    );
}
