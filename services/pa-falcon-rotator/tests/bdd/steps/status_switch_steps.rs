//! Step definitions for status_switch.feature

use crate::world::FalconRotatorWorld;
use cucumber::{given, then, when};

#[given(expr = "the device reports raw voltage {int}")]
async fn given_voltage(world: &mut FalconRotatorWorld, raw: u32) {
    let _ = (world, raw);
    todo!("status_switch_steps::given_voltage implemented in Phase 3e")
}

#[given(expr = "the device's limit_detect is {word}")]
async fn given_limit_detect(world: &mut FalconRotatorWorld, value: String) {
    let _ = (world, value);
    todo!("status_switch_steps::given_limit_detect implemented in Phase 3e")
}

#[when("I read MaxSwitch")]
async fn read_max_switch(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("status_switch_steps::read_max_switch implemented in Phase 3e")
}

// `I read GetSwitchValue for id N` is referenced by a Phase 3d scenario in
// `connection_lifecycle.feature` (NOT_CONNECTED for disconnected switch),
// so its body lands here ahead of the rest of 3e — the
// `ensure_connected!` guard on the switch getter handles that case
// without needing Phase 3e's actual value plumbing.
#[when(expr = "I read GetSwitchValue for id {int}")]
async fn read_switch_value(world: &mut FalconRotatorWorld, id: usize) {
    match world.status_switch().get_switch_value(id).await {
        Ok(v) => {
            world.switch_value_result = Some(v);
            world.last_error_code = None;
        }
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

#[when(expr = "I read GetSwitch for id {int}")]
async fn read_switch_bool(world: &mut FalconRotatorWorld, id: usize) {
    let _ = (world, id);
    todo!("status_switch_steps::read_switch_bool implemented in Phase 3e")
}

#[when(expr = "I read GetSwitchName for id {int}")]
async fn read_switch_name(world: &mut FalconRotatorWorld, id: usize) {
    let _ = (world, id);
    todo!("status_switch_steps::read_switch_name implemented in Phase 3e")
}

#[when(expr = "I read GetSwitchDescription for id {int}")]
async fn read_switch_description(world: &mut FalconRotatorWorld, id: usize) {
    let _ = (world, id);
    todo!("status_switch_steps::read_switch_description implemented in Phase 3e")
}

#[when(expr = "I read MinSwitchValue for id {int}")]
async fn read_min_switch_value(world: &mut FalconRotatorWorld, id: usize) {
    let _ = (world, id);
    todo!("status_switch_steps::read_min_switch_value implemented in Phase 3e")
}

#[when(expr = "I read MaxSwitchValue for id {int}")]
async fn read_max_switch_value(world: &mut FalconRotatorWorld, id: usize) {
    let _ = (world, id);
    todo!("status_switch_steps::read_max_switch_value implemented in Phase 3e")
}

#[when(expr = "I read SwitchStep for id {int}")]
async fn read_switch_step(world: &mut FalconRotatorWorld, id: usize) {
    let _ = (world, id);
    todo!("status_switch_steps::read_switch_step implemented in Phase 3e")
}

#[when(expr = "I read CanWrite for id {int}")]
async fn read_can_write(world: &mut FalconRotatorWorld, id: usize) {
    let _ = (world, id);
    todo!("status_switch_steps::read_can_write implemented in Phase 3e")
}

#[then(expr = "the switch name should be {string}")]
async fn switch_name_should_be(world: &mut FalconRotatorWorld, expected: String) {
    let _ = (world, expected);
    todo!("status_switch_steps::switch_name_should_be implemented in Phase 3e")
}

#[then(expr = "the switch description should mention {string}")]
async fn switch_description_should_mention(world: &mut FalconRotatorWorld, substring: String) {
    let _ = (world, substring);
    todo!("status_switch_steps::switch_description_should_mention implemented in Phase 3e")
}

#[when(expr = "I call SetSwitch on id {int} with {word}")]
async fn call_set_switch(world: &mut FalconRotatorWorld, id: usize, value: String) {
    let _ = (world, id, value);
    todo!("status_switch_steps::call_set_switch implemented in Phase 3e")
}

#[then(expr = "MaxSwitch should be {int}")]
async fn max_switch_should_be(world: &mut FalconRotatorWorld, expected: usize) {
    let _ = (world, expected);
    todo!("status_switch_steps::max_switch_should_be implemented in Phase 3e")
}

#[then(expr = "the switch value should be {float}")]
async fn switch_value_should_be(world: &mut FalconRotatorWorld, expected: f64) {
    let _ = (world, expected);
    todo!("status_switch_steps::switch_value_should_be implemented in Phase 3e")
}

#[then(expr = "the switch boolean should be {word}")]
async fn switch_boolean_should_be(world: &mut FalconRotatorWorld, expected: String) {
    let _ = (world, expected);
    todo!("status_switch_steps::switch_boolean_should_be implemented in Phase 3e")
}

#[then(expr = "the set should fail with code {int}")]
async fn set_should_fail_with(world: &mut FalconRotatorWorld, code: u16) {
    let _ = (world, code);
    todo!("status_switch_steps::set_should_fail_with implemented in Phase 3e")
}

#[then(expr = "CanWrite for id {int} should be {word}")]
async fn can_write_should_be(world: &mut FalconRotatorWorld, id: usize, value: String) {
    let _ = (world, id, value);
    todo!("status_switch_steps::can_write_should_be implemented in Phase 3e")
}
