//! Step definitions for halt.feature

use crate::world::FalconRotatorWorld;
use cucumber::{then, when};

#[when("I call Halt")]
async fn call_halt(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("halt_steps::call_halt implemented in Phase 3d")
}

#[then("FH should have been sent")]
async fn fh_sent(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("halt_steps::fh_sent implemented in Phase 3d")
}

#[then("TargetPosition should track current Position after Halt")]
async fn target_tracks_position_after_halt(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("halt_steps::target_tracks_position_after_halt implemented in Phase 3d")
}
