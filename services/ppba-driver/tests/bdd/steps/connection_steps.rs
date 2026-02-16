//! Step definitions for connection_lifecycle.feature

use crate::world::mock_serial;
use crate::world::PpbaWorld;
use ascom_alpaca::api::Device;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a switch device with standard mock responses")]
fn switch_device_with_standard_responses(world: &mut PpbaWorld) {
    world.build_switch_device_with_responses(mock_serial::standard_connection_responses());
}

#[given("an OC device with standard mock responses")]
fn oc_device_with_standard_responses(world: &mut PpbaWorld) {
    world.build_oc_device_with_responses(mock_serial::standard_connection_responses());
}

#[given(expr = "a switch device with a failing serial port {string}")]
fn switch_device_with_failing_serial(world: &mut PpbaWorld, error_msg: String) {
    world.build_switch_device_with_failing_factory(&error_msg);
}

#[given(expr = "an OC device with a failing serial port {string}")]
fn oc_device_with_failing_serial(world: &mut PpbaWorld, error_msg: String) {
    world.build_oc_device_with_failing_factory(&error_msg);
}

#[given("a switch device with bad ping response")]
fn switch_device_with_bad_ping(world: &mut PpbaWorld) {
    world.build_switch_device_with_bad_ping();
}

#[given("an OC device with bad ping response")]
fn oc_device_with_bad_ping(world: &mut PpbaWorld) {
    world.build_oc_device_with_bad_ping();
}

#[given("a serial manager with standard mock responses")]
fn manager_with_standard_responses(world: &mut PpbaWorld) {
    world.build_manager_with_responses(mock_serial::standard_connection_responses());
}

#[given("a serial manager with no mock responses")]
fn manager_with_no_responses(world: &mut PpbaWorld) {
    world.build_manager_with_responses(vec![]);
}

#[given(expr = "a serial manager with a failing factory {string}")]
fn manager_with_failing_factory(world: &mut PpbaWorld, error_msg: String) {
    world.build_manager_with_failing_factory(&error_msg);
}

#[given("a serial manager with bad ping response")]
fn manager_with_bad_ping(world: &mut PpbaWorld) {
    world.build_manager_with_responses(vec!["GARBAGE_RESPONSE".to_string()]);
}

#[given("a switch device with no mock responses")]
fn switch_device_with_no_responses(world: &mut PpbaWorld) {
    world.build_switch_device_with_responses(vec![]);
}

// ============================================================================
// When steps
// ============================================================================

#[when("I connect the switch device")]
async fn connect_switch_device(world: &mut PpbaWorld) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    device.set_connected(true).await.unwrap();
}

#[when("I disconnect the switch device")]
async fn disconnect_switch_device(world: &mut PpbaWorld) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    device.set_connected(false).await.unwrap();
}

#[when("I try to connect the switch device")]
async fn try_connect_switch_device(world: &mut PpbaWorld) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.set_connected(true).await {
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

#[when("I connect the OC device")]
async fn connect_oc_device(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    device.set_connected(true).await.unwrap();
}

#[when("I disconnect the OC device")]
async fn disconnect_oc_device(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    device.set_connected(false).await.unwrap();
}

#[when("I try to connect the OC device")]
async fn try_connect_oc_device(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.set_connected(true).await {
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

#[when(expr = "I cycle the switch device connection {int} times")]
async fn cycle_switch_device_connection(world: &mut PpbaWorld, count: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for _ in 0..count {
        device.set_connected(true).await.unwrap();
        assert!(device.connected().await.unwrap());
        device.set_connected(false).await.unwrap();
        assert!(!device.connected().await.unwrap());
    }
}

#[when("I connect the serial manager")]
async fn connect_serial_manager(world: &mut PpbaWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    manager.connect().await.unwrap();
}

#[when("I disconnect the serial manager")]
async fn disconnect_serial_manager(world: &mut PpbaWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    manager.disconnect().await;
}

#[when("I try to connect the serial manager")]
async fn try_connect_serial_manager(world: &mut PpbaWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    match manager.connect().await {
        Ok(()) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
        }
    }
}

// ============================================================================
// Then steps
// ============================================================================

#[then("the switch device should be disconnected")]
async fn switch_device_should_be_disconnected(world: &mut PpbaWorld) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(!device.connected().await.unwrap());
}

#[then("the switch device should be connected")]
async fn switch_device_should_be_connected(world: &mut PpbaWorld) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(device.connected().await.unwrap());
}

#[then("the OC device should be disconnected")]
async fn oc_device_should_be_disconnected(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    assert!(!device.connected().await.unwrap());
}

#[then("the OC device should be connected")]
async fn oc_device_should_be_connected(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    assert!(device.connected().await.unwrap());
}

#[then("the serial manager should be available")]
fn manager_should_be_available(world: &mut PpbaWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    assert!(manager.is_available());
}

#[then("the serial manager should not be available")]
fn manager_should_not_be_available(world: &mut PpbaWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    assert!(!manager.is_available());
}

#[then(expr = "the serial manager debug representation should contain {string}")]
fn manager_debug_contains(world: &mut PpbaWorld, expected: String) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let debug_str = format!("{:?}", manager);
    assert!(
        debug_str.contains(&expected),
        "expected debug to contain '{}', got: {}",
        expected,
        debug_str
    );
}

#[then("the last operation should have failed")]
fn last_operation_should_have_failed(world: &mut PpbaWorld) {
    assert!(
        world.last_error.is_some(),
        "expected an error but none occurred"
    );
}

#[then(expr = "the last error code should be {word}")]
fn last_error_code_should_be(world: &mut PpbaWorld, expected_code: String) {
    let actual_code = world
        .last_error_code
        .expect("expected an error code but none was set");
    let expected = match expected_code.as_str() {
        "NOT_CONNECTED" => ASCOMErrorCode::NOT_CONNECTED.raw(),
        "INVALID_VALUE" => ASCOMErrorCode::INVALID_VALUE.raw(),
        "INVALID_OPERATION" => ASCOMErrorCode::INVALID_OPERATION.raw(),
        "NOT_IMPLEMENTED" => ASCOMErrorCode::NOT_IMPLEMENTED.raw(),
        "VALUE_NOT_SET" => ASCOMErrorCode::VALUE_NOT_SET.raw(),
        other => panic!("Unknown ASCOM error code: {}", other),
    };
    assert_eq!(
        actual_code, expected,
        "expected error code {} ({}), got {} for error: {:?}",
        expected_code, expected, actual_code, world.last_error
    );
}
