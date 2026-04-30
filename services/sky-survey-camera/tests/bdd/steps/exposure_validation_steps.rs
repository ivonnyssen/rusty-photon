//! Step stubs for `exposure_validation.feature`.
//! Phase-2: bodies are `todo!()` so scenarios fail loudly until phase 3.

use crate::world::SkySurveyCameraWorld;
use cucumber::{given, then, when};

#[given("the camera is connected with the survey backend stubbed")]
async fn connected_stubbed(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: start service with mock SurveyClient + connect");
}

#[given("an exposure is already in flight")]
async fn exposure_in_flight(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: kick off StartExposure that does not yet complete");
}

#[when(
    expr = "I StartExposure with BinX {int} BinY {int} NumX {int} NumY {int} StartX {int} StartY {int} Duration {float}"
)]
#[allow(clippy::too_many_arguments)]
async fn start_exposure(
    _world: &mut SkySurveyCameraWorld,
    _bin_x: i32,
    _bin_y: i32,
    _num_x: i32,
    _num_y: i32,
    _start_x: i32,
    _start_y: i32,
    _duration_s: f64,
) {
    todo!("phase 3: PUT /api/v1/camera/0/startexposure with all parameters");
}

#[then("the exposure is rejected with ASCOM INVALID_VALUE")]
fn rejected_invalid_value(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: assert world.last_ascom_error == 0x401");
}

#[then("the exposure is rejected with ASCOM INVALID_OPERATION")]
fn rejected_invalid_operation(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: assert world.last_ascom_error == 0x40B");
}

#[then("the exposure is rejected with ASCOM NOT_CONNECTED")]
fn rejected_not_connected(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: assert world.last_ascom_error == 0x407");
}
