//! Steps for slew.feature.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then, when};

#[given("the device is parked")]
async fn device_is_parked(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: connect, then call park() and wait for AtPark = true")
}

#[when(expr = "I slew asynchronously to RA {float} hours and Dec {float} degrees")]
async fn slew_async_to(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    world
        .mount()
        .slew_to_coordinates_async(ra, dec)
        .await
        .unwrap();
}

#[when(expr = "I try to slew asynchronously to RA {float} hours and Dec {float} degrees")]
async fn try_slew_async_to(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    match world.mount().slew_to_coordinates_async(ra, dec).await {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when("I slew to the stored target")]
async fn slew_to_target(world: &mut StarAdventurerWorld) {
    world.mount().slew_to_target_async().await.unwrap();
}

#[when("I try to slew to the stored target")]
async fn try_slew_to_target(world: &mut StarAdventurerWorld) {
    match world.mount().slew_to_target_async().await {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when(expr = "I set TargetRightAscension to {float} hours")]
async fn set_target_ra(world: &mut StarAdventurerWorld, hours: f64) {
    world
        .mount()
        .set_target_right_ascension(hours)
        .await
        .unwrap();
}

#[when(expr = "I set TargetDeclination to {float} degrees")]
async fn set_target_dec(world: &mut StarAdventurerWorld, deg: f64) {
    world.mount().set_target_declination(deg).await.unwrap();
}

#[given("the mount reports both axes stopped in goto mode")]
async fn axes_stopped_in_goto(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: mock state.ra.running = false, goto = true; same for dec")
}

#[when("the mount reports both axes stopped in goto mode")]
async fn when_axes_stopped_in_goto(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: mock state.ra.running = false, goto = true; same for dec")
}

#[then("the mount should have received commands matching:")]
async fn commands_matching(world: &mut StarAdventurerWorld, step: &Step) {
    let _rows = step.table.as_ref().expect("expected a data table");
    todo!("Phase 3: read mock command-history, regex-match each row's pattern in order")
}

#[then(expr = "TargetRightAscension should be {float} hours within {float}")]
async fn target_ra_should_be(world: &mut StarAdventurerWorld, expected: f64, tolerance: f64) {
    let actual = world.mount().target_right_ascension().await.unwrap();
    assert!((actual - expected).abs() < tolerance);
}

#[then(expr = "TargetDeclination should be {float} degrees within {float}")]
async fn target_dec_should_be(world: &mut StarAdventurerWorld, expected: f64, tolerance: f64) {
    let actual = world.mount().target_declination().await.unwrap();
    assert!((actual - expected).abs() < tolerance);
}

#[then(
    expr = "the slew target on the wire should correspond to RA {float} hours and Dec {float} degrees"
)]
async fn wire_slew_target(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    todo!("Phase 3: read mock command-history for last :S1/:S2, decode the bias-encoded ticks, convert back to RA/Dec, assert within tolerance")
}

#[then(expr = "the mount should eventually receive a tracking-mode :G1 within {int} seconds")]
async fn mount_eventually_tracking_g1(world: &mut StarAdventurerWorld, secs: u64) {
    todo!("Phase 3: poll mock command-history; tracking-mode :G1 has goto bit clear")
}

#[then("the mount should not receive a tracking-mode :G1")]
async fn mount_should_not_tracking_g1(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: assert no tracking-mode :G1 ever appeared")
}
