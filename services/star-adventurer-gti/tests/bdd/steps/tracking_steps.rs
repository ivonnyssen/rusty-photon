//! Steps for tracking.feature.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::{then, when};

#[when("I enable tracking")]
async fn enable_tracking(world: &mut StarAdventurerWorld) {
    world.mount().set_tracking(true).await.unwrap();
}

#[when("I disable tracking")]
async fn disable_tracking(world: &mut StarAdventurerWorld) {
    world.mount().set_tracking(false).await.unwrap();
}

#[when("I try to enable tracking")]
async fn try_enable_tracking(world: &mut StarAdventurerWorld) {
    match world.mount().set_tracking(true).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(e.code.raw());
            world.last_error = Some(e.message.to_string());
        }
    }
}

#[when("I try to set TrackingRate to Lunar")]
async fn try_set_tracking_rate_lunar(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::api::telescope::DriveRate;
    match world.mount().set_tracking_rate(DriveRate::Lunar).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(e.code.raw());
            world.last_error = Some(e.message.to_string());
        }
    }
}

#[then("Tracking should be true")]
async fn tracking_should_be_true(world: &mut StarAdventurerWorld) {
    assert!(world.mount().tracking().await.unwrap());
}

#[then("Tracking should be false")]
async fn tracking_should_be_false(world: &mut StarAdventurerWorld) {
    assert!(!world.mount().tracking().await.unwrap());
}

#[then("TrackingRate should be Sidereal")]
async fn tracking_rate_should_be_sidereal(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::api::telescope::DriveRate;
    let rate = world.mount().tracking_rate().await.unwrap();
    assert_eq!(rate, DriveRate::Sidereal);
}

#[then("the Dec axis should have received no commands")]
async fn dec_axis_no_commands(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: assert mock command-history has no `:_2...` frames since the last reset")
}
