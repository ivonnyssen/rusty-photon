//! BDD step definitions for compute_image_stats MCP tool
//!
//! The capture step is defined in tool_steps.rs (shared across features).
//! It stores last_image_path and last_document_id on the world for chaining.
//!
//! Shared steps reused from measure_basic_steps.rs:
//! (`I fetch the exposure document for the captured document_id`,
//! `the exposure document should contain a section named ...`, and
//! `the {string} section should contain {string}`).

use cucumber::{then, when};

use crate::steps::tool_steps::ensure_mcp_client;
use crate::world::RpWorld;

// --- When steps ---

#[when("the MCP client calls \"compute_image_stats\" with the captured image path")]
async fn mcp_call_compute_stats_with_last_path(world: &mut RpWorld) {
    let image_path = world
        .last_image_path
        .clone()
        .expect("no captured image path available");

    call_compute_image_stats(world, Some(&image_path), None).await;
}

#[when("the MCP client calls \"compute_image_stats\" with the captured document_id")]
async fn mcp_call_compute_stats_with_last_document_id(world: &mut RpWorld) {
    let document_id = world
        .last_document_id
        .clone()
        .expect("no captured document_id available");

    call_compute_image_stats(world, None, Some(&document_id)).await;
}

#[when(expr = "the MCP client calls \"compute_image_stats\" with image path {string}")]
async fn mcp_call_compute_stats_with_path(world: &mut RpWorld, image_path: String) {
    call_compute_image_stats(world, Some(&image_path), None).await;
}

#[when(expr = "the MCP client calls \"compute_image_stats\" with document_id {string}")]
async fn mcp_call_compute_stats_with_document_id(world: &mut RpWorld, document_id: String) {
    call_compute_image_stats(world, None, Some(&document_id)).await;
}

#[when("the MCP client calls \"compute_image_stats\" with no arguments")]
async fn mcp_call_compute_stats_no_args(world: &mut RpWorld) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool("compute_image_stats", serde_json::json!({}))
        .await;

    match &result {
        Ok(v) => world.last_image_stats = Some(v.clone()),
        Err(_) => world.last_image_stats = None,
    }
    world.last_tool_result = Some(result);
}

// --- Then steps ---

#[then(expr = "the image stats result should contain {string} as a non-negative integer")]
fn stats_contains_non_negative_integer(world: &mut RpWorld, field: String) {
    let stats = world
        .last_image_stats
        .as_ref()
        .expect("no image stats result");

    let value = stats
        .get(&field)
        .unwrap_or_else(|| panic!("expected '{}' in image stats, got: {:?}", field, stats));

    assert!(
        value.as_u64().is_some() || value.as_i64().is_some_and(|v| v >= 0),
        "expected '{}' to be a non-negative integer, got: {:?}",
        field,
        value
    );
}

#[then(expr = "the image stats result should contain {string} as a non-negative number")]
fn stats_contains_non_negative_number(world: &mut RpWorld, field: String) {
    let stats = world
        .last_image_stats
        .as_ref()
        .expect("no image stats result");

    let value = stats
        .get(&field)
        .unwrap_or_else(|| panic!("expected '{}' in image stats, got: {:?}", field, stats));

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

#[then(expr = "the image stats result should contain {string} as a positive integer")]
fn stats_contains_positive_integer(world: &mut RpWorld, field: String) {
    let stats = world
        .last_image_stats
        .as_ref()
        .expect("no image stats result");

    let value = stats
        .get(&field)
        .unwrap_or_else(|| panic!("expected '{}' in image stats, got: {:?}", field, stats));

    let num = value.as_u64().unwrap_or_else(|| {
        panic!(
            "expected '{}' to be a non-negative integer, got: {:?}",
            field, value
        )
    });

    assert!(num > 0, "expected '{}' to be positive, got: {}", field, num);
}

#[then(expr = "the image stats result should contain {string}")]
fn stats_contains_field(world: &mut RpWorld, field: String) {
    let stats = world
        .last_image_stats
        .as_ref()
        .expect("no image stats result");

    assert!(
        stats.get(&field).is_some(),
        "expected '{}' in image stats, got: {:?}",
        field,
        stats
    );
}

// --- Helpers ---

async fn call_compute_image_stats(
    world: &mut RpWorld,
    image_path: Option<&str>,
    document_id: Option<&str>,
) {
    ensure_mcp_client(world).await;
    let mut args = serde_json::json!({});
    if let Some(path) = image_path {
        args["image_path"] = serde_json::json!(path);
    }
    if let Some(doc_id) = document_id {
        args["document_id"] = serde_json::json!(doc_id);
    }

    let result = world.mcp().call_tool("compute_image_stats", args).await;

    match &result {
        Ok(v) => world.last_image_stats = Some(v.clone()),
        Err(_) => world.last_image_stats = None,
    }
    world.last_tool_result = Some(result);
}
