//! Step definitions for cover_control.feature.

use ascom_alpaca::api::cover_calibrator::CoverStatus;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

use crate::world::Fp2World;

#[when("open_cover is called")]
async fn open_cover(world: &mut Fp2World) {
    world.device().open_cover().await.unwrap();
}

#[when("close_cover is called")]
async fn close_cover(world: &mut Fp2World) {
    world.device().close_cover().await.unwrap();
}

#[when("the cover has been opened")]
async fn cover_has_been_opened(world: &mut Fp2World) {
    world.device().open_cover().await.unwrap();
    world.wait_for_cover_state(CoverStatus::Open).await;
}

#[when("open_cover is called and the call is captured")]
async fn open_cover_capture(world: &mut Fp2World) {
    world.last_error = world.device().open_cover().await.err();
}

#[when("close_cover is called and the call is captured")]
async fn close_cover_capture(world: &mut Fp2World) {
    world.last_error = world.device().close_cover().await.err();
}

#[then("the call should fail with a not-connected error")]
async fn assert_call_failed_not_connected(world: &mut Fp2World) {
    let err = world
        .last_error
        .take()
        .expect("expected an error from the call");
    assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
}
