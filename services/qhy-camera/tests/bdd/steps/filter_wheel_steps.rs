use ascom_alpaca::api::{Device, FilterWheel};
use cucumber::{given, then, when};

use crate::world::{default_filter_wheel_config, QhyCameraWorld};
use qhy_camera::FilterWheelConfig;

// --- Given ---

#[given(expr = "a filter wheel config with names {string}")]
fn filter_wheel_config_with_names(world: &mut QhyCameraWorld, names: String) {
    let filter_names: Vec<String> = names.split(',').map(|s| s.trim().to_string()).collect();
    world.filter_wheel_config = Some(FilterWheelConfig {
        filter_names,
        ..default_filter_wheel_config()
    });
}

#[given("a connected filter wheel device with config")]
async fn connected_filter_wheel_with_config(world: &mut QhyCameraWorld) {
    world.build_filter_wheel_with_mock();
    let fw = world.filter_wheel.as_ref().unwrap();
    fw.set_connected(true).await.unwrap();
}

// --- When ---

#[when(expr = "I set filter wheel position to {int}")]
async fn set_fw_position(world: &mut QhyCameraWorld, position: usize) {
    let fw = world.filter_wheel.as_ref().unwrap();
    fw.set_position(position).await.unwrap();
}

#[when(expr = "I try to set filter wheel position to {int}")]
async fn try_set_fw_position(world: &mut QhyCameraWorld, position: usize) {
    let fw = world.filter_wheel.as_ref().unwrap();
    match fw.set_position(position).await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

#[when("I try to read filter wheel position")]
async fn try_read_fw_position(world: &mut QhyCameraWorld) {
    let fw = world.filter_wheel.as_ref().unwrap();
    match fw.position().await {
        Ok(_) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(format!("{:?}", e.code));
            world.last_error = Some(e.to_string());
        }
    }
}

// --- Then ---

#[then(expr = "filter wheel position should be {int}")]
async fn check_fw_position(world: &mut QhyCameraWorld, expected: usize) {
    let fw = world.filter_wheel.as_ref().unwrap();
    let pos = fw.position().await.unwrap();
    assert_eq!(
        pos,
        Some(expected),
        "expected position Some({}), got {:?}",
        expected,
        pos
    );
}

#[then(expr = "filter names should have {int} entries")]
async fn check_filter_names_count(world: &mut QhyCameraWorld, expected: usize) {
    let fw = world.filter_wheel.as_ref().unwrap();
    assert_eq!(fw.names().await.unwrap().len(), expected);
}

#[then(expr = "first filter name should be {string}")]
async fn check_first_filter_name(world: &mut QhyCameraWorld, expected: String) {
    let fw = world.filter_wheel.as_ref().unwrap();
    let names = fw.names().await.unwrap();
    assert_eq!(
        names[0], expected,
        "first filter name should be '{}', got '{}'",
        expected, names[0]
    );
}

#[then(expr = "focus_offsets should have {int} entries")]
async fn check_focus_offsets_count(world: &mut QhyCameraWorld, expected: usize) {
    let fw = world.filter_wheel.as_ref().unwrap();
    assert_eq!(fw.focus_offsets().await.unwrap().len(), expected);
}

#[then("all focus offsets should be 0")]
async fn check_all_focus_offsets_zero(world: &mut QhyCameraWorld) {
    let fw = world.filter_wheel.as_ref().unwrap();
    let offsets = fw.focus_offsets().await.unwrap();
    assert!(
        offsets.iter().all(|&o| o == 0),
        "all focus offsets should be 0, got {:?}",
        offsets
    );
}
