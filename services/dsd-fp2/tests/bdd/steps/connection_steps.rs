use ascom_alpaca::api::Device;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{given, then, when};

use crate::world::{calibrator_status_from_str, cover_status_from_str, Fp2World};

#[given("a freshly constructed FP2 device")]
fn fresh_device(world: &mut Fp2World) {
    world.build();
}

#[given("a freshly constructed FP2 device whose simulator pretends to be DeepSkyDad.FP1")]
async fn fresh_device_fp1_firmware(world: &mut Fp2World) {
    let state = dsd_fp2::MockState::default();
    state.set_firmware("DeepSkyDad.FP1", "1.0.0").await;
    world.build_with(state);
}

#[given("a connected FP2 device")]
async fn connected_device(world: &mut Fp2World) {
    world.build();
    world
        .device()
        .set_connected(true)
        .await
        .expect("connect should succeed");
}

#[given("a connected FP2 device whose simulator starts open")]
async fn connected_device_starting_open(world: &mut Fp2World) {
    let state = dsd_fp2::MockState::default();
    state.set_cover_angle(0).await;
    world.build_with(state);
    world
        .device()
        .set_connected(true)
        .await
        .expect("connect should succeed");
}

#[when("the device is connected")]
async fn connect(world: &mut Fp2World) {
    world
        .device()
        .set_connected(true)
        .await
        .expect("connect should succeed");
}

#[when("the device is connected and the connect attempt is captured")]
async fn connect_capture(world: &mut Fp2World) {
    world.last_error = world.device().set_connected(true).await.err();
}

#[when("the device is disconnected")]
async fn disconnect(world: &mut Fp2World) {
    world
        .device()
        .set_connected(false)
        .await
        .expect("disconnect should succeed");
}

#[then("the device should report connected")]
async fn assert_connected(world: &mut Fp2World) {
    let connected = world.device().connected().await.unwrap();
    assert!(connected, "expected connected, got disconnected");
}

#[then("the device should report disconnected")]
async fn assert_disconnected(world: &mut Fp2World) {
    let connected = world.device().connected().await.unwrap();
    assert!(!connected, "expected disconnected, got connected");
}

#[then(regex = r#"^the cached firmware board should be "(.+)"$"#)]
async fn assert_firmware_board(world: &mut Fp2World, expected: String) {
    let snap = world.manager().snapshot();
    let s = snap.read().await.clone();
    assert_eq!(s.firmware_board.as_deref(), Some(expected.as_str()));
}

#[then(regex = r"^cover_state should be (\w+)$")]
async fn assert_cover_state(world: &mut Fp2World, expected: String) {
    use ascom_alpaca::api::CoverCalibrator;
    let actual = world.device().cover_state().await.unwrap();
    assert_eq!(actual, cover_status_from_str(&expected));
}

#[then(regex = r"^calibrator_state should be (\w+)$")]
async fn assert_calibrator_state(world: &mut Fp2World, expected: String) {
    use ascom_alpaca::api::CoverCalibrator;
    let actual = world.device().calibrator_state().await.unwrap();
    assert_eq!(actual, calibrator_status_from_str(&expected));
}

#[then("the connect attempt should fail with a not-connected error")]
async fn assert_connect_failed_not_connected(world: &mut Fp2World) {
    let err = world
        .last_error
        .as_ref()
        .expect("expected an error from set_connected");
    assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
}
