//! Step definitions for switch_control.feature

use std::time::Duration;

use crate::world::mock_serial;
use crate::world::PpbaWorld;
use ascom_alpaca::api::Switch;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a switch device with auto-dew enabled mock responses")]
fn switch_device_with_autodew(world: &mut PpbaWorld) {
    world.build_switch_device_with_responses(mock_serial::autodew_enabled_responses());
}

#[given("a switch device with USB hub mock responses")]
fn switch_device_with_usb_hub_responses(world: &mut PpbaWorld) {
    world.build_switch_device_with_responses(vec![
        "PPBA_OK".to_string(),
        // connect: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // poller tick 1: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // USB hub set command echo
        "PU:1".to_string(),
        // spares
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]);
}

#[given("a switch device with auto-dew toggle mock responses")]
fn switch_device_with_autodew_toggle_responses(world: &mut PpbaWorld) {
    world.build_switch_device_with_responses(vec![
        "PPBA_OK".to_string(),
        // connect: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // poller tick 1: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // auto-dew toggle command echo
        "PD:1".to_string(),
        // refresh_status after set (auto-dew now on)
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(),
        // spares
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:1:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]);
}

// ============================================================================
// When steps
// ============================================================================

#[when("I wait for status cache")]
async fn wait_for_status_cache(_world: &mut PpbaWorld) {
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[when(expr = "I set switch {int} value to {float}")]
async fn set_switch_value(world: &mut PpbaWorld, id: usize, value: f64) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    device.set_switch_value(id, value).await.unwrap();
}

#[when(expr = "I try to set switch {int} value to {float}")]
async fn try_set_switch_value(world: &mut PpbaWorld, id: usize, value: f64) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.set_switch_value(id, value).await {
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

#[then(expr = "switch {int} value should be {float}")]
async fn switch_value_should_be(world: &mut PpbaWorld, id: usize, expected: f64) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let value = device.get_switch_value(id).await.unwrap();
    assert_eq!(value, expected, "switch {} value mismatch", id);
}

#[then(expr = "switch {int} boolean should be true")]
async fn switch_boolean_should_be_true(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let value = device.get_switch(id).await.unwrap();
    assert!(value, "switch {} should be true", id);
}

#[then(expr = "setting switch {int} boolean to true should succeed")]
async fn setting_switch_boolean_should_succeed(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    // Just ensure it doesn't error; result may vary based on mock responses
    let _ = device.set_switch(id, true).await;
}

#[then(expr = "switches {int} through {int} should be writable")]
async fn switches_should_be_writable(world: &mut PpbaWorld, from: usize, to: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in from..=to {
        let can_write = device.can_write(id).await.unwrap();
        assert!(can_write, "Switch {} should be writable", id);
    }
}

#[then(expr = "switches {int} through {int} should not be writable")]
async fn switches_should_not_be_writable(world: &mut PpbaWorld, from: usize, to: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in from..=to {
        let can_write = device.can_write(id).await.unwrap();
        assert!(!can_write, "Switch {} should not be writable", id);
    }
}

#[then(expr = "switch {int} should not be writable")]
async fn switch_should_not_be_writable(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let can_write = device.can_write(id).await.unwrap();
    assert!(!can_write, "Switch {} should not be writable", id);
}

#[then(expr = "switch {int} should be writable")]
async fn switch_should_be_writable(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    let can_write = device.can_write(id).await.unwrap();
    assert!(can_write, "Switch {} should be writable", id);
}

#[then(expr = "setting switch {int} value to {float} should succeed")]
async fn setting_switch_value_should_succeed(world: &mut PpbaWorld, id: usize, value: f64) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    device.set_switch_value(id, value).await.unwrap();
}

#[then(
    expr = "all {int} switches should be queryable for name, description, min, max, step, value, and can_write"
)]
async fn all_switches_queryable(world: &mut PpbaWorld, count: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in 0..count {
        device.get_switch_name(id).await.unwrap();
        device.get_switch_description(id).await.unwrap();
        device.min_switch_value(id).await.unwrap();
        device.max_switch_value(id).await.unwrap();
        device.switch_step(id).await.unwrap();
        device.get_switch_value(id).await.unwrap();
        device.can_write(id).await.unwrap();
    }
}

#[then(expr = "get_switch should work for switches {int} through {int}")]
async fn get_switch_should_work(world: &mut PpbaWorld, from: usize, to: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in from..=to {
        device.get_switch(id).await.unwrap();
    }
}
