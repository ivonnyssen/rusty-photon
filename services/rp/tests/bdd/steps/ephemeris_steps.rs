//! BDD step definitions for the ephemeris primitive MCP tools.

use cucumber::{given, then, when};
use serde_json::Value;

use crate::steps::tool_steps::ensure_mcp_client;
use crate::world::RpWorld;

// --- Given steps ---

#[given(expr = "rp is configured with site latitude {float} longitude {float}")]
fn site_configured(world: &mut RpWorld, lat: f64, lon: f64) {
    world.site = Some((lat, lon));
}

// --- When steps ---

/// Polaris ICRS coords (J2000.0): RA = 2.530... h, Dec = +89.264°.
const POLARIS_RA: f64 = 2.5301944;
const POLARIS_DEC: f64 = 89.2641111;

#[when("the MCP client calls \"compute_alt_az\" for Polaris")]
async fn call_alt_az_polaris(world: &mut RpWorld) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool(
            "compute_alt_az",
            serde_json::json!({"ra": POLARIS_RA, "dec": POLARIS_DEC}),
        )
        .await;
    world.last_tool_result = Some(result);
}

#[when(expr = "the MCP client calls \"compute_alt_az\" with ra {string} dec {string}")]
async fn call_alt_az_explicit(world: &mut RpWorld, ra: String, dec: String) {
    ensure_mcp_client(world).await;
    let ra: f64 = ra.parse().expect("ra must parse as f64");
    let dec: f64 = dec.parse().expect("dec must parse as f64");
    let result = world
        .mcp()
        .call_tool("compute_alt_az", serde_json::json!({"ra": ra, "dec": dec}))
        .await;
    world.last_tool_result = Some(result);
}

#[when(expr = "the MCP client calls \"get_local_sidereal_time\" with time {string}")]
async fn call_lst(world: &mut RpWorld, time: String) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool("get_local_sidereal_time", serde_json::json!({"time": time}))
        .await;
    world.last_tool_result = Some(result);
}

#[when(expr = "the MCP client calls \"get_target_status\" for target {string}")]
async fn call_target_status(world: &mut RpWorld, name: String) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool(
            "get_target_status",
            serde_json::json!({"target_name": name}),
        )
        .await;
    world.last_tool_result = Some(result);
}

#[when("the MCP client calls \"get_next_target\"")]
async fn call_next_target(world: &mut RpWorld) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool("get_next_target", serde_json::json!({}))
        .await;
    world.last_tool_result = Some(result);
}

#[then(expr = "the result target_name should be {string}")]
fn result_target_name(world: &mut RpWorld, expected: String) {
    let value = success_payload(world);
    let name = value
        .get("target_name")
        .and_then(|v| v.as_str())
        .expect("missing `target_name`");
    assert_eq!(name, expected.as_str());
}

#[then("the result altitude_degrees should be a finite number")]
fn result_altitude_finite(world: &mut RpWorld) {
    let value = success_payload(world);
    let alt = value
        .get("altitude_degrees")
        .and_then(|v| v.as_f64())
        .expect("missing `altitude_degrees`");
    assert!(alt.is_finite(), "altitude_degrees not finite: {alt}");
}

#[then(expr = "the result reason should be {string}")]
fn result_reason(world: &mut RpWorld, expected: String) {
    let value = success_payload(world);
    let reason = value
        .get("reason")
        .and_then(|v| v.as_str())
        .expect("missing `reason`");
    assert_eq!(reason, expected.as_str());
}

#[then("the result target should be null")]
fn result_target_null(world: &mut RpWorld) {
    let value = success_payload(world);
    assert!(
        value.get("target").is_some_and(|v| v.is_null()),
        "expected target=null, got: {value}"
    );
}

#[when(expr = "the MCP client calls \"get_twilight\" for date {string} kind {string}")]
async fn call_twilight(world: &mut RpWorld, date: String, kind: String) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool(
            "get_twilight",
            serde_json::json!({"date": date, "kind": kind}),
        )
        .await;
    world.last_tool_result = Some(result);
}

// --- Then steps ---

#[then("the result lst_hours should be in the range [0, 24)")]
fn lst_in_range(world: &mut RpWorld) {
    let value = success_payload(world);
    let lst = value
        .get("lst_hours")
        .and_then(|v| v.as_f64())
        .expect("missing `lst_hours`");
    assert!((0.0..24.0).contains(&lst), "lst_hours {lst} not in [0, 24)");
}

#[then(expr = "the result altitude_degrees should be approximately {float} within {float}")]
fn altitude_within(world: &mut RpWorld, expected: f64, tolerance: f64) {
    let value = success_payload(world);
    let alt = value
        .get("altitude_degrees")
        .and_then(|v| v.as_f64())
        .expect("missing `altitude_degrees`");
    assert!(
        (alt - expected).abs() < tolerance,
        "altitude_degrees {alt} not within {tolerance} of expected {expected}"
    );
}

#[then(expr = "the tool error message should mention {string}")]
fn error_mentions(world: &mut RpWorld, fragment: String) {
    let result = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref();
    let msg = match result {
        Err(e) => e.as_str(),
        Ok(_) => panic!("expected tool call error, got success"),
    };
    assert!(
        msg.contains(fragment.as_str()),
        "expected error to contain {fragment:?}, got: {msg}"
    );
}

// --- Helpers ---

fn success_payload(world: &RpWorld) -> &Value {
    world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("expected tool call to succeed")
}
