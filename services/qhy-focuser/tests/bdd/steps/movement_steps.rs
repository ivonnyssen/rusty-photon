//! Step definitions for movement_control.feature

use crate::world::mock_serial;
use crate::world::QhyFocuserWorld;
use ascom_alpaca::api::Focuser;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a focuser device with standard mock responses and a move response")]
fn device_with_move_response(world: &mut QhyFocuserWorld) {
    let mut responses = mock_serial::standard_connection_responses();
    // AbsoluteMove response
    responses.push(r#"{"idx": 6}"#.to_string());
    // Extra polling
    responses.push(r#"{"idx": 5, "pos": 10000}"#.to_string());
    responses.push(r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string());
    world.build_device_with_responses(responses);
}

#[given("a focuser device with standard mock responses and move then abort responses")]
fn device_with_move_and_abort_responses(world: &mut QhyFocuserWorld) {
    let mut responses = mock_serial::standard_connection_responses();
    // AbsoluteMove response
    responses.push(r#"{"idx": 6}"#.to_string());
    // Abort response
    responses.push(r#"{"idx": 3}"#.to_string());
    // Extra polling
    responses.push(r#"{"idx": 5, "pos": 15000}"#.to_string());
    responses.push(r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string());
    world.build_device_with_responses(responses);
}

#[given(expr = "a serial manager with responses for move then position-at-target {int}")]
fn manager_with_move_and_target_reached(world: &mut QhyFocuserWorld, target: i64) {
    let mut responses = mock_serial::standard_connection_responses();
    // AbsoluteMove response
    responses.push(r#"{"idx": 6}"#.to_string());
    // refresh_position GetPosition response — position matches target
    responses.push(format!(r#"{{"idx": 5, "pos": {}}}"#, target));
    // Extra polling
    responses.push(format!(r#"{{"idx": 5, "pos": {}}}"#, target));
    responses.push(r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string());
    world.build_manager_with_responses(responses);
}

#[given("a serial manager with standard mock responses and a set-speed response")]
fn manager_with_set_speed_response(world: &mut QhyFocuserWorld) {
    let mut responses = mock_serial::standard_connection_responses();
    responses.push(r#"{"idx": 13}"#.to_string());
    responses.push(r#"{"idx": 5, "pos": 10000}"#.to_string());
    responses.push(r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string());
    world.build_manager_with_responses(responses);
}

#[given("a serial manager with standard mock responses and a set-reverse response")]
fn manager_with_set_reverse_response(world: &mut QhyFocuserWorld) {
    let mut responses = mock_serial::standard_connection_responses();
    responses.push(r#"{"idx": 7}"#.to_string());
    responses.push(r#"{"idx": 5, "pos": 10000}"#.to_string());
    responses.push(r#"{"idx": 4, "o_t": 25000, "c_t": 30000, "c_r": 125}"#.to_string());
    world.build_manager_with_responses(responses);
}

// ============================================================================
// When steps
// ============================================================================

#[when(expr = "I move the focuser to position {int}")]
async fn move_focuser(world: &mut QhyFocuserWorld, position: i32) {
    let device = world.device.as_ref().expect("device not created");
    device.move_(position).await.unwrap();
}

#[when(expr = "I try to move the focuser to position {int}")]
async fn try_move_focuser(world: &mut QhyFocuserWorld, position: i32) {
    let device = world.device.as_ref().expect("device not created");
    match device.move_(position).await {
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

#[when("I halt the focuser")]
async fn halt_focuser(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    device.halt().await.unwrap();
}

#[when("I try to halt the focuser")]
async fn try_halt_focuser(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    match device.halt().await {
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

#[when(expr = "I send a move-absolute command to {int}")]
async fn send_move_absolute(world: &mut QhyFocuserWorld, position: i64) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    manager.move_absolute(position).await.unwrap();
}

#[when("I refresh the position")]
async fn refresh_position(world: &mut QhyFocuserWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    manager.refresh_position().await.unwrap();
}

#[when(expr = "I set the speed to {int}")]
async fn set_speed(world: &mut QhyFocuserWorld, speed: u8) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    manager.set_speed(speed).await.unwrap();
}

#[when(expr = "I try to set the speed to {int}")]
async fn try_set_speed(world: &mut QhyFocuserWorld, speed: u8) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    match manager.set_speed(speed).await {
        Ok(()) => world.last_error = None,
        Err(e) => world.last_error = Some(format!("{:?}", e)),
    }
}

#[when(expr = "I set reverse to {word}")]
async fn set_reverse(world: &mut QhyFocuserWorld, enabled: String) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    manager.set_reverse(enabled == "true").await.unwrap();
}

#[when(expr = "I try to set reverse to {word}")]
async fn try_set_reverse(world: &mut QhyFocuserWorld, enabled: String) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    match manager.set_reverse(enabled == "true").await {
        Ok(()) => world.last_error = None,
        Err(e) => world.last_error = Some(format!("{:?}", e)),
    }
}

// ============================================================================
// Then steps
// ============================================================================

#[then("the operation should fail with invalid-value")]
fn operation_should_fail_invalid_value(world: &mut QhyFocuserWorld) {
    let code = world
        .last_error_code
        .expect("expected an error but none occurred");
    assert_eq!(
        code,
        ASCOMErrorCode::INVALID_VALUE.raw(),
        "expected INVALID_VALUE error code, got: {}",
        code
    );
}

#[then(expr = "the cached target position should be {int}")]
async fn cached_target_position(world: &mut QhyFocuserWorld, expected: i64) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    assert_eq!(state.target_position, Some(expected));
}

#[then("the cached target position should be empty")]
async fn cached_target_position_empty(world: &mut QhyFocuserWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    assert_eq!(state.target_position, None);
}

#[then("the operation should succeed")]
fn operation_should_succeed(world: &mut QhyFocuserWorld) {
    assert!(
        world.last_error.is_none(),
        "expected success but got error: {:?}",
        world.last_error
    );
}

#[then("the serial manager operation should fail with not-connected")]
fn manager_operation_should_fail_not_connected(world: &mut QhyFocuserWorld) {
    let error = world
        .last_error
        .as_ref()
        .expect("expected an error but none occurred");
    assert!(
        error.contains("NotConnected"),
        "expected NotConnected error, got: {}",
        error
    );
}
