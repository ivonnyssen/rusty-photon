//! BDD step definitions for the `estimate_background` MCP tool.
//!
//! Shared steps live in `tool_steps.rs` (`capture`, `lists available
//! tools`, `the tool call should return an error`, `the error message
//! should contain ...`). The exposure-document fetch step
//! (`I fetch the exposure document for the captured document_id`) and the
//! generic section-presence assertions live in `measure_basic_steps.rs`
//! and are reused here -- both tools persist into the same exposure
//! document, so the document-fetch plumbing is shared.

use cucumber::{then, when};
use serde_json::Value;

use crate::steps::tool_steps::ensure_mcp_client;
use crate::world::RpWorld;

// --- When steps ---

#[when("the MCP client calls \"estimate_background\" with the captured image path")]
async fn mcp_call_with_last_path(world: &mut RpWorld) {
    let image_path = world
        .last_image_path
        .clone()
        .expect("no captured image path available");

    call_tool(world, Some(&image_path), None, None, None).await;
}

#[when("the MCP client calls \"estimate_background\" with the captured document_id")]
async fn mcp_call_with_last_document_id(world: &mut RpWorld) {
    let document_id = world
        .last_document_id
        .clone()
        .expect("no captured document_id available");

    call_tool(world, None, Some(&document_id), None, None).await;
}

#[when(
    expr = "the MCP client calls \"estimate_background\" with the captured image path, k {float} and max_iters {int}"
)]
async fn mcp_call_with_k_and_iters(world: &mut RpWorld, k: f64, max_iters: i64) {
    let image_path = world
        .last_image_path
        .clone()
        .expect("no captured image path available");

    call_tool(
        world,
        Some(&image_path),
        None,
        Some(k),
        Some(max_iters as u32),
    )
    .await;
}

#[when(
    expr = "the MCP client calls \"estimate_background\" with the captured image path and k {float}"
)]
async fn mcp_call_with_bad_k(world: &mut RpWorld, k: f64) {
    let image_path = world
        .last_image_path
        .clone()
        .expect("no captured image path available");

    call_tool(world, Some(&image_path), None, Some(k), None).await;
}

#[when(
    expr = "the MCP client calls \"estimate_background\" with the captured image path and max_iters {int}"
)]
async fn mcp_call_with_bad_iters(world: &mut RpWorld, max_iters: i64) {
    let image_path = world
        .last_image_path
        .clone()
        .expect("no captured image path available");

    call_tool(world, Some(&image_path), None, None, Some(max_iters as u32)).await;
}

#[when(expr = "the MCP client calls \"estimate_background\" with image path {string}")]
async fn mcp_call_with_path(world: &mut RpWorld, image_path: String) {
    call_tool(world, Some(&image_path), None, None, None).await;
}

#[when(expr = "the MCP client calls \"estimate_background\" with document_id {string}")]
async fn mcp_call_with_document_id(world: &mut RpWorld, document_id: String) {
    call_tool(world, None, Some(&document_id), None, None).await;
}

#[when("the MCP client calls \"estimate_background\" with no arguments")]
async fn mcp_call_no_args(world: &mut RpWorld) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool("estimate_background", serde_json::json!({}))
        .await;

    record_result(world, result);
}

// --- Then steps ---

#[then(expr = "the estimate_background result should contain {string} as a non-negative number")]
fn result_contains_non_negative_number(world: &mut RpWorld, field: String) {
    let result = result_or_panic(world);
    let value = result.get(&field).unwrap_or_else(|| {
        panic!(
            "expected '{}' in estimate_background result, got: {:?}",
            field, result
        )
    });

    let num = value
        .as_f64()
        .unwrap_or_else(|| panic!("expected '{}' to be a number, got: {:?}", field, value));

    assert!(
        num >= 0.0,
        "expected '{}' to be non-negative, got: {}",
        field,
        num
    );
}

#[then(expr = "the estimate_background result should contain {string} as a positive integer")]
fn result_contains_positive_integer(world: &mut RpWorld, field: String) {
    let result = result_or_panic(world);
    let value = result.get(&field).unwrap_or_else(|| {
        panic!(
            "expected '{}' in estimate_background result, got: {:?}",
            field, result
        )
    });

    let num = value
        .as_u64()
        .or_else(|| value.as_i64().map(|v| v as u64))
        .unwrap_or_else(|| panic!("expected '{}' to be an integer, got: {:?}", field, value));

    assert!(num > 0, "expected '{}' to be positive, got: {}", field, num);
}

// --- Helpers ---

async fn call_tool(
    world: &mut RpWorld,
    image_path: Option<&str>,
    document_id: Option<&str>,
    k: Option<f64>,
    max_iters: Option<u32>,
) {
    ensure_mcp_client(world).await;

    let mut args = serde_json::Map::new();
    if let Some(path) = image_path {
        args.insert("image_path".to_string(), Value::String(path.to_string()));
    }
    if let Some(doc_id) = document_id {
        args.insert("document_id".to_string(), Value::String(doc_id.to_string()));
    }
    if let Some(k) = k {
        args.insert("k".to_string(), serde_json::json!(k));
    }
    if let Some(it) = max_iters {
        args.insert("max_iters".to_string(), serde_json::json!(it));
    }

    let result = world
        .mcp()
        .call_tool("estimate_background", Value::Object(args))
        .await;

    record_result(world, result);
}

fn record_result(world: &mut RpWorld, result: Result<Value, String>) {
    match &result {
        Ok(v) => world.last_estimate_background_result = Some(v.clone()),
        Err(_) => world.last_estimate_background_result = None,
    }
    world.last_tool_result = Some(result);
}

fn result_or_panic(world: &RpWorld) -> &Value {
    world
        .last_estimate_background_result
        .as_ref()
        .expect("no estimate_background result")
}
