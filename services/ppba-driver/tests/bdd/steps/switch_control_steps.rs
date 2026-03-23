//! Step definitions for switch_control.feature
//!
//! Also defines the shared "Given a running PPBA server with the switch connected"
//! and "Given a running PPBA server with auto-dew enabled" steps used by other features.

use crate::steps::infrastructure::*;
use crate::world::PpbaWorld;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a running PPBA server with the switch connected")]
async fn running_server_with_switch_connected(world: &mut PpbaWorld) {
    world.config = default_test_config();
    world.start_ppba().await;

    let url = world.switch_url();
    let resp = alpaca_put(&url, "connected", &[("Connected", "true")]).await;
    assert!(
        !is_alpaca_error(&resp),
        "connecting switch failed: {}",
        alpaca_error_message(&resp)
    );
}

#[given("a running PPBA server with auto-dew enabled")]
async fn running_server_with_autodew_enabled(world: &mut PpbaWorld) {
    world.config = default_test_config();
    world.start_ppba().await;

    let url = world.switch_url();
    let resp = alpaca_put(&url, "connected", &[("Connected", "true")]).await;
    assert!(
        !is_alpaca_error(&resp),
        "connecting switch failed: {}",
        alpaca_error_message(&resp)
    );

    world.wait_for_switch_data().await;

    // Enable auto-dew (switch 5 = 1.0)
    let resp = alpaca_put(&url, "setswitchvalue", &[("Id", "5"), ("Value", "1.0")]).await;
    assert!(
        !is_alpaca_error(&resp),
        "enabling auto-dew failed: {}",
        alpaca_error_message(&resp)
    );

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
    let url = world.switch_url();
    let id_str = id.to_string();
    let val_str = value.to_string();
    let resp = alpaca_put(
        &url,
        "setswitchvalue",
        &[("Id", &id_str), ("Value", &val_str)],
    )
    .await;
    assert!(
        !is_alpaca_error(&resp),
        "set switch {} value to {} failed: {}",
        id,
        value,
        alpaca_error_message(&resp)
    );
}

#[when(expr = "I try to set switch {int} value to {float}")]
async fn try_set_switch_value(world: &mut PpbaWorld, id: i32, value: f64) {
    let url = world.switch_url();
    let id_str = id.to_string();
    let val_str = value.to_string();
    let resp = alpaca_put(
        &url,
        "setswitchvalue",
        &[("Id", &id_str), ("Value", &val_str)],
    )
    .await;
    world.capture_response(&resp);
}

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "switch {int} value should be {float}")]
async fn switch_value_should_be(world: &mut PpbaWorld, id: i32, expected: f64) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("getswitchvalue?Id={}", id)).await;
    assert!(
        !is_alpaca_error(&resp),
        "getswitchvalue failed for switch {}",
        id
    );
    let value = alpaca_value(&resp)
        .as_f64()
        .expect("switch value should be a number");
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
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("getswitch?Id={}", id)).await;
    assert!(
        !is_alpaca_error(&resp),
        "getswitch failed for switch {}",
        id
    );
    assert_eq!(alpaca_value(&resp), true, "switch {} should be true", id);
}

#[then(expr = "setting switch {int} boolean to true should succeed")]
async fn setting_switch_boolean_should_succeed(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let id_str = id.to_string();
    let resp = alpaca_put(&url, "setswitch", &[("Id", &id_str), ("State", "true")]).await;
    assert!(
        !is_alpaca_error(&resp),
        "setswitch({}, true) failed: {}",
        id,
        alpaca_error_message(&resp)
    );
}

#[then(expr = "switches {int} through {int} should be writable")]
async fn switches_should_be_writable(world: &mut PpbaWorld, from: i32, to: i32) {
    let url = world.switch_url();
    for id in from..=to {
        let resp = alpaca_get(&url, &format!("canwrite?Id={}", id)).await;
        assert!(!is_alpaca_error(&resp), "canwrite failed for switch {}", id);
        assert_eq!(
            alpaca_value(&resp),
            true,
            "switch {} should be writable",
            id
        );
    }
}

#[then(expr = "switches {int} through {int} should not be writable")]
async fn switches_should_not_be_writable(world: &mut PpbaWorld, from: i32, to: i32) {
    let url = world.switch_url();
    for id in from..=to {
        let resp = alpaca_get(&url, &format!("canwrite?Id={}", id)).await;
        assert!(!is_alpaca_error(&resp), "canwrite failed for switch {}", id);
        assert_eq!(
            alpaca_value(&resp),
            false,
            "switch {} should not be writable",
            id
        );
    }
}

#[then(expr = "switch {int} should not be writable")]
async fn switch_should_not_be_writable(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("canwrite?Id={}", id)).await;
    assert!(!is_alpaca_error(&resp), "canwrite failed for switch {}", id);
    assert_eq!(
        alpaca_value(&resp),
        false,
        "switch {} should not be writable",
        id
    );
}

#[then(expr = "switch {int} should be writable")]
async fn switch_should_be_writable(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("canwrite?Id={}", id)).await;
    assert!(!is_alpaca_error(&resp), "canwrite failed for switch {}", id);
    assert_eq!(
        alpaca_value(&resp),
        true,
        "switch {} should be writable",
        id
    );
}

#[then(expr = "setting switch {int} value to {float} should succeed")]
async fn setting_switch_value_should_succeed(world: &mut PpbaWorld, id: i32, value: f64) {
    let url = world.switch_url();
    let id_str = id.to_string();
    let val_str = value.to_string();
    let resp = alpaca_put(
        &url,
        "setswitchvalue",
        &[("Id", &id_str), ("Value", &val_str)],
    )
    .await;
    assert!(
        !is_alpaca_error(&resp),
        "setswitchvalue({}, {}) failed: {}",
        id,
        value,
        alpaca_error_message(&resp)
    );
}

#[then(
    expr = "all {int} switches should be queryable for name, description, min, max, step, value, and can_write"
)]
async fn all_switches_queryable(world: &mut PpbaWorld, count: i32) {
    let url = world.switch_url();
    for id in 0..count {
        let resp = alpaca_get(&url, &format!("getswitchname?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "getswitchname failed for switch {}",
            id
        );

        let resp = alpaca_get(&url, &format!("getswitchdescription?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "getswitchdescription failed for switch {}",
            id
        );

        let resp = alpaca_get(&url, &format!("minswitchvalue?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "minswitchvalue failed for switch {}",
            id
        );

        let resp = alpaca_get(&url, &format!("maxswitchvalue?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "maxswitchvalue failed for switch {}",
            id
        );

        let resp = alpaca_get(&url, &format!("switchstep?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "switchstep failed for switch {}",
            id
        );

        let resp = alpaca_get(&url, &format!("getswitchvalue?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "getswitchvalue failed for switch {}",
            id
        );

        let resp = alpaca_get(&url, &format!("canwrite?Id={}", id)).await;
        assert!(!is_alpaca_error(&resp), "canwrite failed for switch {}", id);
    }
}

#[then(expr = "get_switch should work for switches {int} through {int}")]
async fn get_switch_should_work(world: &mut PpbaWorld, from: i32, to: i32) {
    let url = world.switch_url();
    for id in from..=to {
        let resp = alpaca_get(&url, &format!("getswitch?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "getswitch failed for switch {}",
            id
        );
    }
}
