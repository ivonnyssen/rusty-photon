//! Step definitions for calibrator_control.feature.

use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

use crate::world::Fp2World;

#[when(regex = r"^calibrator_on is called with brightness (\d+)$")]
async fn calibrator_on_succeeds(world: &mut Fp2World, brightness: u32) {
    world.device().calibrator_on(brightness).await.unwrap();
}

#[when(regex = r"^calibrator_on is called with brightness (\d+) and the call is captured$")]
async fn calibrator_on_capture(world: &mut Fp2World, brightness: u32) {
    world.last_error = world.device().calibrator_on(brightness).await.err();
}

#[when("calibrator_off is called")]
async fn calibrator_off_succeeds(world: &mut Fp2World) {
    world.device().calibrator_off().await.unwrap();
}

#[when("calibrator_off is called and the call is captured")]
async fn calibrator_off_capture(world: &mut Fp2World) {
    world.last_error = world.device().calibrator_off().await.err();
}

#[then(regex = r"^brightness should be (\d+)$")]
async fn assert_brightness(world: &mut Fp2World, expected: u32) {
    assert_eq!(world.device().brightness().await.unwrap(), expected);
}

#[then(regex = r"^max_brightness should be (\d+)$")]
async fn assert_max_brightness(world: &mut Fp2World, expected: u32) {
    assert_eq!(world.device().max_brightness().await.unwrap(), expected);
}

#[then("the call should fail with an invalid-value error")]
async fn assert_call_failed_invalid_value(world: &mut Fp2World) {
    let err = world
        .last_error
        .take()
        .expect("expected an error from the call");
    assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
}
