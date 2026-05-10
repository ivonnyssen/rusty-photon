//! Steps for coordinate_reads.feature.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::{given, then, when};

#[given(expr = "a mount with CPR {int} on both axes")]
async fn mount_with_cpr(world: &mut StarAdventurerWorld, cpr: u32) {
    todo!("Phase 3: pre-seed mock state.cpr_ra/cpr_dec")
}

#[given(expr = "the RA-axis encoder reads {int} ticks")]
async fn ra_encoder_reads(world: &mut StarAdventurerWorld, ticks: i32) {
    todo!("Phase 3: pre-seed mock state.ra.position_ticks")
}

#[given(expr = "the Dec-axis encoder reads {int} ticks")]
async fn dec_encoder_reads(world: &mut StarAdventurerWorld, ticks: i32) {
    todo!("Phase 3: pre-seed mock state.dec.position_ticks")
}

#[given(expr = "site longitude is {float} degrees")]
async fn site_longitude_is(world: &mut StarAdventurerWorld, deg: f64) {
    todo!("Phase 3: set world.config.mount.site_longitude_deg")
}

#[given(expr = "UTC is {string}")]
async fn utc_is(world: &mut StarAdventurerWorld, ts: String) {
    todo!("Phase 3: pin world.fixed_utc to a parsed timestamp; coordinates module reads it via injection")
}

#[given("the mount reports both axes stopped")]
async fn mount_axes_stopped(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: mock state.ra.running = false, state.dec.running = false")
}

#[given("the mount reports the RA axis running in goto mode")]
async fn ra_axis_running_goto(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: mock state.ra.running = true, state.ra.goto = true")
}

#[when("I try to read RightAscension")]
async fn try_read_ra(world: &mut StarAdventurerWorld) {
    match world.mount().right_ascension().await {
        Ok(_) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when("I try to read Declination")]
async fn try_read_dec(world: &mut StarAdventurerWorld) {
    match world.mount().declination().await {
        Ok(_) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[then(expr = "RightAscension should equal SiderealTime within {float} hours")]
async fn ra_equals_sidereal_time(world: &mut StarAdventurerWorld, tolerance: f64) {
    let ra = world.mount().right_ascension().await.unwrap();
    let lst = world.mount().sidereal_time().await.unwrap();
    assert!((ra - lst).abs() < tolerance, "RA {ra} vs LST {lst}");
}

#[then(expr = "Declination should be {float} degrees within {float}")]
async fn declination_should_be(world: &mut StarAdventurerWorld, expected: f64, tolerance: f64) {
    let actual = world.mount().declination().await.unwrap();
    assert!(
        (actual - expected).abs() < tolerance,
        "{actual} vs {expected}"
    );
}

#[then(expr = "RightAscension should be {float} hours within {float}")]
async fn ra_should_be(world: &mut StarAdventurerWorld, expected: f64, tolerance: f64) {
    let actual = world.mount().right_ascension().await.unwrap();
    assert!(
        (actual - expected).abs() < tolerance,
        "{actual} vs {expected}"
    );
}

#[then(expr = "SiderealTime should be approximately {float} hours within {float}")]
async fn sidereal_time_should_be(world: &mut StarAdventurerWorld, expected: f64, tolerance: f64) {
    let actual = world.mount().sidereal_time().await.unwrap();
    assert!(
        (actual - expected).abs() < tolerance,
        "{actual} vs {expected}"
    );
}

#[then("Slewing should be false")]
async fn slewing_should_be_false(world: &mut StarAdventurerWorld) {
    assert!(!world.mount().slewing().await.unwrap());
}

#[then("Slewing should be true")]
async fn slewing_should_be_true(world: &mut StarAdventurerWorld) {
    assert!(world.mount().slewing().await.unwrap());
}

#[then(expr = "Slewing should eventually be false within {int} seconds")]
async fn slewing_eventually_false(world: &mut StarAdventurerWorld, secs: u64) {
    todo!("Phase 3: poll Slewing every 200ms, fail after secs elapse")
}
