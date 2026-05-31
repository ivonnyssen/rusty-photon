//! Step definitions for config_actions.feature.
//!
//! Reuses the optics/SkyView Givens and `I start the service` When from
//! connection_steps; the spawned binary serves the actions through its
//! run_reloadable loop.

use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

use crate::world::SkySurveyCameraWorld;

#[when("the supported actions are queried")]
async fn query_supported_actions(world: &mut SkySurveyCameraWorld) {
    let camera = world.acquire_camera().await;
    world.last_supported_actions = Some(camera.supported_actions().await.unwrap());
}

#[then("the queried supported actions should include config.get, config.apply, and config.schema")]
async fn assert_supported_actions(world: &mut SkySurveyCameraWorld) {
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

#[when("config.schema is called")]
async fn call_config_schema(world: &mut SkySurveyCameraWorld) {
    let camera = world.acquire_camera().await;
    let body = camera
        .action("config.schema".to_string(), String::new())
        .await
        .expect("config.schema failed");
    world.last_response =
        Some(serde_json::from_str(&body).expect("config.schema returned invalid JSON"));
}

#[then("the schema should describe the device, optics, pointing, survey, and server sections")]
async fn assert_schema_sections(world: &mut SkySurveyCameraWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let props = response["schema"]["properties"]
        .as_object()
        .expect("schema.properties is not an object");
    for section in ["device", "optics", "pointing", "survey", "server"] {
        assert!(
            props.contains_key(section),
            "schema is missing section {section}: {props:?}"
        );
    }
}

#[then("the schema should mark device.unique_id as a locked field")]
async fn assert_schema_locked_field(world: &mut SkySurveyCameraWorld) {
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

#[when("config.get is called")]
async fn call_config_get(world: &mut SkySurveyCameraWorld) {
    world.current_config().await;
}

#[then(regex = r#"^the config should report the device name as "([^"]+)"$"#)]
async fn assert_config_device_name(world: &mut SkySurveyCameraWorld, expected: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    assert_eq!(
        response["config"]["device"]["name"].as_str(),
        Some(expected.as_str())
    );
}

#[then("the config should report no overrides")]
async fn assert_no_overrides(world: &mut SkySurveyCameraWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let overrides = response["overrides"]
        .as_array()
        .expect("`overrides` is not an array");
    assert!(
        overrides.is_empty(),
        "expected no overrides, got {overrides:?}"
    );
}

#[when(
    regex = r#"^config\.apply pins the bound port and sets the device description to "([^"]+)"$"#
)]
async fn apply_pin_port_and_description(world: &mut SkySurveyCameraWorld, description: String) {
    let port = world.bound_port();
    let mut config = world.current_config().await;
    config["server"]["port"] = serde_json::json!(port);
    config["device"]["description"] = serde_json::json!(description);
    world.call_config_apply(config).await;
}

#[when("config.apply is called with an empty device unique_id")]
async fn apply_empty_unique_id(world: &mut SkySurveyCameraWorld) {
    let mut config = world.current_config().await;
    config["device"]["unique_id"] = serde_json::json!("");
    world.call_config_apply(config).await;
}

#[then(regex = r"^the apply status should be (\w+)$")]
async fn assert_apply_status(world: &mut SkySurveyCameraWorld, expected: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    assert_eq!(response["status"].as_str(), Some(expected.as_str()));
}

#[then(regex = r#"^the reloaded service serves device description "([^"]+)"$"#)]
async fn assert_reloaded_serves(world: &mut SkySurveyCameraWorld, expected: String) {
    world.wait_for_config_description(&expected).await;
}

#[then("the response should contain validation errors")]
async fn assert_validation_errors(world: &mut SkySurveyCameraWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let errors = response["errors"]
        .as_array()
        .expect("`errors` is not an array");
    assert!(!errors.is_empty(), "expected validation errors, got none");
}

#[when(regex = r#"^the action "([^"]+)" is called$"#)]
async fn call_named_action(world: &mut SkySurveyCameraWorld, name: String) {
    let camera = world.acquire_camera().await;
    match camera.action(name, String::new()).await {
        Ok(_) => world.last_action_error_code = None,
        Err(e) => world.last_action_error_code = Some(e.code.raw()),
    }
}

#[then("the call should fail with an action-not-implemented error")]
async fn assert_action_not_implemented(world: &mut SkySurveyCameraWorld) {
    assert_eq!(
        world.last_action_error_code,
        Some(ASCOMErrorCode::ACTION_NOT_IMPLEMENTED.raw()),
        "expected ACTION_NOT_IMPLEMENTED, got {:?}",
        world.last_action_error_code
    );
}
