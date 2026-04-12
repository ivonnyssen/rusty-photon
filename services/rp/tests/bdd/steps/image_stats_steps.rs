//! BDD step definitions for compute_image_stats MCP tool
//!
//! The capture step is defined in tool_steps.rs (shared across features).
//! It stores last_image_path and last_document_id on the world for chaining.

use cucumber::{then, when};

use crate::steps::mcp_test_client;
use crate::world::RpWorld;

// --- When steps ---

#[when("the MCP client calls \"compute_image_stats\" with the captured image path")]
async fn mcp_call_compute_stats_with_last_path(world: &mut RpWorld) {
    let image_path = world
        .last_image_path
        .clone()
        .expect("no captured image path available");
    let document_id = world.last_document_id.clone();

    call_compute_image_stats(world, &image_path, document_id.as_deref()).await;
}

#[when(expr = "the MCP client calls \"compute_image_stats\" with image path {string}")]
async fn mcp_call_compute_stats_with_path(world: &mut RpWorld, image_path: String) {
    call_compute_image_stats(world, &image_path, None).await;
}

#[when("the MCP client calls \"compute_image_stats\" with no image_path")]
async fn mcp_call_compute_stats_no_path(world: &mut RpWorld) {
    let url = world.rp_mcp_url();
    let result =
        mcp_test_client::call_tool(&url, "compute_image_stats", serde_json::json!({})).await;

    match &result {
        Ok(v) => {
            world.last_image_stats = Some(v.clone());
        }
        Err(_) => {
            world.last_image_stats = None;
        }
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

    let num = value
        .as_u64()
        .or_else(|| value.as_i64().map(|v| v as u64))
        .unwrap_or_else(|| panic!("expected '{}' to be an integer, got: {:?}", field, value));

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
    image_path: &str,
    document_id: Option<&str>,
) {
    let url = world.rp_mcp_url();
    let mut args = serde_json::json!({
        "image_path": image_path
    });
    if let Some(doc_id) = document_id {
        args["document_id"] = serde_json::json!(doc_id);
    }

    let result = mcp_test_client::call_tool(&url, "compute_image_stats", args).await;

    match &result {
        Ok(v) => {
            world.last_image_stats = Some(v.clone());
        }
        Err(_) => {
            world.last_image_stats = None;
        }
    }
    world.last_tool_result = Some(result);
}
