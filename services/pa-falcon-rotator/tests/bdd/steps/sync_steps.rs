//! Step definitions for sync_offset.feature

use crate::world::FalconRotatorWorld;
use cucumber::{then, when};

#[when(expr = "I call Sync with {float}")]
async fn call_sync(world: &mut FalconRotatorWorld, position: f64) {
    let _ = (world, position);
    todo!("sync_steps::call_sync implemented in Phase 3d")
}

#[when("I call Sync with NaN")]
async fn call_sync_nan(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("sync_steps::call_sync_nan implemented in Phase 3d")
}

#[then(expr = "Sync should fail with code {int}")]
async fn sync_should_fail_with(world: &mut FalconRotatorWorld, code: u16) {
    let _ = (world, code);
    todo!("sync_steps::sync_should_fail_with implemented in Phase 3d")
}

#[then("no SD command should have been sent")]
async fn no_sd_command(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("sync_steps::no_sd_command implemented in Phase 3d")
}

#[then("MechanicalPosition should be unchanged")]
async fn mechanical_position_unchanged(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("sync_steps::mechanical_position_unchanged implemented in Phase 3d")
}
