//! Exposure-lifecycle steps. The `StartExposure` step is shared with the
//! binning/ROI feature (the same phrasing drives both the happy path and the
//! geometry-rejection scenarios).

use ascom_alpaca::api::camera::CameraState;
use cucumber::{then, when};

use crate::world::CameraWorld;

#[when(
    regex = r"^I (?:try to )?StartExposure on camera device (\d+) with BinX (\d+) BinY (\d+) NumX (\d+) NumY (\d+) StartX (\d+) StartY (\d+) Duration (-?[0-9.]+) Light (true|false)$"
)]
#[allow(clippy::too_many_arguments)]
async fn start_exposure(
    world: &mut CameraWorld,
    _device: u32,
    bin_x: u8,
    bin_y: u8,
    num_x: u32,
    num_y: u32,
    start_x: u32,
    start_y: u32,
    duration: f64,
    light: bool,
) {
    world
        .try_start_exposure(
            bin_x, bin_y, num_x, num_y, start_x, start_y, duration, light,
        )
        .await;
}

// `And the exposure ... completes` follows a `When`, so it is When-typed; also
// registered as Then for robustness.
#[when(regex = r"^the exposure on camera device (\d+) completes$")]
#[then(regex = r"^the exposure on camera device (\d+) completes$")]
async fn exposure_completes(world: &mut CameraWorld, _device: u32) {
    world.wait_image_ready().await;
}

#[then(regex = r"^camera device (\d+) returns an ImageArray of (\d+) by (\d+)$")]
async fn image_array_dims(world: &mut CameraWorld, _device: u32, width: usize, height: usize) {
    let camera = world.camera();
    let image = camera.image_array().await.unwrap();
    assert_eq!(image.dim().0, width, "ImageArray width");
    assert_eq!(image.dim().1, height, "ImageArray height");
}

#[then(regex = r"^camera device (\d+) reports a set LastExposureStartTime$")]
async fn last_exposure_start_time_set(world: &mut CameraWorld, _device: u32) {
    world.camera().last_exposure_start_time().await.unwrap();
}

#[then(regex = r"^camera device (\d+) reports LastExposureDuration as ([0-9.]+)$")]
async fn last_exposure_duration(world: &mut CameraWorld, _device: u32, expected: f64) {
    let camera = world.camera();
    let actual = camera.last_exposure_duration().await.unwrap().as_secs_f64();
    assert!(
        (actual - expected).abs() < 1e-6,
        "LastExposureDuration {actual} != {expected}"
    );
}

#[then(regex = r"^camera device (\d+) reports CameraState as (\w+)$")]
async fn reports_camera_state(world: &mut CameraWorld, _device: u32, state: String) {
    let expected = match state.as_str() {
        "Idle" => CameraState::Idle,
        "Exposing" => CameraState::Exposing,
        "Error" => CameraState::Error,
        other => panic!("unknown CameraState: {other}"),
    };
    assert_eq!(world.camera().camera_state().await.unwrap(), expected);
}

#[then(regex = r"^camera device (\d+) reports PercentCompleted as (\d+)$")]
async fn reports_percent(world: &mut CameraWorld, _device: u32, expected: u8) {
    assert_eq!(world.camera().percent_completed().await.unwrap(), expected);
}

#[when(regex = r"^I abort the exposure on camera device (\d+)$")]
async fn abort_exposure(world: &mut CameraWorld, _device: u32) {
    world.camera().abort_exposure().await.unwrap();
}

#[when(regex = r"^I try to StopExposure on camera device (\d+)$")]
async fn try_stop_exposure(world: &mut CameraWorld, _device: u32) {
    world.last_error_code = world
        .camera()
        .stop_exposure()
        .await
        .err()
        .map(|e| e.code.raw());
}
