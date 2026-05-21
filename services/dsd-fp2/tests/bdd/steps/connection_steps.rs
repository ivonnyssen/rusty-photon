//! Step definitions for connection_lifecycle.feature.

use cucumber::{given, then, when};

use crate::world::{calibrator_status_from_str, cover_status_from_str, Fp2World};

#[given("a running FP2 service")]
async fn running_fp2_service(world: &mut Fp2World) {
    world.start().await;
}

#[given("a connected FP2 device")]
async fn connected_fp2_device(world: &mut Fp2World) {
    world.start().await;
    world.device().set_connected(true).await.unwrap();
}

#[when("the device is connected")]
async fn connect(world: &mut Fp2World) {
    world.device().set_connected(true).await.unwrap();
}

#[when("the device is disconnected")]
async fn disconnect(world: &mut Fp2World) {
    world.device().set_connected(false).await.unwrap();
}

#[then("the device should report connected")]
async fn assert_connected(world: &mut Fp2World) {
    assert!(
        world.device().connected().await.unwrap(),
        "expected connected, got disconnected"
    );
}

#[then("the device should report disconnected")]
async fn assert_disconnected(world: &mut Fp2World) {
    assert!(
        !world.device().connected().await.unwrap(),
        "expected disconnected, got connected"
    );
}

#[then(regex = r"^cover_state should be (\w+)$")]
async fn assert_cover_state(world: &mut Fp2World, expected: String) {
    let actual = world.device().cover_state().await.unwrap();
    assert_eq!(actual, cover_status_from_str(&expected));
}

#[then(regex = r"^calibrator_state should be (\w+)$")]
async fn assert_calibrator_state(world: &mut Fp2World, expected: String) {
    let actual = world.device().calibrator_state().await.unwrap();
    assert_eq!(actual, calibrator_status_from_str(&expected));
}

#[then(regex = r"^cover_state should eventually be (\w+)$")]
async fn assert_eventually_cover_state(world: &mut Fp2World, expected: String) {
    world
        .wait_for_cover_state(cover_status_from_str(&expected))
        .await;
}

#[then(regex = r"^calibrator_state should eventually be (\w+)$")]
async fn assert_eventually_calibrator_state(world: &mut Fp2World, expected: String) {
    world
        .wait_for_calibrator_state(calibrator_status_from_str(&expected))
        .await;
}
