//! Steps for side_of_pier.feature.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::{given, then, when};

#[given(expr = "the Dec-axis encoder reports angle {float} degrees")]
async fn dec_encoder_reports_angle(world: &mut StarAdventurerWorld, deg: f64) {
    // Convert Dec angle (degrees, unfolded — values outside ±90° are
    // allowed so scenarios can place the encoder "past the pole" to
    // exercise the post-flip branch) → encoder ticks against the
    // GTi-default CPR (`0x375F00 = 3,628,800`). Each scenario in this
    // feature pins CPR to that default.
    const GTI_CPR: u32 = 0x0037_5F00;
    let ticks = (deg * (GTI_CPR as f64) / 360.0).round() as i32;
    world.queue_seed("dec_ticks", ticks.into()).await;
}

// The parametric `I try to set SideOfPier to {word}` step is
// registered in `meridian_flip_steps.rs`; existing scenarios that
// match this phrase exactly resolve there instead.

#[when(expr = "I read DestinationSideOfPier for RA {float} hours and Dec {float} degrees")]
async fn read_destination_side(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    let side = world
        .mount()
        .destination_side_of_pier(ra, dec)
        .await
        .expect("DestinationSideOfPier should succeed for this scenario");
    world.record_destination_pier_side(side);
}

#[when(expr = "I try to read DestinationSideOfPier for RA {float} hours and Dec {float} degrees")]
async fn try_read_destination_side(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    match world.mount().destination_side_of_pier(ra, dec).await {
        Ok(side) => world.record_destination_pier_side(side),
        Err(e) => world.record_error(e),
    }
}

#[then(expr = "SideOfPier should be {word}")]
async fn side_of_pier_should_be(world: &mut StarAdventurerWorld, expected: String) {
    use ascom_alpaca::api::telescope::PierSide;
    use std::time::Duration;
    let want = pier_side_from_label(&expected);
    // The snapshot lags the seeded Dec encoder position by up to one
    // polling cycle, so retry briefly before failing.
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    let mut last = PierSide::Unknown;
    while std::time::Instant::now() < deadline {
        last = world.mount().side_of_pier().await.unwrap();
        if last == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(last, want);
}

#[then(expr = "DestinationSideOfPier should be {word}")]
async fn destination_side_should_be(world: &mut StarAdventurerWorld, expected: String) {
    let want = pier_side_from_label(&expected);
    let got = world
        .last_destination_pier_side
        .expect("no DestinationSideOfPier captured — did the When step run?");
    assert_eq!(got, want);
}

fn pier_side_from_label(label: &str) -> ascom_alpaca::api::telescope::PierSide {
    use ascom_alpaca::api::telescope::PierSide;
    match label {
        "East" => PierSide::East,
        "West" => PierSide::West,
        "Unknown" => PierSide::Unknown,
        other => panic!("unknown PierSide name: {other}"),
    }
}
