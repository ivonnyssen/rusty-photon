use ascom_alpaca::api::CoverCalibrator;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

use crate::world::Fp2World;

#[when(regex = r"^calibrator_on is called with brightness (\d+)$")]
async fn calibrator_on_succeeds(world: &mut Fp2World, brightness: u32) {
    world
        .device()
        .calibrator_on(brightness)
        .await
        .expect("calibrator_on should succeed");
}

#[when(regex = r"^calibrator_on is called with brightness (\d+) and the call is captured$")]
async fn calibrator_on_capture(world: &mut Fp2World, brightness: u32) {
    world.last_error = world.device().calibrator_on(brightness).await.err();
}

#[when("calibrator_off is called")]
async fn calibrator_off_succeeds(world: &mut Fp2World) {
    world
        .device()
        .calibrator_off()
        .await
        .expect("calibrator_off should succeed");
}

#[when("calibrator_off is called and the call is captured")]
async fn calibrator_off_capture(world: &mut Fp2World) {
    world.last_error = world.device().calibrator_off().await.err();
}

#[then(regex = r"^brightness should be (\d+)$")]
async fn assert_brightness(world: &mut Fp2World, expected: u32) {
    let actual = world.device().brightness().await.unwrap();
    assert_eq!(actual, expected);
}

#[then(regex = r"^max_brightness should be (\d+)$")]
async fn assert_max_brightness(world: &mut Fp2World, expected: u32) {
    let actual = world.device().max_brightness().await.unwrap();
    assert_eq!(actual, expected);
}

#[then(regex = r"^the simulator brightness should be (\d+)$")]
async fn assert_simulator_brightness(world: &mut Fp2World, expected: u16) {
    assert_eq!(world.factory().state().brightness().await, expected);
}

#[then("the simulator light should be on")]
async fn assert_simulator_light_on(world: &mut Fp2World) {
    assert!(world.factory().state().light_on().await);
}

#[then("the simulator light should be off")]
async fn assert_simulator_light_off(world: &mut Fp2World) {
    assert!(!world.factory().state().light_on().await);
}

#[then("the call should fail with an invalid-value error")]
async fn assert_call_failed_invalid_value(world: &mut Fp2World) {
    let err = world
        .last_error
        .take()
        .expect("expected an error from the call");
    assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
}
