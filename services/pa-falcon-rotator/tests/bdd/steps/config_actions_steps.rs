//! Step definitions for config_actions.feature.
//!
//! Reuses the `Given a running pa-falcon-rotator service` step from
//! connection_steps. The in-process server is built with the config-action
//! context wired (see `world::start_service`), so both devices dispatch the
//! actions against one config file + reload signal.

use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

use crate::world::FalconRotatorWorld;

#[when("the supported actions are queried on the rotator device")]
async fn query_supported_actions_rotator(world: &mut FalconRotatorWorld) {
    let rotator = Arc::clone(world.rotator());
    world.last_supported_actions = Some(rotator.supported_actions().await.unwrap());
}

#[when("the supported actions are queried on the switch device")]
async fn query_supported_actions_switch(world: &mut FalconRotatorWorld) {
    let switch = Arc::clone(world.status_switch());
    world.last_supported_actions = Some(switch.supported_actions().await.unwrap());
}

#[then("the queried supported actions should include config.get, config.apply, and config.schema")]
async fn assert_supported_actions(world: &mut FalconRotatorWorld) {
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

#[when("config.schema is called on the rotator device")]
async fn call_config_schema(world: &mut FalconRotatorWorld) {
    let rotator = Arc::clone(world.rotator());
    let body = rotator
        .action("config.schema".to_string(), String::new())
        .await
        .expect("config.schema failed");
    world.last_response =
        Some(serde_json::from_str(&body).expect("config.schema returned invalid JSON"));
}

#[then("the schema should describe the serial, server, rotator, and switch sections")]
async fn assert_schema_sections(world: &mut FalconRotatorWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let props = response["schema"]["properties"]
        .as_object()
        .expect("schema.properties is not an object");
    for section in ["serial", "server", "rotator", "switch"] {
        assert!(
            props.contains_key(section),
            "schema is missing section {section}: {props:?}"
        );
    }
}

#[then("the schema should mark rotator.unique_id and switch.unique_id as locked fields")]
async fn assert_schema_locked_fields(world: &mut FalconRotatorWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let locked = response["locked_fields"]
        .as_array()
        .expect("`locked_fields` is not an array");
    for expected in ["rotator.unique_id", "switch.unique_id"] {
        assert!(
            locked.iter().any(|f| f.as_str() == Some(expected)),
            "locked_fields {locked:?} does not include {expected}"
        );
    }
}

#[when("config.get is called on the rotator device")]
async fn call_config_get(world: &mut FalconRotatorWorld) {
    world.current_config().await;
}

#[then(regex = r"^the config should report serial\.port as (\S+)$")]
async fn assert_config_serial_port(world: &mut FalconRotatorWorld, expected: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    assert_eq!(
        response["config"]["serial"]["port"].as_str(),
        Some(expected.as_str())
    );
}

#[then("the config should report no overrides")]
async fn assert_no_overrides(world: &mut FalconRotatorWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let overrides = response["overrides"]
        .as_array()
        .expect("`overrides` is not an array");
    assert!(
        overrides.is_empty(),
        "expected no overrides, got {overrides:?}"
    );
}

#[when(regex = r#"^config\.apply sets the rotator name to "([^"]+)"$"#)]
async fn apply_rotator_name(world: &mut FalconRotatorWorld, name: String) {
    let mut config = world.current_config().await;
    config["rotator"]["name"] = serde_json::json!(name);
    world.call_config_apply(config).await;
}

#[when("config.apply is called with an empty serial port")]
async fn apply_empty_serial_port(world: &mut FalconRotatorWorld) {
    let mut config = world.current_config().await;
    config["serial"]["port"] = serde_json::json!("");
    world.call_config_apply(config).await;
}

#[then(regex = r"^the apply status should be (\w+)$")]
async fn assert_apply_status(world: &mut FalconRotatorWorld, expected: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    assert_eq!(response["status"].as_str(), Some(expected.as_str()));
}

#[then("the reload signal should fire")]
async fn assert_reload_fires(world: &mut FalconRotatorWorld) {
    let reload = world.reload.as_ref().expect("no reload signal");
    tokio::time::timeout(Duration::from_secs(2), reload.recv())
        .await
        .expect("config.apply should fire the reload");
}

#[then(regex = r#"^the persisted config should report the rotator name as "([^"]+)"$"#)]
async fn assert_persisted_rotator_name(world: &mut FalconRotatorWorld, expected: String) {
    let path = world.config_path.as_ref().expect("no config path");
    let content = std::fs::read_to_string(path).expect("read persisted config");
    let parsed: serde_json::Value =
        serde_json::from_str(&content).expect("persisted config is not valid JSON");
    assert_eq!(
        parsed["rotator"]["name"].as_str(),
        Some(expected.as_str()),
        "persisted config: {parsed}"
    );
}

#[then("the response should contain validation errors")]
async fn assert_validation_errors(world: &mut FalconRotatorWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let errors = response["errors"]
        .as_array()
        .expect("`errors` is not an array");
    assert!(!errors.is_empty(), "expected validation errors, got none");
}

#[when(regex = r#"^the action "([^"]+)" is called on the rotator device$"#)]
async fn call_named_action(world: &mut FalconRotatorWorld, name: String) {
    let rotator = Arc::clone(world.rotator());
    match rotator.action(name, String::new()).await {
        Ok(_) => world.last_error_code = None,
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

#[then("the call should fail with an action-not-implemented error")]
async fn assert_action_not_implemented(world: &mut FalconRotatorWorld) {
    assert_eq!(
        world.last_error_code,
        Some(ASCOMErrorCode::ACTION_NOT_IMPLEMENTED.raw()),
        "expected ACTION_NOT_IMPLEMENTED, got {:?}",
        world.last_error_code
    );
}
