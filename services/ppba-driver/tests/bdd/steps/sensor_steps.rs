//! Step definitions for sensor_readings.feature
//!
//! The feature file uses "Given a running PPBA server with the switch connected"
//! (defined in switch_control_steps) and "When I wait for the switch data to be available"
//! (also in switch_control_steps). This file only defines the Then assertions
//! specific to sensor value checks.

use crate::steps::infrastructure::*;
use crate::world::PpbaWorld;
use cucumber::then;

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "switch {int} value should be approximately {float}")]
async fn switch_value_approximately(world: &mut PpbaWorld, id: i32, expected: f64) {
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
        (value - expected).abs() < 0.1,
        "switch {} value should be ~{}, got {}",
        id,
        expected,
        value
    );
}

#[then(expr = "switch {int} value should be in range {float} to {float}")]
async fn switch_value_in_range(world: &mut PpbaWorld, id: i32, min: f64, max: f64) {
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
        (min..=max).contains(&value),
        "switch {} value {} should be in range {}..={}",
        id,
        value,
        min,
        max
    );
}

#[then(expr = "switch {int} value should be non-negative")]
async fn switch_value_non_negative(world: &mut PpbaWorld, id: i32) {
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
        value >= 0.0,
        "switch {} value should be >= 0, got {}",
        id,
        value
    );
}

#[then(expr = "switch {int} value should be 0.0 or 1.0")]
async fn switch_value_boolean_range(world: &mut PpbaWorld, id: i32) {
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
        value == 0.0 || value == 1.0,
        "switch {} should be 0.0 or 1.0, got {}",
        id,
        value
    );
}

#[then(expr = "switch {int} value should be positive")]
async fn switch_value_positive(world: &mut PpbaWorld, id: i32) {
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
        value > 0.0,
        "switch {} value should be positive, got {}",
        id,
        value
    );
}
