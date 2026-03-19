use ascom_alpaca::api::camera::{CameraState, SensorType};
use ascom_alpaca::api::{Camera, Device};
use cucumber::{then, when};

use crate::world::QhyCameraWorld;

// --- When ---

#[when(expr = "I set bin_x to {int}")]
async fn set_bin_x(world: &mut QhyCameraWorld, value: u8) {
    let camera = world.camera.as_ref().unwrap();
    camera.set_bin_x(value).await.unwrap();
}

#[when(expr = "I try to set bin_x to {int}")]
async fn try_set_bin_x(world: &mut QhyCameraWorld, value: u8) {
    let camera = world.camera.as_ref().unwrap();
    match camera.set_bin_x(value).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

#[when(expr = "I set start_x to {int}")]
async fn set_start_x(world: &mut QhyCameraWorld, value: u32) {
    let camera = world.camera.as_ref().unwrap();
    camera.set_start_x(value).await.unwrap();
}

#[when(expr = "I set num_x to {int}")]
async fn set_num_x(world: &mut QhyCameraWorld, value: u32) {
    let camera = world.camera.as_ref().unwrap();
    camera.set_num_x(value).await.unwrap();
}

#[when(expr = "I set gain to {int}")]
async fn set_gain(world: &mut QhyCameraWorld, value: i32) {
    let camera = world.camera.as_ref().unwrap();
    camera.set_gain(value).await.unwrap();
}

#[when(expr = "I try to set gain to {int}")]
async fn try_set_gain(world: &mut QhyCameraWorld, value: i32) {
    let camera = world.camera.as_ref().unwrap();
    match camera.set_gain(value).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

#[when("I try to read camera_x_size")]
async fn try_read_camera_x_size(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    match camera.camera_x_size().await {
        Ok(_) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

// --- Then ---

#[then(expr = "camera_x_size should be {int}")]
async fn check_camera_x_size(world: &mut QhyCameraWorld, expected: u32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.camera_x_size().await.unwrap(), expected);
}

#[then(expr = "camera_y_size should be {int}")]
async fn check_camera_y_size(world: &mut QhyCameraWorld, expected: u32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.camera_y_size().await.unwrap(), expected);
}

#[then(expr = "pixel_size_x should be {float}")]
async fn check_pixel_size_x(world: &mut QhyCameraWorld, expected: f64) {
    let camera = world.camera.as_ref().unwrap();
    let actual = camera.pixel_size_x().await.unwrap();
    assert!(
        (actual - expected).abs() < 0.01,
        "expected pixel_size_x={}, got {}",
        expected,
        actual
    );
}

#[then(expr = "pixel_size_y should be {float}")]
async fn check_pixel_size_y(world: &mut QhyCameraWorld, expected: f64) {
    let camera = world.camera.as_ref().unwrap();
    let actual = camera.pixel_size_y().await.unwrap();
    assert!(
        (actual - expected).abs() < 0.01,
        "expected pixel_size_y={}, got {}",
        expected,
        actual
    );
}

#[then(expr = "bin_x should be {int}")]
async fn check_bin_x(world: &mut QhyCameraWorld, expected: u8) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.bin_x().await.unwrap(), expected);
}

#[then(expr = "bin_y should be {int}")]
async fn check_bin_y(world: &mut QhyCameraWorld, expected: u8) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.bin_y().await.unwrap(), expected);
}

#[then(expr = "max_bin_x should be {int}")]
async fn check_max_bin_x(world: &mut QhyCameraWorld, expected: u8) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.max_bin_x().await.unwrap(), expected);
}

#[then(expr = "max_bin_y should be {int}")]
async fn check_max_bin_y(world: &mut QhyCameraWorld, expected: u8) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.max_bin_y().await.unwrap(), expected);
}

#[then(expr = "start_x should be {int}")]
async fn check_start_x(world: &mut QhyCameraWorld, expected: u32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.start_x().await.unwrap(), expected);
}

#[then(expr = "start_y should be {int}")]
async fn check_start_y(world: &mut QhyCameraWorld, expected: u32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.start_y().await.unwrap(), expected);
}

#[then(expr = "num_x should be {int}")]
async fn check_num_x(world: &mut QhyCameraWorld, expected: u32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.num_x().await.unwrap(), expected);
}

#[then(expr = "num_y should be {int}")]
async fn check_num_y(world: &mut QhyCameraWorld, expected: u32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.num_y().await.unwrap(), expected);
}

#[then(expr = "gain_min should be {int}")]
async fn check_gain_min(world: &mut QhyCameraWorld, expected: i32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.gain_min().await.unwrap(), expected);
}

#[then(expr = "gain_max should be {int}")]
async fn check_gain_max(world: &mut QhyCameraWorld, expected: i32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.gain_max().await.unwrap(), expected);
}

#[then(expr = "gain should be {int}")]
async fn check_gain(world: &mut QhyCameraWorld, expected: i32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.gain().await.unwrap(), expected);
}

#[then(expr = "offset_min should be {int}")]
async fn check_offset_min(world: &mut QhyCameraWorld, expected: i32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.offset_min().await.unwrap(), expected);
}

#[then(expr = "offset_max should be {int}")]
async fn check_offset_max(world: &mut QhyCameraWorld, expected: i32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.offset_max().await.unwrap(), expected);
}

#[then("exposure_min should be available")]
async fn check_exposure_min(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    camera.exposure_min().await.unwrap();
}

#[then("exposure_max should be available")]
async fn check_exposure_max(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    camera.exposure_max().await.unwrap();
}

#[then("exposure_resolution should be available")]
async fn check_exposure_resolution(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    camera.exposure_resolution().await.unwrap();
}

#[then("camera_state should be idle")]
async fn check_camera_state_idle(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.camera_state().await.unwrap(), CameraState::Idle);
}

#[then("camera_state should be exposing")]
async fn check_camera_state_exposing(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.camera_state().await.unwrap(), CameraState::Exposing);
}

#[then(expr = "can_abort_exposure should be {word}")]
async fn check_can_abort(world: &mut QhyCameraWorld, expected: String) {
    let camera = world.camera.as_ref().unwrap();
    let expected = expected == "true";
    assert_eq!(camera.can_abort_exposure().await.unwrap(), expected);
}

#[then(expr = "can_stop_exposure should be {word}")]
async fn check_can_stop(world: &mut QhyCameraWorld, expected: String) {
    let camera = world.camera.as_ref().unwrap();
    let expected = expected == "true";
    assert_eq!(camera.can_stop_exposure().await.unwrap(), expected);
}

#[then(expr = "has_shutter should be {word}")]
async fn check_has_shutter(world: &mut QhyCameraWorld, expected: String) {
    let camera = world.camera.as_ref().unwrap();
    let expected = expected == "true";
    assert_eq!(camera.has_shutter().await.unwrap(), expected);
}

#[then("sensor_type should be monochrome")]
async fn check_sensor_type_mono(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.sensor_type().await.unwrap(), SensorType::Monochrome);
}

#[then(expr = "sensor_name should be {string}")]
async fn check_sensor_name(world: &mut QhyCameraWorld, expected: String) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.sensor_name().await.unwrap(), expected);
}

#[then(expr = "readout_modes should have {int} entries")]
async fn check_readout_modes_count(world: &mut QhyCameraWorld, expected: usize) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.readout_modes().await.unwrap().len(), expected);
}

#[then(expr = "can_fast_readout should be {word}")]
async fn check_can_fast_readout(world: &mut QhyCameraWorld, expected: String) {
    let camera = world.camera.as_ref().unwrap();
    let expected = expected == "true";
    assert_eq!(camera.can_fast_readout().await.unwrap(), expected);
}

#[then(expr = "can_set_ccd_temperature should be {word}")]
async fn check_can_set_ccd_temp(world: &mut QhyCameraWorld, expected: String) {
    let camera = world.camera.as_ref().unwrap();
    let expected = expected == "true";
    assert_eq!(camera.can_set_ccd_temperature().await.unwrap(), expected);
}

#[then(expr = "can_get_cooler_power should be {word}")]
async fn check_can_get_cooler_power(world: &mut QhyCameraWorld, expected: String) {
    let camera = world.camera.as_ref().unwrap();
    let expected = expected == "true";
    assert_eq!(camera.can_get_cooler_power().await.unwrap(), expected);
}

#[then(expr = "max_adu should be {int}")]
async fn check_max_adu(world: &mut QhyCameraWorld, expected: u32) {
    let camera = world.camera.as_ref().unwrap();
    assert_eq!(camera.max_adu().await.unwrap(), expected);
}

#[then(expr = "driver_info should contain {string}")]
async fn check_driver_info(world: &mut QhyCameraWorld, expected: String) {
    let camera = world.camera.as_ref().unwrap();
    let info = camera.driver_info().await.unwrap();
    assert!(
        info.contains(&expected),
        "driver_info '{}' should contain '{}'",
        info,
        expected
    );
}

#[then("driver_version should not be empty")]
async fn check_driver_version(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    let version = camera.driver_version().await.unwrap();
    assert!(!version.is_empty(), "driver_version should not be empty");
}
