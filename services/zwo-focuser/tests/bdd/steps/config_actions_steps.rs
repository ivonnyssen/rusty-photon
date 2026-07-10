//! Configuration-action steps (`config.get` / `config.apply` / `config.schema`).

use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

use crate::world::FocuserWorld;

#[when(regex = r"^the supported actions are queried on focuser device (\d+)$")]
async fn query_supported_actions(world: &mut FocuserWorld, _device: u32) {
    let focuser = world.focuser();
    world.last_actions = Some(focuser.supported_actions().await.unwrap());
}

#[then("the supported actions should include config.get, config.apply, and config.schema")]
async fn supported_actions_include_config(world: &mut FocuserWorld) {
    let actions = world
        .last_actions
        .as_ref()
        .expect("no supported actions stashed");
    for action in ["config.get", "config.apply", "config.schema"] {
        assert!(
            actions.iter().any(|a| a == action),
            "supported actions {actions:?} missing {action}"
        );
    }
}

#[when("config.schema is called")]
async fn call_config_schema(world: &mut FocuserWorld) {
    world.call_action("config.schema", "").await;
}

#[then("the schema should describe the devices and server sections")]
async fn schema_describes_sections(world: &mut FocuserWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let props = response["schema"]["properties"]
        .as_object()
        .expect("schema.properties is an object");
    for section in ["devices", "server"] {
        assert!(
            props.contains_key(section),
            "schema missing section {section}"
        );
    }
}

#[then(regex = r"^the schema should mark (\S+) as a read-only field$")]
async fn schema_marks_read_only(world: &mut FocuserWorld, field: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let read_only = response["read_only_fields"]
        .as_array()
        .expect("read_only_fields is an array");
    assert!(
        read_only.iter().any(|v| v.as_str() == Some(field.as_str())),
        "read_only_fields {read_only:?} missing {field}"
    );
}

#[then("the schema should report no locked identity fields")]
async fn schema_no_locked_fields(world: &mut FocuserWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let locked = response["locked_fields"]
        .as_array()
        .expect("locked_fields is an array");
    assert!(
        locked.is_empty(),
        "expected no locked fields, got {locked:?}"
    );
}

#[when("config.get is called")]
async fn call_config_get(world: &mut FocuserWorld) {
    world.call_action("config.get", "").await;
}

#[then("the config should report an empty devices map")]
async fn config_empty_devices(world: &mut FocuserWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let devices = response["config"]["devices"]
        .as_object()
        .expect("config.devices is an object");
    assert!(
        devices.is_empty(),
        "expected empty devices, got {devices:?}"
    );
}

#[then("the config should report no CLI-pinned override paths")]
async fn config_no_overrides(world: &mut FocuserWorld) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let overrides = response["overrides"]
        .as_array()
        .expect("overrides is an array");
    assert!(
        overrides.is_empty(),
        "expected no overrides, got {overrides:?}"
    );
}

#[when(regex = r#"^config\.apply sets the devices override "([^"]+)" name to "([^"]+)"$"#)]
async fn apply_device_name(world: &mut FocuserWorld, serial: String, name: String) {
    let mut config = world.config_get().await;
    config["devices"]
        .as_object_mut()
        .expect("config.devices is an object")
        .insert(serial, serde_json::json!({ "name": name }));
    world.call_action("config.apply", &config.to_string()).await;
}

#[then(regex = r"^the apply status should be (\w+)$")]
async fn apply_status_is(world: &mut FocuserWorld, status: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    assert_eq!(response["status"].as_str(), Some(status.as_str()));
}

#[then(regex = r"^the reload list should include (\w+)$")]
async fn reload_list_includes(world: &mut FocuserWorld, section: String) {
    let response = world.last_response.as_ref().expect("no response stashed");
    let reload = response["reload"].as_array().expect("reload is an array");
    assert!(
        reload
            .iter()
            .any(|v| v.as_str().is_some_and(|s| s.starts_with(&section))),
        "reload {reload:?} does not include {section}"
    );
}

#[when(regex = r#"^the action "([^"]+)" is called on focuser device (\d+)$"#)]
async fn call_named_action(world: &mut FocuserWorld, action: String, _device: u32) {
    world.call_action(&action, "").await;
}

#[then("the call should fail with an action-not-implemented error")]
async fn call_action_not_implemented(world: &mut FocuserWorld) {
    assert_eq!(
        world.last_error_code,
        Some(ASCOMErrorCode::ACTION_NOT_IMPLEMENTED.raw())
    );
}
