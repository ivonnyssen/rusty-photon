//! Step definitions for switch_metadata.feature

use crate::world::mock_serial;
use crate::world::PpbaWorld;
use ascom_alpaca::api::{Device, Switch};
use cucumber::{given, then, when};
use ppba_driver::Config;

// ============================================================================
// Given steps
// ============================================================================

#[given(expr = "a switch device with name {string}")]
fn switch_device_with_name(world: &mut PpbaWorld, name: String) {
    let mut config = Config::default();
    config.switch.name = name;
    world.build_switch_device_with_config_and_responses(
        config,
        mock_serial::standard_connection_responses(),
    );
}

#[given(expr = "a switch device with unique ID {string}")]
fn switch_device_with_unique_id(world: &mut PpbaWorld, unique_id: String) {
    let mut config = Config::default();
    config.switch.unique_id = unique_id;
    world.build_switch_device_with_config_and_responses(
        config,
        mock_serial::standard_connection_responses(),
    );
}

#[given(expr = "a switch device with description {string}")]
fn switch_device_with_description(world: &mut PpbaWorld, description: String) {
    let mut config = Config::default();
    config.switch.description = description;
    world.build_switch_device_with_config_and_responses(
        config,
        mock_serial::standard_connection_responses(),
    );
}

// ============================================================================
// When steps
// ============================================================================

#[when(expr = "I try to set switch {int} name to {string}")]
async fn try_set_switch_name(world: &mut PpbaWorld, _id: usize, name: String) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.set_switch_name(0, name).await {
        Ok(()) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "the switch device static name should be {string}")]
fn switch_device_static_name_should_be(world: &mut PpbaWorld, expected: String) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert_eq!(device.static_name(), expected);
}

#[then(expr = "the switch device unique ID should be {string}")]
fn switch_device_unique_id_should_be(world: &mut PpbaWorld, expected: String) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert_eq!(device.unique_id(), expected);
}

#[then(expr = "the switch device description should be {string}")]
async fn switch_device_description_should_be(world: &mut PpbaWorld, expected: String) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let description = device.description().await.unwrap();
    assert_eq!(description, expected);
}

#[then(expr = "the switch device driver info should contain {string}")]
async fn switch_device_driver_info_should_contain(world: &mut PpbaWorld, expected: String) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let info = device.driver_info().await.unwrap();
    assert!(
        info.contains(&expected),
        "expected driver info to contain '{}', got: {}",
        expected,
        info
    );
}

#[then("the switch device driver version should not be empty")]
async fn switch_device_driver_version_not_empty(world: &mut PpbaWorld) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let version = device.driver_version().await.unwrap();
    assert!(!version.is_empty());
}

#[then(expr = "the switch device max switch should be {int}")]
async fn switch_device_max_switch_should_be(world: &mut PpbaWorld, expected: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let max = device.max_switch().await.unwrap();
    assert_eq!(max, expected);
}

#[then(expr = "all {int} switches should have non-empty names")]
async fn all_switches_have_names(world: &mut PpbaWorld, count: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in 0..count {
        let name = device.get_switch_name(id).await.unwrap();
        assert!(
            !name.is_empty(),
            "Switch {} should have a non-empty name",
            id
        );
    }
}

#[then(expr = "all {int} switches should have non-empty descriptions")]
async fn all_switches_have_descriptions(world: &mut PpbaWorld, count: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in 0..count {
        let desc = device.get_switch_description(id).await.unwrap();
        assert!(
            !desc.is_empty(),
            "Switch {} should have a non-empty description",
            id
        );
    }
}

#[then("all switches should have min less than max and positive step")]
async fn all_switches_consistent(world: &mut PpbaWorld) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in 0..16 {
        let min = device.min_switch_value(id).await.unwrap();
        let max = device.max_switch_value(id).await.unwrap();
        let step = device.switch_step(id).await.unwrap();
        assert!(
            min < max,
            "Switch {} min ({}) should be less than max ({})",
            id,
            min,
            max
        );
        assert!(
            step > 0.0,
            "Switch {} step should be positive, got {}",
            id,
            step
        );
    }
}

#[then(expr = "switch {int} min value should be {float}")]
async fn switch_min_value_should_be(world: &mut PpbaWorld, id: usize, expected: f64) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let min = device.min_switch_value(id).await.unwrap();
    assert_eq!(min, expected);
}

#[then(expr = "switch {int} max value should be {float}")]
async fn switch_max_value_should_be(world: &mut PpbaWorld, id: usize, expected: f64) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let max = device.max_switch_value(id).await.unwrap();
    assert_eq!(max, expected);
}

#[then(expr = "all {int} switches should have positive step values")]
async fn all_switches_positive_step(world: &mut PpbaWorld, count: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in 0..count {
        let step = device.switch_step(id).await.unwrap();
        assert!(
            step > 0.0,
            "Switch {} should have positive step, got {}",
            id,
            step
        );
    }
}

#[then(expr = "switch {int} name should be queryable")]
async fn switch_name_queryable(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    device.get_switch_name(id).await.unwrap();
}

#[then(expr = "querying switch {int} name should fail")]
async fn querying_switch_name_should_fail(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(device.get_switch_name(id).await.is_err());
}

#[then(expr = "the switch device debug output should contain {string}")]
fn switch_device_debug_should_contain(world: &mut PpbaWorld, expected: String) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let debug_output = format!("{:?}", device);
    assert!(
        debug_output.contains(&expected),
        "expected debug output to contain '{}', got: {}",
        expected,
        debug_output
    );
}

#[then("the switch device static name should not be empty")]
fn switch_device_static_name_not_empty(world: &mut PpbaWorld) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(!device.static_name().is_empty());
}

#[then("the switch device unique ID should not be empty")]
fn switch_device_unique_id_not_empty(world: &mut PpbaWorld) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(!device.unique_id().is_empty());
}
