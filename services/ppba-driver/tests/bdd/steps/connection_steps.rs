//! Step definitions for connection_lifecycle.feature
//!
//! Also defines shared steps used across multiple features:
//! - "Given a running PPBA server"
//! - "When I connect/disconnect the switch/OC device"
//! - "Then the last operation should have failed"
//! - "Then the last error code should be {word}"

use crate::steps::infrastructure::*;
use crate::world::PpbaWorld;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a running PPBA server")]
async fn a_running_ppba_server(world: &mut PpbaWorld) {
    world.config = default_test_config();
    world.start_ppba().await;
}

// ============================================================================
// When steps
// ============================================================================

#[when("I connect the switch device")]
async fn connect_switch_device(world: &mut PpbaWorld) {
    let url = world.switch_url();
    let resp = alpaca_put(&url, "connected", &[("Connected", "true")]).await;
    assert!(
        !is_alpaca_error(&resp),
        "connecting switch failed: {}",
        alpaca_error_message(&resp)
    );
}

#[when("I disconnect the switch device")]
async fn disconnect_switch_device(world: &mut PpbaWorld) {
    let url = world.switch_url();
    let resp = alpaca_put(&url, "connected", &[("Connected", "false")]).await;
    assert!(
        !is_alpaca_error(&resp),
        "disconnecting switch failed: {}",
        alpaca_error_message(&resp)
    );
}

#[when("I try to connect the switch device")]
async fn try_connect_switch_device(world: &mut PpbaWorld) {
    let url = world.switch_url();
    let resp = alpaca_put(&url, "connected", &[("Connected", "true")]).await;
    world.capture_response(&resp);
}

#[when("I connect the OC device")]
async fn connect_oc_device(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_put(&url, "connected", &[("Connected", "true")]).await;
    assert!(
        !is_alpaca_error(&resp),
        "connecting OC failed: {}",
        alpaca_error_message(&resp)
    );
}

#[when("I disconnect the OC device")]
async fn disconnect_oc_device(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_put(&url, "connected", &[("Connected", "false")]).await;
    assert!(
        !is_alpaca_error(&resp),
        "disconnecting OC failed: {}",
        alpaca_error_message(&resp)
    );
}

#[when("I try to connect the OC device")]
async fn try_connect_oc_device(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_put(&url, "connected", &[("Connected", "true")]).await;
    world.capture_response(&resp);
}

#[when(expr = "I cycle the switch device connection {int} times")]
async fn cycle_switch_device_connection(world: &mut PpbaWorld, count: usize) {
    let url = world.switch_url();
    for _ in 0..count {
        let resp = alpaca_put(&url, "connected", &[("Connected", "true")]).await;
        assert!(!is_alpaca_error(&resp), "connect failed during cycle");

        let resp = alpaca_get(&url, "connected").await;
        assert_eq!(
            alpaca_value(&resp),
            true,
            "switch should be connected during cycle"
        );

        let resp = alpaca_put(&url, "connected", &[("Connected", "false")]).await;
        assert!(!is_alpaca_error(&resp), "disconnect failed during cycle");

        let resp = alpaca_get(&url, "connected").await;
        assert_eq!(
            alpaca_value(&resp),
            false,
            "switch should be disconnected during cycle"
        );
    }
}

// ============================================================================
// Then steps
// ============================================================================

#[then("the switch device should report connected")]
async fn switch_device_should_report_connected(world: &mut PpbaWorld) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, "connected").await;
    assert_eq!(alpaca_value(&resp), true, "switch should report connected");
}

#[then("the switch device should report disconnected")]
async fn switch_device_should_report_disconnected(world: &mut PpbaWorld) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, "connected").await;
    assert_eq!(
        alpaca_value(&resp),
        false,
        "switch should report disconnected"
    );
}

#[then("the OC device should report connected")]
async fn oc_device_should_report_connected(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "connected").await;
    assert_eq!(alpaca_value(&resp), true, "OC should report connected");
}

#[then("the OC device should report disconnected")]
async fn oc_device_should_report_disconnected(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "connected").await;
    assert_eq!(alpaca_value(&resp), false, "OC should report disconnected");
}

#[then("the last operation should have failed")]
fn last_operation_should_have_failed(world: &mut PpbaWorld) {
    assert!(
        world.last_error_number.is_some(),
        "expected an error but none occurred"
    );
}

#[then(expr = "the last error code should be {word}")]
fn last_error_code_should_be(world: &mut PpbaWorld, expected_code: String) {
    let actual = world
        .last_error_number
        .expect("expected an error code but none was set");
    let expected = match expected_code.as_str() {
        "NOT_CONNECTED" => ASCOMErrorCode::NOT_CONNECTED.raw() as i64,
        "INVALID_VALUE" => ASCOMErrorCode::INVALID_VALUE.raw() as i64,
        "INVALID_OPERATION" => ASCOMErrorCode::INVALID_OPERATION.raw() as i64,
        "NOT_IMPLEMENTED" => ASCOMErrorCode::NOT_IMPLEMENTED.raw() as i64,
        "VALUE_NOT_SET" => ASCOMErrorCode::VALUE_NOT_SET.raw() as i64,
        other => panic!("Unknown ASCOM error code: {}", other),
    };
    assert_eq!(
        actual, expected,
        "expected error code {} ({}), got {} (message: {:?})",
        expected_code, expected, actual, world.last_error_message
    );
}
