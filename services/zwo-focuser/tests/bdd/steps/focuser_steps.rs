//! Device-specific steps: move, halt, step-size/position queries, and
//! numeric property reports.

use cucumber::{then, when};

use crate::world::FocuserWorld;

// --- movement ----------------------------------------------------------------

#[when(regex = r"^I move focuser device (\d+) to position (-?\d+)$")]
async fn move_focuser(world: &mut FocuserWorld, _device: u32, position: i32) {
    world.focuser().move_(position).await.unwrap();
}

#[when(regex = r"^I try to move focuser device (\d+) to position (-?\d+)$")]
async fn try_move_focuser(world: &mut FocuserWorld, _device: u32, position: i32) {
    world.try_move(position).await;
}

#[when(regex = r"^I halt focuser device (\d+)$")]
async fn halt_focuser(world: &mut FocuserWorld, _device: u32) {
    world.focuser().halt().await.unwrap();
}

#[when(regex = r"^I try to halt focuser device (\d+)$")]
async fn try_halt_focuser(world: &mut FocuserWorld, _device: u32) {
    world.try_halt().await;
}

#[when(regex = r"^I query position on focuser device (\d+)$")]
async fn query_position(world: &mut FocuserWorld, _device: u32) {
    world.try_position().await;
}

#[when(regex = r"^I query step size on focuser device (\d+)$")]
async fn query_step_size(world: &mut FocuserWorld, _device: u32) {
    world.try_step_size().await;
}

#[when(regex = r"^I try to set temp comp to (true|false) on focuser device (\d+)$")]
async fn try_set_temp_comp(world: &mut FocuserWorld, value: bool, _device: u32) {
    world.try_set_temp_comp(value).await;
}

// --- numeric property reports ------------------------------------------------

#[then(regex = r"^focuser device (\d+) reports (MaxStep|MaxIncrement) as (\d+)$")]
async fn focuser_reports_u32(
    world: &mut FocuserWorld,
    _device: u32,
    property: String,
    expected: u32,
) {
    let focuser = world.focuser();
    let actual = match property.as_str() {
        "MaxStep" => focuser.max_step().await.unwrap(),
        "MaxIncrement" => focuser.max_increment().await.unwrap(),
        other => panic!("unknown u32 property: {other}"),
    };
    assert_eq!(
        actual, expected,
        "{property} expected {expected}, got {actual}"
    );
}

#[then(regex = r"^focuser device (\d+) reports Position as (-?\d+)$")]
async fn focuser_reports_position(world: &mut FocuserWorld, _device: u32, expected: i32) {
    let actual = world.focuser().position().await.unwrap();
    assert_eq!(
        actual, expected,
        "Position expected {expected}, got {actual}"
    );
}

#[then(regex = r"^focuser device (\d+) reports Temperature as ([\-0-9.]+)$")]
async fn focuser_reports_temperature(world: &mut FocuserWorld, _device: u32, expected: f64) {
    let actual = world.focuser().temperature().await.unwrap();
    assert_eq!(
        actual, expected,
        "Temperature expected {expected}, got {actual}"
    );
}
