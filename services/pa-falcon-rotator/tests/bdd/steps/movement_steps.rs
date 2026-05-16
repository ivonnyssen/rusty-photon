//! Step definitions for movement.feature

use crate::world::FalconRotatorWorld;
use cucumber::{then, when};

#[when(expr = "I call MoveAbsolute with {float}")]
async fn move_absolute(world: &mut FalconRotatorWorld, position: f64) {
    let _ = (world, position);
    todo!("movement_steps::move_absolute implemented in Phase 3d")
}

#[when(expr = "I call Move with {float}")]
async fn move_relative(world: &mut FalconRotatorWorld, delta: f64) {
    let _ = (world, delta);
    todo!("movement_steps::move_relative implemented in Phase 3d")
}

#[when(expr = "I call MoveMechanical with {float}")]
async fn move_mechanical(world: &mut FalconRotatorWorld, mech: f64) {
    let _ = (world, mech);
    todo!("movement_steps::move_mechanical implemented in Phase 3d")
}

#[when(expr = "I call MoveAbsolute with NaN")]
async fn move_absolute_nan(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("movement_steps::move_absolute_nan implemented in Phase 3d")
}

#[when("I read IsMoving")]
async fn read_is_moving(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("movement_steps::read_is_moving implemented in Phase 3d")
}

#[then(expr = "MD:{float} should have been sent")]
async fn md_command_sent(world: &mut FalconRotatorWorld, value: f64) {
    let _ = (world, value);
    todo!("movement_steps::md_command_sent implemented in Phase 3d")
}

#[then(expr = "IsMoving should be {word}")]
async fn is_moving_should_be(world: &mut FalconRotatorWorld, value: String) {
    let _ = (world, value);
    todo!("movement_steps::is_moving_should_be implemented in Phase 3d")
}

#[then(expr = "the move should fail with code {int}")]
async fn move_should_fail_with(world: &mut FalconRotatorWorld, code: u16) {
    let _ = (world, code);
    todo!("movement_steps::move_should_fail_with implemented in Phase 3d")
}
