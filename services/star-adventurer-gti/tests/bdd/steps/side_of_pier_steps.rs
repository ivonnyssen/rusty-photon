//! Steps for side_of_pier.feature.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::{given, then, when};

#[given(expr = "the RA-axis encoder reports mechanical hour angle {float} hours")]
async fn ra_encoder_reports_ha(world: &mut StarAdventurerWorld, ha_hours: f64) {
    // Convert HA → encoder ticks against the GTi-default CPR
    // (`0x375F00 = 3,628,800`). Each scenario in this feature pins
    // CPR to that default.
    const GTI_CPR: u32 = 0x0037_5F00;
    let ticks = (ha_hours * (GTI_CPR as f64) / 24.0).round() as i32;
    world.queue_seed("ra_ticks", ticks.into()).await;
}

#[when("I try to set SideOfPier to East")]
async fn try_set_side_of_pier_east(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::api::telescope::PierSide;
    match world.mount().set_side_of_pier(PierSide::East).await {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when(expr = "I try to read DestinationSideOfPier for RA {float} hours and Dec {float} degrees")]
async fn try_read_destination_side(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    match world.mount().destination_side_of_pier(ra, dec).await {
        Ok(_) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[then(expr = "SideOfPier should be {word}")]
async fn side_of_pier_should_be(world: &mut StarAdventurerWorld, expected: String) {
    use ascom_alpaca::api::telescope::PierSide;
    use std::time::Duration;
    let want = match expected.as_str() {
        "East" => PierSide::East,
        "West" => PierSide::West,
        "Unknown" => PierSide::Unknown,
        other => panic!("unknown PierSide name: {other}"),
    };
    // The snapshot lags the seeded RA encoder position by up to one
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
