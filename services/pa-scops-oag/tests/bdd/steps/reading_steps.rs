//! Step definitions for focuser_readings.feature

use crate::world::ScopsWorld;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

#[when("I try to read the position")]
async fn try_read_position(world: &mut ScopsWorld) {
    match world.focuser().position().await {
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
async fn try_read_temperature(world: &mut ScopsWorld) {
    match world.focuser().temperature().await {
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
async fn try_read_is_moving(world: &mut ScopsWorld) {
    match world.focuser().is_moving().await {
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

#[then(expr = "the position should be {int}")]
async fn position_should_be(world: &mut ScopsWorld, expected: i32) {
    let position = world.focuser().position().await.unwrap();
    assert_eq!(position, expected);
}

#[then("the focuser should not be moving")]
async fn focuser_should_not_be_moving(world: &mut ScopsWorld) {
    assert!(!world.focuser().is_moving().await.unwrap());
}

#[then("the focuser should be moving")]
async fn focuser_should_be_moving(world: &mut ScopsWorld) {
    assert!(world.focuser().is_moving().await.unwrap());
}

#[then("the operation should fail with not-connected")]
fn operation_should_fail_not_connected(world: &mut ScopsWorld) {
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
