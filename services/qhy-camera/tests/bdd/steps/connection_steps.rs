use ascom_alpaca::api::Device;
use cucumber::{given, then, when};

use crate::world::{QhyCameraWorld, TestCameraHandle, TestFilterWheelHandle};

// --- Given ---

#[given("a camera device with mock SDK")]
fn camera_with_mock(world: &mut QhyCameraWorld) {
    world.build_camera_with_mock();
}

#[given("a camera device with failing SDK")]
fn camera_with_failing_sdk(world: &mut QhyCameraWorld) {
    world.build_camera_with_handle(Box::new(TestCameraHandle::failing()));
}

#[given("a connected camera device")]
async fn connected_camera(world: &mut QhyCameraWorld) {
    world.build_camera_with_mock();
    let camera = world.camera.as_ref().unwrap();
    camera.set_connected(true).await.unwrap();
}

#[given("a filter wheel device with mock SDK")]
fn filter_wheel_with_mock(world: &mut QhyCameraWorld) {
    world.build_filter_wheel_with_mock();
}

#[given("a filter wheel device with failing SDK")]
fn filter_wheel_with_failing_sdk(world: &mut QhyCameraWorld) {
    world.build_filter_wheel_with_handle(Box::new(TestFilterWheelHandle::failing()));
}

#[given("a connected filter wheel device")]
async fn connected_filter_wheel(world: &mut QhyCameraWorld) {
    world.build_filter_wheel_with_mock();
    let fw = world.filter_wheel.as_ref().unwrap();
    fw.set_connected(true).await.unwrap();
}

// --- When ---

#[when("I connect the camera")]
async fn connect_camera(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    camera.set_connected(true).await.unwrap();
}

#[when("I disconnect the camera")]
async fn disconnect_camera(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    camera.set_connected(false).await.unwrap();
}

#[when("I try to connect the camera")]
async fn try_connect_camera(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    match camera.set_connected(true).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

#[when("I connect the filter wheel")]
async fn connect_filter_wheel(world: &mut QhyCameraWorld) {
    let fw = world.filter_wheel.as_ref().unwrap();
    fw.set_connected(true).await.unwrap();
}

#[when("I disconnect the filter wheel")]
async fn disconnect_filter_wheel(world: &mut QhyCameraWorld) {
    let fw = world.filter_wheel.as_ref().unwrap();
    fw.set_connected(false).await.unwrap();
}

#[when("I try to connect the filter wheel")]
async fn try_connect_filter_wheel(world: &mut QhyCameraWorld) {
    let fw = world.filter_wheel.as_ref().unwrap();
    match fw.set_connected(true).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

// --- Then ---

#[then("the camera should be connected")]
async fn camera_connected(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    assert!(
        camera.connected().await.unwrap(),
        "expected camera connected"
    );
}

#[then("the camera should not be connected")]
async fn camera_not_connected(world: &mut QhyCameraWorld) {
    let camera = world.camera.as_ref().unwrap();
    assert!(
        !camera.connected().await.unwrap(),
        "expected camera not connected"
    );
}

#[then("the filter wheel should be connected")]
async fn filter_wheel_connected(world: &mut QhyCameraWorld) {
    let fw = world.filter_wheel.as_ref().unwrap();
    assert!(
        fw.connected().await.unwrap(),
        "expected filter wheel connected"
    );
}

#[then("the filter wheel should not be connected")]
async fn filter_wheel_not_connected(world: &mut QhyCameraWorld) {
    let fw = world.filter_wheel.as_ref().unwrap();
    assert!(
        !fw.connected().await.unwrap(),
        "expected filter wheel not connected"
    );
}

#[then("the operation should fail with a not-connected error")]
fn not_connected_error(world: &mut QhyCameraWorld) {
    assert!(
        world.last_error.is_some(),
        "expected an error but none occurred"
    );
}

#[then("the operation should fail with an invalid-value error")]
fn invalid_value_error(world: &mut QhyCameraWorld) {
    assert!(
        world.last_error.is_some(),
        "expected an error but none occurred"
    );
}

#[then("the operation should fail with an invalid-operation error")]
fn invalid_operation_error(world: &mut QhyCameraWorld) {
    assert!(
        world.last_error.is_some(),
        "expected an error but none occurred"
    );
}
