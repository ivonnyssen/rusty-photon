use std::time::Duration;

use ascom_alpaca::api::Camera;
use cucumber::{then, when};

use crate::world::QhyCameraWorld;

// --- When ---

#[when(expr = "I start a {float} second exposure")]
async fn start_exposure(world: &mut QhyCameraWorld, seconds: f64) {
    let camera = world.camera.as_ref().unwrap();
    camera
        .start_exposure(Duration::from_secs_f64(seconds), true)
        .await
        .unwrap();
}

#[when(expr = "I try to start a {float} second exposure")]
async fn try_start_exposure(world: &mut QhyCameraWorld, seconds: f64) {
    let camera = world.camera.as_ref().unwrap();
    match camera
        .start_exposure(Duration::from_secs_f64(seconds), true)
        .await
    {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

#[when("I try to start a dark frame exposure")]
async fn try_start_dark_frame(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    match camera.start_exposure(Duration::from_secs(1), false).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

#[when("I try to start another exposure")]
async fn try_start_another_exposure(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    match camera.start_exposure(Duration::from_secs(1), true).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

#[when("I wait for exposure to complete")]
async fn wait_for_exposure(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    // Poll image_ready with a timeout
    for _ in 0..100 {
        if camera.image_ready().await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("exposure did not complete within timeout");
}

#[when("I abort the exposure")]
async fn abort_exposure(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    camera.abort_exposure().await.unwrap();
    // Give the background task a moment to process
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// --- Then ---

#[then(expr = "image_ready should be {word}")]
async fn check_image_ready(world: &mut QhyCameraWorld, expected: String) {
    let camera = world.camera.as_ref().unwrap();
    let expected = expected == "true";
    assert_eq!(camera.image_ready().await.unwrap(), expected);
}

#[then("image_array should be available")]
async fn check_image_array(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    camera.image_array().await.unwrap();
}

#[then("last_exposure_duration should be available")]
async fn check_last_exposure_duration(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    camera.last_exposure_duration().await.unwrap();
}

#[then("last_exposure_start_time should be available")]
async fn check_last_exposure_start_time(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    camera.last_exposure_start_time().await.unwrap();
}
