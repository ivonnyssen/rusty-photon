//! Step definitions for connection_lifecycle.feature

use crate::world::mock_serial;
use crate::world::QhyFocuserWorld;
use ascom_alpaca::api::Device;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a focuser device with standard mock responses")]
fn device_with_standard_responses(world: &mut QhyFocuserWorld) {
    world.build_device_with_responses(mock_serial::standard_connection_responses());
}

#[given(expr = "a focuser device with a failing serial port {string}")]
fn device_with_failing_serial(world: &mut QhyFocuserWorld, error_msg: String) {
    world.build_device_with_failing_factory(&error_msg);
}

#[given("a serial manager with standard mock responses")]
fn manager_with_standard_responses(world: &mut QhyFocuserWorld) {
    world.build_manager_with_responses(mock_serial::standard_connection_responses());
}

#[given("a serial manager with no responses")]
fn manager_with_no_responses(world: &mut QhyFocuserWorld) {
    world.build_manager_with_responses(vec![]);
}

// ============================================================================
// When steps
// ============================================================================

#[when("I connect the device")]
async fn connect_device(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    device.set_connected(true).await.unwrap();
}

#[when("I disconnect the device")]
async fn disconnect_device(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    device.set_connected(false).await.unwrap();
}

#[when("I try to connect the device")]
async fn try_connect_device(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    match device.set_connected(true).await {
        Ok(()) => world.last_error = None,
        Err(e) => world.last_error = Some(e.to_string()),
    }
}

#[when("I connect the serial manager")]
async fn connect_serial_manager(world: &mut QhyFocuserWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    manager.connect().await.unwrap();
}

#[when("I disconnect the serial manager")]
async fn disconnect_serial_manager(world: &mut QhyFocuserWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    manager.disconnect().await;
}

// ============================================================================
// Then steps
// ============================================================================

#[then("the device should be disconnected")]
async fn device_should_be_disconnected(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    assert!(!device.connected().await.unwrap());
}

#[then("the device should be connected")]
async fn device_should_be_connected(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    assert!(device.connected().await.unwrap());
}

#[then(expr = "connecting should fail with an error containing {string}")]
fn connecting_should_fail_with(world: &mut QhyFocuserWorld, expected: String) {
    let error = world
        .last_error
        .as_ref()
        .expect("expected a connection error but none occurred");
    assert!(
        error.contains(&expected),
        "expected error containing '{}', got: {}",
        expected,
        error
    );
}

#[then("the serial manager should be available")]
fn manager_should_be_available(world: &mut QhyFocuserWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    assert!(manager.is_available());
}

#[then("the serial manager should not be available")]
fn manager_should_not_be_available(world: &mut QhyFocuserWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    assert!(!manager.is_available());
}

#[then(expr = "the serial manager debug representation should contain {string}")]
fn manager_debug_contains(world: &mut QhyFocuserWorld, expected: String) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let debug_str = format!("{:?}", manager);
    assert!(
        debug_str.contains(&expected),
        "expected debug to contain '{}', got: {}",
        expected,
        debug_str
    );
}
