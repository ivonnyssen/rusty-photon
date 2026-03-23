//! Step definitions for switch_metadata.feature

use crate::steps::infrastructure::*;
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
    let url = world.switch_url();
    let id_str = id.to_string();
    let resp = alpaca_put(&url, "setswitchname", &[("Id", &id_str), ("Name", &name)]).await;
    world.capture_response(&resp);
}

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "the switch device static name should be {string}")]
async fn switch_device_static_name_should_be(world: &mut PpbaWorld, expected: String) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, "name").await;
    assert!(!is_alpaca_error(&resp), "GET name failed");
    assert_eq!(
        alpaca_value(&resp).as_str().unwrap(),
        expected,
        "switch device name mismatch"
    );
}

#[then(expr = "the switch device unique ID should be {string}")]
async fn switch_device_unique_id_should_be(world: &mut PpbaWorld, expected: String) {
    let base = world.base_url.as_ref().expect("server not started");
    let resp = alpaca_get(base, "management/v1/configureddevices").await;
    let devices = alpaca_value(&resp)
        .as_array()
        .expect("configureddevices should return an array");
    let switch_entry = devices
        .iter()
        .find(|d| d["DeviceType"].as_str() == Some("Switch"))
        .expect("no Switch device found in configureddevices");
    assert_eq!(
        switch_entry["UniqueID"].as_str().unwrap(),
        expected,
        "switch unique ID mismatch"
    );
}

#[then(expr = "the switch device description should be {string}")]
async fn switch_device_description_should_be(world: &mut PpbaWorld, expected: String) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, "description").await;
    assert!(!is_alpaca_error(&resp), "GET description failed");
    let desc = alpaca_value(&resp).as_str().unwrap();
    assert!(
        desc.contains(&expected),
        "expected description to contain '{}', got: {}",
        expected,
        desc
    );
}

#[then(expr = "the switch device driver info should contain {string}")]
async fn switch_device_driver_info_should_contain(world: &mut PpbaWorld, expected: String) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, "driverinfo").await;
    assert!(!is_alpaca_error(&resp), "GET driverinfo failed");
    let info = alpaca_value(&resp).as_str().unwrap();
    assert!(
        info.contains(&expected),
        "expected driver info to contain '{}', got: {}",
        expected,
        info
    );
}

#[then("the switch device driver version should not be empty")]
async fn switch_device_driver_version_not_empty(world: &mut PpbaWorld) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, "driverversion").await;
    assert!(!is_alpaca_error(&resp), "GET driverversion failed");
    let version = alpaca_value(&resp).as_str().unwrap();
    assert!(!version.is_empty(), "driver version should not be empty");
}

#[then(expr = "the switch device max switch should be {int}")]
async fn switch_device_max_switch_should_be(world: &mut PpbaWorld, expected: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, "maxswitch").await;
    assert!(!is_alpaca_error(&resp), "GET maxswitch failed");
    let max = alpaca_value(&resp).as_i64().unwrap();
    assert_eq!(max, expected as i64, "max switch mismatch");
}

#[then(expr = "all {int} switches should have non-empty names")]
async fn all_switches_have_names(world: &mut PpbaWorld, count: i32) {
    let url = world.switch_url();
    for id in 0..count {
        let resp = alpaca_get(&url, &format!("getswitchname?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "getswitchname failed for switch {}",
            id
        );
        let name = alpaca_value(&resp).as_str().unwrap();
        assert!(
            !name.is_empty(),
            "switch {} should have a non-empty name",
            id
        );
    }
}

#[then(expr = "all {int} switches should have non-empty descriptions")]
async fn all_switches_have_descriptions(world: &mut PpbaWorld, count: i32) {
    let url = world.switch_url();
    for id in 0..count {
        let resp = alpaca_get(&url, &format!("getswitchdescription?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "getswitchdescription failed for switch {}",
            id
        );
        let desc = alpaca_value(&resp).as_str().unwrap();
        assert!(
            !desc.is_empty(),
            "switch {} should have a non-empty description",
            id
        );
    }
}

#[then("all switches should have min less than max and positive step")]
async fn all_switches_consistent(world: &mut PpbaWorld) {
    let url = world.switch_url();
    for id in 0..16 {
        let resp_min = alpaca_get(&url, &format!("minswitchvalue?Id={}", id)).await;
        let resp_max = alpaca_get(&url, &format!("maxswitchvalue?Id={}", id)).await;
        let resp_step = alpaca_get(&url, &format!("switchstep?Id={}", id)).await;

        let min = alpaca_value(&resp_min).as_f64().unwrap();
        let max = alpaca_value(&resp_max).as_f64().unwrap();
        let step = alpaca_value(&resp_step).as_f64().unwrap();

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
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("minswitchvalue?Id={}", id)).await;
    assert!(
        !is_alpaca_error(&resp),
        "minswitchvalue failed for switch {}",
        id
    );
    let min = alpaca_value(&resp).as_f64().unwrap();
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
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("maxswitchvalue?Id={}", id)).await;
    assert!(
        !is_alpaca_error(&resp),
        "maxswitchvalue failed for switch {}",
        id
    );
    let max = alpaca_value(&resp).as_f64().unwrap();
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
    let url = world.switch_url();
    for id in 0..count {
        let resp = alpaca_get(&url, &format!("switchstep?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "switchstep failed for switch {}",
            id
        );
        let step = alpaca_value(&resp).as_f64().unwrap();
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
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("getswitchname?Id={}", id)).await;
    assert!(
        !is_alpaca_error(&resp),
        "getswitchname should succeed for switch {}",
        id
    );
}

#[then(expr = "querying switch {int} name should fail")]
async fn querying_switch_name_should_fail(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("getswitchname?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "getswitchname should fail for switch {}",
        id
    );
}

#[then("the switch device static name should not be empty")]
async fn switch_device_static_name_not_empty(world: &mut PpbaWorld) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, "name").await;
    assert!(!is_alpaca_error(&resp), "GET name failed");
    let name = alpaca_value(&resp).as_str().unwrap();
    assert!(!name.is_empty(), "switch device name should not be empty");
}

#[then("the switch device unique ID should not be empty")]
async fn switch_device_unique_id_not_empty(world: &mut PpbaWorld) {
    let base = world.base_url.as_ref().expect("server not started");
    let resp = alpaca_get(base, "management/v1/configureddevices").await;
    let devices = alpaca_value(&resp)
        .as_array()
        .expect("configureddevices should return an array");
    let switch_entry = devices
        .iter()
        .find(|d| d["DeviceType"].as_str() == Some("Switch"))
        .expect("no Switch device found in configureddevices");
    let uid = switch_entry["UniqueID"].as_str().unwrap();
    assert!(!uid.is_empty(), "switch unique ID should not be empty");
}
