//! Steps for side_of_pier.feature.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::{given, then, when};

#[given(expr = "the RA-axis encoder reports mechanical hour angle {float} hours")]
async fn ra_encoder_reports_ha(world: &mut StarAdventurerWorld, ha_hours: f64) {
    todo!("Phase 3: convert ha_hours to encoder ticks given seeded CPR, set mock state.ra.position_ticks")
}

#[when("I try to set SideOfPier to East")]
async fn try_set_side_of_pier_east(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::api::telescope::PierSide;
    match world.mount().set_side_of_pier(PierSide::East).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(e.code.raw());
            world.last_error = Some(e.message.to_string());
        }
    }
}

#[when(expr = "I try to read DestinationSideOfPier for RA {float} hours and Dec {float} degrees")]
async fn try_read_destination_side(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    match world.mount().destination_side_of_pier(ra, dec).await {
        Ok(_) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(e.code.raw());
            world.last_error = Some(e.message.to_string());
        }
    }
}

#[then(expr = "SideOfPier should be {word}")]
async fn side_of_pier_should_be(world: &mut StarAdventurerWorld, expected: String) {
    use ascom_alpaca::api::telescope::PierSide;
    let actual = world.mount().side_of_pier().await.unwrap();
    let want = match expected.as_str() {
        "East" => PierSide::East,
        "West" => PierSide::West,
        "Unknown" => PierSide::Unknown,
        other => panic!("unknown PierSide name: {other}"),
    };
    assert_eq!(actual, want);
}
