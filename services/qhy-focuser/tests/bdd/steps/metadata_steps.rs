//! Step definitions for device_metadata.feature

use crate::world::mock_serial;
use crate::world::QhyFocuserWorld;
use ascom_alpaca::api::{Device, Focuser};
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{given, then, when};
use qhy_focuser::Config;

// ============================================================================
// Given steps
// ============================================================================

#[given(expr = "a focuser device with name {string}")]
fn device_with_name(world: &mut QhyFocuserWorld, name: String) {
    let mut config = Config::default();
    config.focuser.name = name;
    world.config = Some(config);
    world.build_device_with_responses(mock_serial::standard_connection_responses());
}

#[given(expr = "a focuser device with unique ID {string}")]
fn device_with_unique_id(world: &mut QhyFocuserWorld, unique_id: String) {
    let mut config = Config::default();
    config.focuser.unique_id = unique_id;
    world.config = Some(config);
    world.build_device_with_responses(mock_serial::standard_connection_responses());
}

#[given(expr = "a focuser device with description {string}")]
fn device_with_description(world: &mut QhyFocuserWorld, description: String) {
    let mut config = Config::default();
    config.focuser.description = description;
    world.config = Some(config);
    world.build_device_with_responses(mock_serial::standard_connection_responses());
}

#[given(expr = "a focuser device with max step {int}")]
fn device_with_max_step(world: &mut QhyFocuserWorld, max_step: u32) {
    let mut config = Config::default();
    config.focuser.max_step = max_step;
    world.config = Some(config);
    world.build_device_with_responses(mock_serial::standard_connection_responses());
}

// ============================================================================
// When steps
// ============================================================================

#[when("I try to enable temperature compensation")]
async fn try_enable_temp_comp(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    match device.set_temp_comp(true).await {
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

#[when("I try to read step size")]
async fn try_read_step_size(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    match device.step_size().await {
        Ok(_) => {
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

#[then(expr = "the device name should be {string}")]
fn device_name_should_be(world: &mut QhyFocuserWorld, expected: String) {
    let device = world.device.as_ref().expect("device not created");
    assert_eq!(device.static_name(), expected);
}

#[then(expr = "the device unique ID should be {string}")]
fn device_unique_id_should_be(world: &mut QhyFocuserWorld, expected: String) {
    let device = world.device.as_ref().expect("device not created");
    assert_eq!(device.unique_id(), expected);
}

#[then(expr = "the device description should be {string}")]
async fn device_description_should_be(world: &mut QhyFocuserWorld, expected: String) {
    let device = world.device.as_ref().expect("device not created");
    let description = device.description().await.unwrap();
    assert_eq!(description, expected);
}

#[then(expr = "the driver info should contain {string}")]
async fn driver_info_should_contain(world: &mut QhyFocuserWorld, expected: String) {
    let device = world.device.as_ref().expect("device not created");
    let info = device.driver_info().await.unwrap();
    assert!(
        info.contains(&expected),
        "expected driver info to contain '{}', got: {}",
        expected,
        info
    );
}

#[then("the driver version should not be empty")]
async fn driver_version_not_empty(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    let version = device.driver_version().await.unwrap();
    assert!(!version.is_empty());
}

#[then("the focuser should be absolute")]
async fn focuser_should_be_absolute(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    assert!(device.absolute().await.unwrap());
}

#[then(expr = "the max step should be {int}")]
async fn max_step_should_be(world: &mut QhyFocuserWorld, expected: u32) {
    let device = world.device.as_ref().expect("device not created");
    assert_eq!(device.max_step().await.unwrap(), expected);
}

#[then(expr = "the max increment should be {int}")]
async fn max_increment_should_be(world: &mut QhyFocuserWorld, expected: u32) {
    let device = world.device.as_ref().expect("device not created");
    assert_eq!(device.max_increment().await.unwrap(), expected);
}

#[then("temperature compensation should not be available")]
async fn temp_comp_not_available(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    assert!(!device.temp_comp_available().await.unwrap());
}

#[then("temperature compensation should be off")]
async fn temp_comp_should_be_off(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    assert!(!device.temp_comp().await.unwrap());
}

#[then("the device debug representation should not be empty")]
fn device_debug_not_empty(world: &mut QhyFocuserWorld) {
    let device = world.device.as_ref().expect("device not created");
    let debug_str = format!("{:?}", device);
    assert!(!debug_str.is_empty());
}

#[then("the operation should fail with not-implemented")]
fn operation_should_fail_not_implemented(world: &mut QhyFocuserWorld) {
    let code = world
        .last_error_code
        .expect("expected an error but none occurred");
    assert_eq!(
        code,
        ASCOMErrorCode::NOT_IMPLEMENTED.raw(),
        "expected NOT_IMPLEMENTED error code, got: {}",
        code
    );
}
