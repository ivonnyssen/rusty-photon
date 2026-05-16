//! Step definitions for position_reads.feature

use crate::world::FalconRotatorWorld;
use cucumber::{given, then, when};

#[given(expr = "the rotator reports mechanical position {float} degrees")]
async fn given_mechanical(world: &mut FalconRotatorWorld, degrees: f64) {
    let _ = (world, degrees);
    todo!("position_steps::given_mechanical implemented in Phase 3d")
}

#[given(expr = "the driver-side sync offset is {float} degrees")]
async fn given_sync_offset(world: &mut FalconRotatorWorld, degrees: f64) {
    let _ = (world, degrees);
    todo!("position_steps::given_sync_offset implemented in Phase 3d")
}

#[when("I read MechanicalPosition")]
async fn read_mechanical_position(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("position_steps::read_mechanical_position implemented in Phase 3d")
}

#[when("I read Position")]
async fn read_position(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("position_steps::read_position implemented in Phase 3d")
}

#[when("I read TargetPosition")]
async fn read_target_position(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("position_steps::read_target_position implemented in Phase 3d")
}

#[then(expr = "MechanicalPosition should be {float} degrees")]
async fn mechanical_position_should_be(world: &mut FalconRotatorWorld, expected: f64) {
    let _ = (world, expected);
    todo!("position_steps::mechanical_position_should_be implemented in Phase 3d")
}

#[then(expr = "Position should be {float} degrees")]
async fn position_should_be(world: &mut FalconRotatorWorld, expected: f64) {
    let _ = (world, expected);
    todo!("position_steps::position_should_be implemented in Phase 3d")
}

#[then(expr = "TargetPosition should be {float} degrees")]
async fn target_position_should_be(world: &mut FalconRotatorWorld, expected: f64) {
    let _ = (world, expected);
    todo!("position_steps::target_position_should_be implemented in Phase 3d")
}

#[then("an FA command should have been issued")]
async fn fa_command_issued(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("position_steps::fa_command_issued implemented in Phase 3d")
}
