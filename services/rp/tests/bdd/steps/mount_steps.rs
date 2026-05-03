//! BDD step definitions for Mount MCP tools.
//!
//! Singular mount per rp deployment — there's no `mount_id` parameter
//! anywhere in this file (or in the `mount.feature` scenarios that
//! drive these steps).

use std::time::Duration;

use cucumber::{given, then, when};
use serde_json::Value;

use bdd_infra::rp_harness::{MountConfig, OmniSimHandle};

use crate::steps::tool_steps::{ensure_mcp_client, start_rp};
use crate::world::RpWorld;

// --- Given steps ---

#[given("rp is running with a mount on the simulator")]
async fn rp_running_with_mount(world: &mut RpWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
    let url = world.omnisim_url();
    world.mount = Some(MountConfig {
        alpaca_url: url,
        device_number: 0,
        settle_after_slew: None,
    });
    start_rp(world).await;
}

#[given("rp is running without a mount")]
async fn rp_running_without_mount(world: &mut RpWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
    world.mount = None;
    start_rp(world).await;
}

#[given(expr = "rp is running with a mount at {string} device {int}")]
async fn rp_running_with_mount_at(world: &mut RpWorld, url: String, device_number: i32) {
    let device_number = u32::try_from(device_number)
        .expect("device_number in mount scenarios must be non-negative");
    world.mount = Some(MountConfig {
        alpaca_url: url,
        device_number,
        settle_after_slew: None,
    });
    start_rp(world).await;
}

#[given(expr = "rp is running with a mount on the simulator and {int}ms settle")]
async fn rp_running_with_mount_and_settle(world: &mut RpWorld, settle_ms: i64) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
    let url = world.omnisim_url();
    let settle_ms = u64::try_from(settle_ms).expect("settle_ms must be non-negative");
    world.mount = Some(MountConfig {
        alpaca_url: url,
        device_number: 0,
        settle_after_slew: Some(Duration::from_millis(settle_ms)),
    });
    start_rp(world).await;
}

#[given(expr = "the mount tracking is set to {word}")]
async fn mount_tracking_set_to(world: &mut RpWorld, value: String) {
    let enabled: bool = match value.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("expected true|false for tracking, got {other}"),
    };
    ensure_mcp_client(world).await;
    // Given step: fail fast on setup problems per testing.md §3.3.
    // If `set_tracking` fails (e.g., misconfigured mount or
    // CanSetTracking == false in the simulator), the scenario should
    // surface that directly rather than failing later with a
    // less-direct symptom from the When step.
    world
        .mcp()
        .call_tool("set_tracking", serde_json::json!({ "enabled": enabled }))
        .await
        .expect("set_tracking should succeed in scenario setup");
}

// --- When steps ---

/// Slew step that interprets `MISSING` in either ra/dec column as
/// "omit that field from the JSON-RPC params" — drives the
/// missing-parameter Outline rows.
#[when(expr = "the MCP client calls \"slew\" with ra {string} dec {string}")]
async fn mcp_call_slew(world: &mut RpWorld, ra: String, dec: String) {
    ensure_mcp_client(world).await;
    let mut args = serde_json::Map::new();
    if ra != "MISSING" {
        let ra_val: f64 = ra
            .parse()
            .unwrap_or_else(|_| panic!("expected f64 or MISSING for ra, got {ra}"));
        args.insert("ra".to_string(), serde_json::json!(ra_val));
    }
    if dec != "MISSING" {
        let dec_val: f64 = dec
            .parse()
            .unwrap_or_else(|_| panic!("expected f64 or MISSING for dec, got {dec}"));
        args.insert("dec".to_string(), serde_json::json!(dec_val));
    }
    let result = world.mcp().call_tool("slew", Value::Object(args)).await;
    world.last_tool_result = Some(result);
}

#[when(expr = "the MCP client calls \"sync_mount\" with ra {string} dec {string}")]
async fn mcp_call_sync_mount(world: &mut RpWorld, ra: String, dec: String) {
    ensure_mcp_client(world).await;
    let mut args = serde_json::Map::new();
    if ra != "MISSING" {
        let ra_val: f64 = ra
            .parse()
            .unwrap_or_else(|_| panic!("expected f64 or MISSING for ra, got {ra}"));
        args.insert("ra".to_string(), serde_json::json!(ra_val));
    }
    if dec != "MISSING" {
        let dec_val: f64 = dec
            .parse()
            .unwrap_or_else(|_| panic!("expected f64 or MISSING for dec, got {dec}"));
        args.insert("dec".to_string(), serde_json::json!(dec_val));
    }
    let result = world
        .mcp()
        .call_tool("sync_mount", Value::Object(args))
        .await;
    world.last_tool_result = Some(result);
}

#[when("the MCP client calls \"get_mount_position\"")]
async fn mcp_call_get_mount_position(world: &mut RpWorld) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool("get_mount_position", serde_json::json!({}))
        .await;
    world.last_tool_result = Some(result);
}

#[when("the MCP client calls \"get_tracking\"")]
async fn mcp_call_get_tracking(world: &mut RpWorld) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool("get_tracking", serde_json::json!({}))
        .await;
    world.last_tool_result = Some(result);
}

#[when(expr = "the MCP client calls \"set_tracking\" with enabled {word}")]
async fn mcp_call_set_tracking(world: &mut RpWorld, enabled: String) {
    ensure_mcp_client(world).await;
    let enabled: bool = match enabled.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("expected true|false for enabled, got {other}"),
    };
    let result = world
        .mcp()
        .call_tool("set_tracking", serde_json::json!({ "enabled": enabled }))
        .await;
    world.last_tool_result = Some(result);
}

// --- Then steps ---

/// OmniSim's slew echo carries sub-arcsecond drift (likely from
/// internal topocentric ↔ J2000 transforms), so we assert tolerance,
/// not exact equality. `0.001` hours ≈ 3.6 arcsec on RA — well under
/// any centering workflow's tolerance and well above OmniSim's drift.
const SLEW_ECHO_TOLERANCE: f64 = 0.001;

#[then(expr = "the slew result actual_ra should be {float}")]
fn slew_actual_ra(world: &mut RpWorld, expected: f64) {
    let result = unwrap_ok(world);
    let actual = result
        .get("actual_ra")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| panic!("expected actual_ra field, got: {result:?}"));
    assert!(
        (actual - expected).abs() < SLEW_ECHO_TOLERANCE,
        "expected actual_ra ≈ {expected} (within {SLEW_ECHO_TOLERANCE}), got {actual}"
    );
}

#[then(expr = "the slew result actual_dec should be {float}")]
fn slew_actual_dec(world: &mut RpWorld, expected: f64) {
    let result = unwrap_ok(world);
    let actual = result
        .get("actual_dec")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| panic!("expected actual_dec field, got: {result:?}"));
    assert!(
        (actual - expected).abs() < SLEW_ECHO_TOLERANCE,
        "expected actual_dec ≈ {expected} (within {SLEW_ECHO_TOLERANCE}), got {actual}"
    );
}

#[then(expr = "the get_mount_position result ra should be {float}")]
fn get_mount_position_ra(world: &mut RpWorld, expected: f64) {
    let result = unwrap_ok(world);
    let actual = result
        .get("ra")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| panic!("expected ra field, got: {result:?}"));
    assert_eq!(actual, expected, "expected ra {expected}, got {actual}");
}

#[then(expr = "the get_mount_position result dec should be {float}")]
fn get_mount_position_dec(world: &mut RpWorld, expected: f64) {
    let result = unwrap_ok(world);
    let actual = result
        .get("dec")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| panic!("expected dec field, got: {result:?}"));
    assert_eq!(actual, expected, "expected dec {expected}, got {actual}");
}

#[then(expr = "the get_tracking result tracking should be {word}")]
fn get_tracking_tracking(world: &mut RpWorld, expected: String) {
    let result = unwrap_ok(world);
    let expected: bool = match expected.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("expected true|false, got {other}"),
    };
    let actual = result
        .get("tracking")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| panic!("expected tracking bool field, got: {result:?}"));
    assert_eq!(
        actual, expected,
        "expected tracking={expected}, got {actual}"
    );
}

#[then(expr = "the get_tracking result can_set_tracking should be {word}")]
fn get_tracking_can_set_tracking(world: &mut RpWorld, expected: String) {
    let result = unwrap_ok(world);
    let expected: bool = match expected.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("expected true|false, got {other}"),
    };
    let actual = result
        .get("can_set_tracking")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| panic!("expected can_set_tracking bool field, got: {result:?}"));
    assert_eq!(
        actual, expected,
        "expected can_set_tracking={expected}, got {actual}"
    );
}

// --- Helpers ---

fn unwrap_ok(world: &RpWorld) -> &Value {
    world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("tool call failed")
}
