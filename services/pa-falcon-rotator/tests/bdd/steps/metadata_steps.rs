//! Step definitions for metadata.feature

use crate::world::FalconRotatorWorld;
use cucumber::then;

#[then("CanReverse should be true")]
async fn can_reverse_true(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("metadata_steps::can_reverse_true implemented in Phase 3d")
}

#[then("StepSize should be 0.01155")]
async fn step_size_is(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("metadata_steps::step_size_is implemented in Phase 3d")
}

#[then("Name should be \"Pegasus Falcon Rotator\"")]
async fn rotator_name_is(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("metadata_steps::rotator_name_is implemented in Phase 3d")
}

#[then("UniqueID should be \"pa-falcon-rotator-001\"")]
async fn rotator_unique_id_is(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("metadata_steps::rotator_unique_id_is implemented in Phase 3d")
}

#[then("InterfaceVersion should be 4")]
async fn rotator_interface_version_is(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("metadata_steps::rotator_interface_version_is implemented in Phase 3d")
}
