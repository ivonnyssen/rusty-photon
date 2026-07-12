//! Step definitions for config_actions.feature.
//!
//! Reuses the `Given a running focuser service` step from connection_steps.

use std::sync::Arc;

use ascom_alpaca::ASCOMErrorCode;
use cucumber::{given, then, when};

use crate::world::QhyFocuserWorld;

#[given(regex = r"^a running focuser service configured with serial\.port (\S+)$")]
async fn running_focuser_service_with_port(world: &mut QhyFocuserWorld, port: String) {
    // Pin the serial port so the scenario's config.get round-trip is
    // deterministic on every platform (the built-in default is
    // platform-dependent: a /dev path on Unix, COM3 on Windows).
    let mut config = qhy_focuser::Config::default();
    config.serial.port = port;
    world.config = Some(config);
    world.start_focuser().await;
}

#[when("the supported actions are queried")]
async fn query_supported_actions(world: &mut QhyFocuserWorld) {
    let focuser = Arc::clone(world.focuser());
    world.last_supported_actions = Some(focuser.supported_actions().await.unwrap());
}

#[when("config.get is called")]
async fn call_config_get(world: &mut QhyFocuserWorld) {
    world.current_config().await;
}

#[when("config.schema is called")]
async fn call_config_schema(world: &mut QhyFocuserWorld) {
    let focuser = Arc::clone(world.focuser());
    let body = focuser
        .action("config.schema".to_string(), String::new())
        .await
        .expect("config.schema failed");
    world.last_response =
        Some(serde_json::from_str(&body).expect("config.schema returned invalid JSON"));
}

#[when(regex = r"^config\.apply sets max_step to (\d+)$")]
async fn apply_max_step(world: &mut QhyFocuserWorld, value: u32) {
    let mut config = world.current_config().await;
    config["focuser"]["max_step"] = serde_json::json!(value);
    world.call_config_apply(config).await;
}

#[when(regex = r"^config\.apply pins the bound port and sets max_step to (\d+)$")]
async fn apply_pin_port_and_max_step(world: &mut QhyFocuserWorld, value: u32) {
    // Pin the OS-assigned bound port into the file so the reload rebinds the
    // *same* port, letting this client reach the reloaded server.
    let port = world.bound_port();
    let mut config = world.current_config().await;
    config["server"]["port"] = serde_json::json!(port);
    config["focuser"]["max_step"] = serde_json::json!(value);
    world.call_config_apply(config).await;
}

#[when("config.apply is called with an invalid baud_rate")]
async fn apply_invalid_baud_rate(world: &mut QhyFocuserWorld) {
    let mut config = world.current_config().await;
    config["serial"]["baud_rate"] = serde_json::json!(0);
    world.call_config_apply(config).await;
}

#[when(regex = r#"^the action "([^"]+)" is called$"#)]
async fn call_named_action(world: &mut QhyFocuserWorld, name: String) {
    let focuser = Arc::clone(world.focuser());
    match focuser.action(name, String::new()).await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[then("the supported actions should include config.get, config.apply, and config.schema")]
async fn assert_supported_actions(world: &mut QhyFocuserWorld) {
    let actions = world
        .last_supported_actions
        .as_ref()
        .expect("no supported actions queried");
    for expected in ["config.get", "config.apply", "config.schema"] {
        assert!(
            actions.iter().any(|a| a == expected),
            "{expected} missing from {actions:?}"
        );
    }
}

#[then("the schema should describe the serial, server, and focuser sections")]
async fn assert_schema_sections(world: &mut QhyFocuserWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let props = response["schema"]["properties"]
        .as_object()
        .expect("schema.properties is not an object");
    for section in ["serial", "server", "focuser"] {
        assert!(
            props.contains_key(section),
            "schema is missing section {section}: {props:?}"
        );
    }
}

#[then("the schema should mark focuser.unique_id as a locked field")]
async fn assert_schema_locked_field(world: &mut QhyFocuserWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let locked = response["locked_fields"]
        .as_array()
        .expect("`locked_fields` is not an array");
    assert!(
        locked
            .iter()
            .any(|f| f.as_str() == Some("focuser.unique_id")),
        "locked_fields {locked:?} does not include focuser.unique_id"
    );
}

#[then("the schema should mark server.port as a read-only field")]
async fn assert_schema_read_only_field(world: &mut QhyFocuserWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let read_only = response["read_only_fields"]
        .as_array()
        .expect("`read_only_fields` is not an array");
    assert!(
        read_only.iter().any(|f| f.as_str() == Some("server.port")),
        "read_only_fields {read_only:?} does not include server.port"
    );
}

#[then(regex = r"^the config should report serial\.port as (\S+)$")]
async fn assert_config_serial_port(world: &mut QhyFocuserWorld, expected: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    assert_eq!(
        response["config"]["serial"]["port"].as_str(),
        Some(expected.as_str())
    );
}

#[then("the config should report no overrides")]
async fn assert_no_overrides(world: &mut QhyFocuserWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let overrides = response["overrides"]
        .as_array()
        .expect("`overrides` is not an array");
    assert!(
        overrides.is_empty(),
        "expected no overrides, got {overrides:?}"
    );
}

#[then(regex = r"^the apply status should be (\w+)$")]
async fn assert_apply_status(world: &mut QhyFocuserWorld, expected: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    assert_eq!(response["status"].as_str(), Some(expected.as_str()));
}

#[then(regex = r"^the reload list should include (\S+)$")]
async fn assert_reload_includes(world: &mut QhyFocuserWorld, path: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let reload = response["reload"]
        .as_array()
        .expect("`reload` is not an array");
    assert!(
        reload.iter().any(|p| p.as_str() == Some(path.as_str())),
        "reload list {reload:?} does not include {path}"
    );
}

#[then("the response should contain validation errors")]
async fn assert_validation_errors(world: &mut QhyFocuserWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let errors = response["errors"]
        .as_array()
        .expect("`errors` is not an array");
    assert!(!errors.is_empty(), "expected validation errors, got none");
}

#[then(regex = r"^the reloaded service serves max_step (\d+)$")]
async fn assert_reloaded_serves(world: &mut QhyFocuserWorld, expected: u32) {
    world.wait_for_config_max_step(expected).await;
}

#[then("the call should fail with an action-not-implemented error")]
async fn assert_action_not_implemented(world: &mut QhyFocuserWorld) {
    assert_eq!(
        world.last_error_code,
        Some(ASCOMErrorCode::ACTION_NOT_IMPLEMENTED.raw()),
        "expected ACTION_NOT_IMPLEMENTED, got {:?} ({:?})",
        world.last_error_code,
        world.last_error
    );
}
