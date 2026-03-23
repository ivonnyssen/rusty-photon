//! Step definitions for switch_control.feature
//!
//! Also defines the shared "Given a running PPBA server with the switch connected"
//! and "Given a running PPBA server with auto-dew enabled" steps used by other features.

use crate::steps::infrastructure::default_test_config;
use crate::world::PpbaWorld;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a running PPBA server with the switch connected")]
async fn running_server_with_switch_connected(world: &mut PpbaWorld) {
    world.config = default_test_config();
    world.start_ppba().await;
    world.switch_ref().set_connected(true).await.unwrap();
}

#[given("a running PPBA server with auto-dew enabled")]
async fn running_server_with_autodew_enabled(world: &mut PpbaWorld) {
    world.config = default_test_config();
    world.start_ppba().await;
    world.switch_ref().set_connected(true).await.unwrap();

    world.wait_for_switch_data().await;

    // Enable auto-dew (switch 5 = 1.0)
    world.switch_ref().set_switch_value(5, 1.0).await.unwrap();

    // Wait for the data to refresh after the set
    world.wait_for_switch_data().await;
}

// ============================================================================
// When steps
// ============================================================================

#[when("I wait for the switch data to be available")]
async fn wait_for_switch_data(world: &mut PpbaWorld) {
    world.wait_for_switch_data().await;
}

#[when(expr = "I set switch {int} value to {float}")]
async fn set_switch_value(world: &mut PpbaWorld, id: i32, value: f64) {
    world
        .switch_ref()
        .set_switch_value(id as usize, value)
        .await
        .unwrap();
}

#[when(expr = "I try to set switch {int} value to {float}")]
async fn try_set_switch_value(world: &mut PpbaWorld, id: i32, value: f64) {
    let result = world
        .switch_ref()
        .set_switch_value(id as usize, value)
        .await;
    world.capture_result(result);
}

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "switch {int} value should be {float}")]
async fn switch_value_should_be(world: &mut PpbaWorld, id: i32, expected: f64) {
    let value = world
        .switch_ref()
        .get_switch_value(id as usize)
        .await
        .unwrap();
    assert!(
        (value - expected).abs() < f64::EPSILON,
        "switch {} value: expected {}, got {}",
        id,
        expected,
        value
    );
}

#[then(expr = "switch {int} boolean should be true")]
async fn switch_boolean_should_be_true(world: &mut PpbaWorld, id: i32) {
    assert!(
        world.switch_ref().get_switch(id as usize).await.unwrap(),
        "switch {} should be true",
        id
    );
}

#[then(expr = "setting switch {int} boolean to true should succeed")]
async fn setting_switch_boolean_should_succeed(world: &mut PpbaWorld, id: i32) {
    world
        .switch_ref()
        .set_switch(id as usize, true)
        .await
        .unwrap();
}

#[then(expr = "switches {int} through {int} should be writable")]
async fn switches_should_be_writable(world: &mut PpbaWorld, from: i32, to: i32) {
    let switch = world.switch_ref();
    for id in from..=to {
        assert!(
            switch.can_write(id as usize).await.unwrap(),
            "switch {} should be writable",
            id
        );
    }
}

#[then(expr = "switches {int} through {int} should not be writable")]
async fn switches_should_not_be_writable(world: &mut PpbaWorld, from: i32, to: i32) {
    let switch = world.switch_ref();
    for id in from..=to {
        assert!(
            !switch.can_write(id as usize).await.unwrap(),
            "switch {} should not be writable",
            id
        );
    }
}

#[then(expr = "switch {int} should not be writable")]
async fn switch_should_not_be_writable(world: &mut PpbaWorld, id: i32) {
    assert!(
        !world.switch_ref().can_write(id as usize).await.unwrap(),
        "switch {} should not be writable",
        id
    );
}

#[then(expr = "switch {int} should be writable")]
async fn switch_should_be_writable(world: &mut PpbaWorld, id: i32) {
    assert!(
        world.switch_ref().can_write(id as usize).await.unwrap(),
        "switch {} should be writable",
        id
    );
}

#[then(expr = "setting switch {int} value to {float} should succeed")]
async fn setting_switch_value_should_succeed(world: &mut PpbaWorld, id: i32, value: f64) {
    world
        .switch_ref()
        .set_switch_value(id as usize, value)
        .await
        .unwrap();
}

#[then(
    expr = "all {int} switches should be queryable for name, description, min, max, step, value, and can_write"
)]
async fn all_switches_queryable(world: &mut PpbaWorld, count: i32) {
    let switch = world.switch_ref();
    for id in 0..count {
        let id = id as usize;
        switch.get_switch_name(id).await.unwrap();
        switch.get_switch_description(id).await.unwrap();
        switch.min_switch_value(id).await.unwrap();
        switch.max_switch_value(id).await.unwrap();
        switch.switch_step(id).await.unwrap();
        switch.get_switch_value(id).await.unwrap();
        switch.can_write(id).await.unwrap();
    }
}

#[then(expr = "get_switch should work for switches {int} through {int}")]
async fn get_switch_should_work(world: &mut PpbaWorld, from: i32, to: i32) {
    let switch = world.switch_ref();
    for id in from..=to {
        switch.get_switch(id as usize).await.unwrap();
    }
}
