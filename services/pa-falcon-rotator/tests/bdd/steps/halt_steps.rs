//! Step definitions for halt.feature

use crate::world::FalconRotatorWorld;
use cucumber::{then, when};

#[when("I call Halt")]
async fn call_halt(world: &mut FalconRotatorWorld) {
    match world.rotator().halt().await {
        Ok(()) => world.last_error_code = None,
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

#[then("FH should have been sent")]
async fn fh_sent(world: &mut FalconRotatorWorld) {
    let log = world.mock().command_log().await;
    assert!(log.iter().any(|c| c == "FH"), "no FH in wire log: {log:?}");
}

#[then("TargetPosition should track current Position after Halt")]
async fn target_tracks_position_after_halt(world: &mut FalconRotatorWorld) {
    // After Halt, the stored target is cleared and TargetPosition falls
    // back to the current Position. Read both and assert they match —
    // their absolute value doesn't matter, only that the driver no
    // longer reports a stale target.
    let position = world.rotator().position().await.unwrap();
    let target = world.rotator().target_position().await.unwrap();
    assert!(
        (position - target).abs() < 1e-6,
        "TargetPosition ({target}) did not track Position ({position}) after Halt"
    );
}
