//! Step definitions for switch_metadata.feature

use crate::steps::infrastructure::default_test_config;
use crate::world::PpbaWorld;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given(expr = "a running PPBA server with switch name {string}")]
async fn running_server_with_switch_name(world: &mut PpbaWorld, name: String) {
    world.config = default_test_config();
    world.config["switch"]["name"] = serde_json::json!(name);
    world.start_ppba().await;
}

#[given(expr = "a running PPBA server with switch unique ID {string}")]
async fn running_server_with_switch_unique_id(world: &mut PpbaWorld, unique_id: String) {
    world.config = default_test_config();
    world.config["switch"]["unique_id"] = serde_json::json!(unique_id);
    world.start_ppba().await;
}

#[given(expr = "a running PPBA server with switch description {string}")]
async fn running_server_with_switch_description(world: &mut PpbaWorld, description: String) {
    world.config = default_test_config();
    world.config["switch"]["description"] = serde_json::json!(description);
    world.start_ppba().await;
}

// ============================================================================
// When steps
// ============================================================================

#[when(expr = "I try to set switch {int} name to {string}")]
async fn try_set_switch_name(world: &mut PpbaWorld, id: i32, name: String) {
    let result = world.switch_ref().set_switch_name(id as usize, name).await;
    world.capture_result(result);
}

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "the switch device static name should be {string}")]
async fn switch_device_static_name_should_be(world: &mut PpbaWorld, expected: String) {
    let name = world.switch_ref().name().await.unwrap();
    assert_eq!(name, expected, "switch device name mismatch");
}

#[then(expr = "the switch device unique ID should be {string}")]
fn switch_device_unique_id_should_be(world: &mut PpbaWorld, expected: String) {
    let uid = world.switch_ref().unique_id();
    assert_eq!(uid, expected, "switch unique ID mismatch");
}

#[then(expr = "the switch device description should be {string}")]
async fn switch_device_description_should_be(world: &mut PpbaWorld, expected: String) {
    let desc = world.switch_ref().description().await.unwrap();
    assert!(
        desc.contains(&expected),
        "expected description to contain '{}', got: {}",
        expected,
        desc
    );
}

#[then(expr = "the switch device driver info should contain {string}")]
async fn switch_device_driver_info_should_contain(world: &mut PpbaWorld, expected: String) {
    let info = world.switch_ref().driver_info().await.unwrap();
    assert!(
        info.contains(&expected),
        "expected driver info to contain '{}', got: {}",
        expected,
        info
    );
}

#[then("the switch device driver version should not be empty")]
async fn switch_device_driver_version_not_empty(world: &mut PpbaWorld) {
    let version = world.switch_ref().driver_version().await.unwrap();
    assert!(!version.is_empty(), "driver version should not be empty");
}

#[then(expr = "the switch device max switch should be {int}")]
async fn switch_device_max_switch_should_be(world: &mut PpbaWorld, expected: i32) {
    let max = world.switch_ref().max_switch().await.unwrap();
    assert_eq!(max, expected as usize, "max switch mismatch");
}

#[then(expr = "all {int} switches should have non-empty names")]
async fn all_switches_have_names(world: &mut PpbaWorld, count: i32) {
    let switch = world.switch_ref();
    for id in 0..count {
        let name = switch.get_switch_name(id as usize).await.unwrap();
        assert!(
            !name.is_empty(),
            "switch {} should have a non-empty name",
            id
        );
    }
}

#[then(expr = "all {int} switches should have non-empty descriptions")]
async fn all_switches_have_descriptions(world: &mut PpbaWorld, count: i32) {
    let switch = world.switch_ref();
    for id in 0..count {
        let desc = switch.get_switch_description(id as usize).await.unwrap();
        assert!(
            !desc.is_empty(),
            "switch {} should have a non-empty description",
            id
        );
    }
}

#[then("all switches should have min less than max and positive step")]
async fn all_switches_consistent(world: &mut PpbaWorld) {
    let switch = world.switch_ref();
    for id in 0..16usize {
        let min = switch.min_switch_value(id).await.unwrap();
        let max = switch.max_switch_value(id).await.unwrap();
        let step = switch.switch_step(id).await.unwrap();

        assert!(
            min < max,
            "switch {} min ({}) should be less than max ({})",
            id,
            min,
            max
        );
        assert!(
            step > 0.0,
            "switch {} step should be positive, got {}",
            id,
            step
        );
    }
}

#[then(expr = "switch {int} min value should be {float}")]
async fn switch_min_value_should_be(world: &mut PpbaWorld, id: i32, expected: f64) {
    let min = world
        .switch_ref()
        .min_switch_value(id as usize)
        .await
        .unwrap();
    assert!(
        (min - expected).abs() < f64::EPSILON,
        "switch {} min: expected {}, got {}",
        id,
        expected,
        min
    );
}

#[then(expr = "switch {int} max value should be {float}")]
async fn switch_max_value_should_be(world: &mut PpbaWorld, id: i32, expected: f64) {
    let max = world
        .switch_ref()
        .max_switch_value(id as usize)
        .await
        .unwrap();
    assert!(
        (max - expected).abs() < f64::EPSILON,
        "switch {} max: expected {}, got {}",
        id,
        expected,
        max
    );
}

#[then(expr = "all {int} switches should have positive step values")]
async fn all_switches_positive_step(world: &mut PpbaWorld, count: i32) {
    let switch = world.switch_ref();
    for id in 0..count {
        let step = switch.switch_step(id as usize).await.unwrap();
        assert!(
            step > 0.0,
            "switch {} should have positive step, got {}",
            id,
            step
        );
    }
}

#[then(expr = "switch {int} name should be queryable")]
async fn switch_name_queryable(world: &mut PpbaWorld, id: i32) {
    world
        .switch_ref()
        .get_switch_name(id as usize)
        .await
        .unwrap();
}

#[then(expr = "querying switch {int} name should fail")]
async fn querying_switch_name_should_fail(world: &mut PpbaWorld, id: i32) {
    world
        .switch_ref()
        .get_switch_name(id as usize)
        .await
        .unwrap_err();
}

#[then("the switch device static name should not be empty")]
async fn switch_device_static_name_not_empty(world: &mut PpbaWorld) {
    let name = world.switch_ref().name().await.unwrap();
    assert!(!name.is_empty(), "switch device name should not be empty");
}

#[then("the switch device unique ID should not be empty")]
fn switch_device_unique_id_not_empty(world: &mut PpbaWorld) {
    let uid = world.switch_ref().unique_id();
    assert!(!uid.is_empty(), "switch unique ID should not be empty");
}
