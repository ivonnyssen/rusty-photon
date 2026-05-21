use ascom_alpaca::api::CoverCalibrator;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};

use crate::world::Fp2World;

#[when("open_cover is called")]
async fn open_cover(world: &mut Fp2World) {
    world
        .device()
        .open_cover()
        .await
        .expect("open_cover should succeed");
}

#[when("close_cover is called")]
async fn close_cover(world: &mut Fp2World) {
    world
        .device()
        .close_cover()
        .await
        .expect("close_cover should succeed");
}

#[when("open_cover is called and the call is captured")]
async fn open_cover_capture(world: &mut Fp2World) {
    world.last_error = world.device().open_cover().await.err();
}

#[when("close_cover is called and the call is captured")]
async fn close_cover_capture(world: &mut Fp2World) {
    world.last_error = world.device().close_cover().await.err();
}

#[when("the cache is refreshed by a single poll")]
async fn refresh_cache(world: &mut Fp2World) {
    world.refresh_cache().await;
}

#[then("the call should fail with a not-connected error")]
async fn assert_call_failed_not_connected(world: &mut Fp2World) {
    let err = world
        .last_error
        .take()
        .expect("expected an error from the call");
    assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
}
