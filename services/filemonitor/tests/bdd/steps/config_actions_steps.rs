//! Step definitions for config_actions.feature.

use std::sync::Arc;

use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

use crate::world::FilemonitorWorld;

#[when("the supported actions are queried")]
async fn query_supported_actions(world: &mut FilemonitorWorld) {
    let monitor = Arc::clone(world.monitor());
    world.last_supported_actions = Some(monitor.supported_actions().await.unwrap());
}

#[when("config.get is called")]
async fn call_config_get(world: &mut FilemonitorWorld) {
    world.call_config_get().await;
}

#[when("config.schema is called")]
async fn call_config_schema(world: &mut FilemonitorWorld) {
    world.call_config_schema().await;
}

#[when(regex = r"^config\.apply sets the polling interval to (\S+)$")]
async fn apply_polling_interval(world: &mut FilemonitorWorld, value: String) {
    let mut config = world.current_config().await;
    config["file"]["polling_interval"] = serde_json::json!(value);
    world.call_config_apply(config).await;
}

#[when(regex = r"^config\.apply pins the bound port and sets the polling interval to (\S+)$")]
async fn apply_pin_port_and_polling_interval(world: &mut FilemonitorWorld, value: String) {
    // Pin the (OS-assigned) bound port into the file so the reload rebinds the
    // *same* port — letting this client reach the reloaded server.
    let port = world.bound_port();
    let mut config = world.current_config().await;
    config["server"]["port"] = serde_json::json!(port);
    config["file"]["polling_interval"] = serde_json::json!(value);
    world.call_config_apply(config).await;
}

#[when("config.apply is called with an empty unique_id")]
async fn apply_empty_unique_id(world: &mut FilemonitorWorld) {
    let mut config = world.current_config().await;
    config["device"]["unique_id"] = serde_json::json!("");
    world.call_config_apply(config).await;
}

#[when("config.apply is called with an invalid regex pattern")]
async fn apply_invalid_regex_pattern(world: &mut FilemonitorWorld) {
    let mut config = world.current_config().await;
    config["parsing"]["rules"] = serde_json::json!([
        { "type": "regex", "pattern": "(unclosed", "safe": true },
    ]);
    world.call_config_apply(config).await;
}

#[when(regex = r#"^the action "([^"]+)" is called$"#)]
async fn call_named_action(world: &mut FilemonitorWorld, name: String) {
    let monitor = Arc::clone(world.monitor());
    world.last_ascom_error = monitor.action(name, String::new()).await.err();
}

#[then("the supported actions should include config.get and config.apply")]
async fn assert_supported_actions(world: &mut FilemonitorWorld) {
    let actions = world
        .last_supported_actions
        .as_ref()
        .expect("no supported actions queried");
    assert!(
        actions.iter().any(|a| a == "config.get"),
        "config.get missing from {actions:?}"
    );
    assert!(
        actions.iter().any(|a| a == "config.apply"),
        "config.apply missing from {actions:?}"
    );
}

#[then("the schema should describe the device, file, parsing, and server sections")]
async fn assert_schema_sections(world: &mut FilemonitorWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let props = response["schema"]["properties"]
        .as_object()
        .expect("schema.properties is not an object");
    for section in ["device", "file", "parsing", "server"] {
        assert!(
            props.contains_key(section),
            "schema is missing section {section}: {props:?}"
        );
    }
}

#[then("the schema should mark device.unique_id as a locked field")]
async fn assert_schema_locked_field(world: &mut FilemonitorWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let locked = response["locked_fields"]
        .as_array()
        .expect("`locked_fields` is not an array");
    assert!(
        locked
            .iter()
            .any(|f| f.as_str() == Some("device.unique_id")),
        "locked_fields {locked:?} does not include device.unique_id"
    );
}

#[then("the schema should mark server.port as a read-only field")]
async fn assert_schema_read_only_field(world: &mut FilemonitorWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let read_only = response["read_only_fields"]
        .as_array()
        .expect("`read_only_fields` is not an array");
    assert!(
        read_only.iter().any(|f| f.as_str() == Some("server.port")),
        "read_only_fields {read_only:?} does not include server.port"
    );
}

#[then(regex = r"^the config should report device\.unique_id as (\S+)$")]
async fn assert_config_unique_id(world: &mut FilemonitorWorld, expected: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    assert_eq!(
        response["config"]["device"]["unique_id"].as_str(),
        Some(expected.as_str())
    );
}

#[then("the config should report no overrides")]
async fn assert_no_overrides(world: &mut FilemonitorWorld) {
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
async fn assert_apply_status(world: &mut FilemonitorWorld, expected: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    assert_eq!(response["status"].as_str(), Some(expected.as_str()));
}

#[then(regex = r"^the reload list should include (\S+)$")]
async fn assert_reload_includes(world: &mut FilemonitorWorld, path: String) {
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
async fn assert_validation_errors(world: &mut FilemonitorWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let errors = response["errors"]
        .as_array()
        .expect("`errors` is not an array");
    assert!(!errors.is_empty(), "expected validation errors, got none");
}

#[then(regex = r"^the reloaded service serves polling interval (\S+)$")]
async fn assert_reloaded_serves(world: &mut FilemonitorWorld, expected: String) {
    world.wait_for_config_polling_interval(&expected).await;
}

#[then("the call should fail with an action-not-implemented error")]
async fn assert_action_not_implemented(world: &mut FilemonitorWorld) {
    let err = world
        .last_ascom_error
        .take()
        .expect("expected an error from the call");
    assert_eq!(err.code, ASCOMErrorCode::ACTION_NOT_IMPLEMENTED);
}
