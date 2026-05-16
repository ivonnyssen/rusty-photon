//! Step definitions for reverse.feature

use crate::world::FalconRotatorWorld;
use cucumber::{given, then, when};

#[given(expr = "the device's motor_reverse is currently {word}")]
async fn given_motor_reverse(world: &mut FalconRotatorWorld, value: String) {
    let _ = (world, value);
    todo!("reverse_steps::given_motor_reverse implemented in Phase 3d")
}

#[when(expr = "I set Reverse to {word}")]
async fn set_reverse(world: &mut FalconRotatorWorld, value: String) {
    let _ = (world, value);
    todo!("reverse_steps::set_reverse implemented in Phase 3d")
}

#[when("I read Reverse")]
async fn read_reverse(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("reverse_steps::read_reverse implemented in Phase 3d")
}

#[then(expr = "Reverse should be {word}")]
async fn reverse_should_be(world: &mut FalconRotatorWorld, value: String) {
    let _ = (world, value);
    todo!("reverse_steps::reverse_should_be implemented in Phase 3d")
}

#[then("no FN command should have been sent")]
async fn no_fn_command(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("reverse_steps::no_fn_command implemented in Phase 3d")
}

#[then(expr = "FN:{word} should have been sent")]
async fn fn_command_sent(world: &mut FalconRotatorWorld, value: String) {
    let _ = (world, value);
    todo!("reverse_steps::fn_command_sent implemented in Phase 3d")
}
