//! Step definitions for focuser_readings.feature

use crate::world::QhyFocuserWorld;
use ascom_alpaca::api::Focuser;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

// ============================================================================
// When steps
// ============================================================================

#[when("I try to read the position")]
async fn try_read_position(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    match device.position().await {
        Ok(pos) => {
            world.position_result = Some(pos);
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read the temperature")]
async fn try_read_temperature(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    match device.temperature().await {
        Ok(temp) => {
            world.temperature_result = Some(temp);
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read is-moving")]
async fn try_read_is_moving(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    match device.is_moving().await {
        Ok(moving) => {
            world.is_moving_result = Some(moving);
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

#[then(expr = "the position should be {int}")]
async fn position_should_be(world: &mut QhyFocuserWorld, expected: i32) {
    let device = world.device.as_ref().expect("device not created");
    let position = device.position().await.unwrap();
    assert_eq!(position, expected);
}

#[then(expr = "the temperature should be approximately {float}")]
async fn temperature_should_be_approx(world: &mut QhyFocuserWorld, expected: f64) {
    let device = world.device.as_ref().expect("device not created");
    let temp = device.temperature().await.unwrap();
    assert!(
        (temp - expected).abs() < 0.001,
        "expected temperature ~{}, got {}",
        expected,
        temp
    );
}

#[then("the focuser should not be moving")]
async fn focuser_should_not_be_moving(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    assert!(!device.is_moving().await.unwrap());
}

#[then("the operation should fail with not-connected")]
fn operation_should_fail_not_connected(world: &mut QhyFocuserWorld) {
    let code = world
        .last_error_code
        .expect("expected an error but none occurred");
    assert_eq!(
        code,
        ASCOMErrorCode::NOT_CONNECTED.raw(),
        "expected NOT_CONNECTED error code, got: {}",
        code
    );
}

#[then(expr = "the cached position should be {int}")]
async fn cached_position_should_be(world: &mut QhyFocuserWorld, expected: i64) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    assert_eq!(state.position, Some(expected));
}

#[then(expr = "the cached outer temperature should be approximately {float}")]
async fn cached_outer_temp_approx(world: &mut QhyFocuserWorld, expected: f64) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    let temp = state.outer_temp.unwrap();
    assert!(
        (temp - expected).abs() < 0.001,
        "expected ~{}, got {}",
        expected,
        temp
    );
}

#[then(expr = "the cached chip temperature should be approximately {float}")]
async fn cached_chip_temp_approx(world: &mut QhyFocuserWorld, expected: f64) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    let temp = state.chip_temp.unwrap();
    assert!(
        (temp - expected).abs() < 0.001,
        "expected ~{}, got {}",
        expected,
        temp
    );
}

#[then(expr = "the cached voltage should be approximately {float}")]
async fn cached_voltage_approx(world: &mut QhyFocuserWorld, expected: f64) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    let voltage = state.voltage.unwrap();
    assert!(
        (voltage - expected).abs() < 0.001,
        "expected ~{}, got {}",
        expected,
        voltage
    );
}

#[then(expr = "the cached firmware version should be {string}")]
async fn cached_firmware_version(world: &mut QhyFocuserWorld, expected: String) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    assert_eq!(state.firmware_version, Some(expected));
}

#[then(expr = "the cached board version should be {string}")]
async fn cached_board_version(world: &mut QhyFocuserWorld, expected: String) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    assert_eq!(state.board_version, Some(expected));
}

#[then(expr = "the cached is-moving should be {word}")]
async fn cached_is_moving(world: &mut QhyFocuserWorld, expected: String) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    let expected_bool = expected == "true";
    assert_eq!(
        state.is_moving, expected_bool,
        "expected is_moving={}, got {}",
        expected_bool, state.is_moving
    );
}

#[then("the cached position should be empty")]
async fn cached_position_empty(world: &mut QhyFocuserWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    assert_eq!(state.position, None);
}

#[then("the cached outer temperature should be empty")]
async fn cached_outer_temp_empty(world: &mut QhyFocuserWorld) {
    let manager = world.serial_manager.as_ref().expect("manager not created");
    let state = manager.get_cached_state().await;
    assert_eq!(state.outer_temp, None);
}
