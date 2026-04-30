//! Step stubs for `cancellation.feature`.
//! Phase-2: bodies are `todo!()` so scenarios fail loudly until phase 3.

use crate::world::SkySurveyCameraWorld;
use cucumber::{then, when};

#[when("I AbortExposure")]
async fn abort_exposure(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: PUT /api/v1/camera/0/abortexposure");
}

#[when("I StopExposure")]
async fn stop_exposure(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: PUT /api/v1/camera/0/stopexposure");
}

#[then("ImageReady is false")]
async fn image_ready_false(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: GET /api/v1/camera/0/imageready returns false");
}

#[then("the cancellation succeeds")]
fn cancellation_succeeds(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: assert world.last_ascom_error.is_none()");
}
