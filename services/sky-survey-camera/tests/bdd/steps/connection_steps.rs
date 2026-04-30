//! Step stubs for `connection_lifecycle.feature`.
//! Phase-2: bodies are `todo!()` so scenarios fail loudly until phase 3.

use crate::world::SkySurveyCameraWorld;
use cucumber::{given, then, when};

#[given("a sky-survey-camera with default optics")]
fn default_optics(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: populate world.* with default optics values");
}

#[given(expr = "a writable cache directory")]
fn writable_cache_dir(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: ensure world.temp_dir holds a writable cache dir");
}

#[given(expr = "a non-writable cache directory")]
fn non_writable_cache_dir(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: arrange a cache dir whose parent permissions cause mkdir to fail");
}

#[given("SkyView is reachable")]
async fn skyview_reachable(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: point survey backend at a healthy stub");
}

#[given("SkyView is unreachable")]
async fn skyview_unreachable(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: point survey backend at an unroutable address");
}

#[when("I start the service")]
async fn start_service(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: write config + start ServiceHandle");
}

#[when("I connect the camera")]
async fn connect_camera(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: PUT /api/v1/camera/0/connected with Connected=true");
}

#[when("I try to connect the camera")]
async fn try_connect_camera(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: capture failure into world.last_ascom_error");
}

#[when("I disconnect the camera")]
async fn disconnect_camera(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: PUT /api/v1/camera/0/connected with Connected=false");
}

#[then("the camera is connected")]
async fn camera_is_connected(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: GET /api/v1/camera/0/connected returns true");
}

#[then("the camera is not connected")]
async fn camera_is_not_connected(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: GET /api/v1/camera/0/connected returns false");
}

#[then("the connect attempt fails with ASCOM UNSPECIFIED_ERROR")]
fn connect_fails_unspecified(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: assert world.last_ascom_error == 0x500");
}
