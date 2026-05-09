//! Steps for park.feature.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::{then, when};

#[when("I park the mount")]
async fn park_mount(world: &mut StarAdventurerWorld) {
    world.mount().park().await.unwrap();
}

#[when("I try to park the mount")]
async fn try_park_mount(world: &mut StarAdventurerWorld) {
    match world.mount().park().await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(e.code.raw());
            world.last_error = Some(e.message.to_string());
        }
    }
}

#[when("I unpark the mount")]
async fn unpark_mount(world: &mut StarAdventurerWorld) {
    world.mount().unpark().await.unwrap();
}

#[when("I try to set the park position")]
async fn try_set_park_position(world: &mut StarAdventurerWorld) {
    match world.mount().set_park().await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(e.code.raw());
            world.last_error = Some(e.message.to_string());
        }
    }
}

#[when("the mount reports both axes stopped at encoder 0")]
async fn mount_reports_axes_stopped_at_zero(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: mock state.ra.position_ticks = 0, running = false; same for dec")
}

#[then("AtPark should be false")]
async fn at_park_false(world: &mut StarAdventurerWorld) {
    assert!(!world.mount().at_park().await.unwrap());
}

#[then(expr = "AtPark should eventually be true within {int} seconds")]
async fn at_park_eventually_true(world: &mut StarAdventurerWorld, secs: u64) {
    todo!("Phase 3: poll AtPark every 200ms, fail after secs elapse")
}

#[then("the mount should have received command :K1 before any :S1")]
async fn k1_before_s1(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: assert mock command-history has :K1 with index < first :S1 index")
}

#[then("the mount should have received a :S1 command targeting encoder 0")]
async fn s1_targeting_zero(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: assert any :S1 frame's bias-decoded payload is 0")
}

#[then("the mount should have received a :S2 command targeting encoder 0")]
async fn s2_targeting_zero(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: assert any :S2 frame's bias-decoded payload is 0")
}

#[then("the mount should not have received a second :S1 command")]
async fn no_second_s1(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: assert at most one :S1 in mock command-history")
}
