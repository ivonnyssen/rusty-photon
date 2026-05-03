//! BDD step definitions for `resolve_target` (catalog lookup) and
//! the structured not-found error payload.

use cucumber::{then, when};
use serde_json::Value;

use crate::steps::tool_steps::ensure_mcp_client;
use crate::world::RpWorld;

#[when(expr = "the MCP client calls \"resolve_target\" with name {string}")]
async fn call_resolve_target(world: &mut RpWorld, name: String) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool("resolve_target", serde_json::json!({"name": name}))
        .await;
    world.last_tool_result = Some(result);
}

#[then("the tool call should fail")]
fn tool_call_failed(world: &mut RpWorld) {
    let result = world.last_tool_result.as_ref().expect("no tool result");
    assert!(
        result.is_err(),
        "expected tool call to fail, got success: {:?}",
        result
    );
}

#[then(expr = "the resolved target name should be {string}")]
fn resolved_name(world: &mut RpWorld, expected: String) {
    let value = result_payload(world);
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .expect("resolved target payload missing `name`");
    assert_eq!(name, expected.as_str(), "resolved target name");
}

#[then(expr = "the resolved target ra_hours should be approximately {float}")]
fn resolved_ra(world: &mut RpWorld, expected: f64) {
    let value = result_payload(world);
    let ra = value
        .get("ra_hours")
        .and_then(|v| v.as_f64())
        .expect("resolved target payload missing `ra_hours`");
    // Catalog values are committed at ~6-decimal precision; the Gherkin
    // input is 4-decimal. Allow 0.001h ≈ 3.6 s of RA — well below any
    // observational tolerance and above the rounding noise.
    assert!(
        (ra - expected).abs() < 0.001,
        "ra_hours {ra} not within 0.001 of expected {expected}"
    );
}

#[then(expr = "the resolved target dec_degrees should be approximately {float}")]
fn resolved_dec(world: &mut RpWorld, expected: f64) {
    let value = result_payload(world);
    let dec = value
        .get("dec_degrees")
        .and_then(|v| v.as_f64())
        .expect("resolved target payload missing `dec_degrees`");
    assert!(
        (dec - expected).abs() < 0.001,
        "dec_degrees {dec} not within 0.001 of expected {expected}"
    );
}

#[then(expr = "the tool error payload should have field {string} equal to {string}")]
fn tool_error_field(world: &mut RpWorld, field: String, expected: String) {
    let payload = error_payload(world);
    let actual = payload
        .get(&field)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("error payload missing field {field:?}: {payload}"));
    assert_eq!(actual, expected.as_str(), "tool error payload {field}");
}

#[then("the tool error payload should carry a non-empty suggestions list")]
fn tool_error_has_suggestions(world: &mut RpWorld) {
    let payload = error_payload(world);
    let arr = payload
        .get("suggestions")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("error payload missing suggestions array: {payload}"));
    assert!(
        !arr.is_empty(),
        "expected non-empty suggestions list, got: {payload}"
    );
}

// --- Helpers ---

fn result_payload(world: &RpWorld) -> &Value {
    world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("expected tool call to succeed")
}

/// The `resolve_target` not-found path embeds JSON in the error
/// content text so a planner plugin can pick out structured fields
/// without parsing free-form text. Step expects the recorded error
/// string to be valid JSON.
fn error_payload(world: &RpWorld) -> Value {
    let result = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref();
    let err = match result {
        Err(e) => e.as_str(),
        Ok(_) => panic!("expected tool call error, got success"),
    };
    serde_json::from_str(err)
        .unwrap_or_else(|e| panic!("expected JSON error payload, got {err:?}: {e}"))
}
