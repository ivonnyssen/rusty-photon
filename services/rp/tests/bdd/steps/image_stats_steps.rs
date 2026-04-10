//! BDD step definitions for compute_image_stats MCP tool

use cucumber::{then, when};

use crate::world::RpWorld;

// --- When steps ---

#[when(expr = "the MCP client calls \"capture\" with camera {string} for {int} ms")]
async fn mcp_call_capture_ms(world: &mut RpWorld, camera_id: String, duration_ms: i32) {
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "capture",
                "arguments": {
                    "camera_id": camera_id,
                    "duration_ms": duration_ms
                }
            }
        }))
        .send()
        .await;

    match resp {
        Ok(r) => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            if body.get("error").is_some() {
                world.last_tool_result = Some(Err(body["error"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_string()));
            } else {
                let result = &body["result"];
                world.last_image_path = result
                    .get("image_path")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                world.last_document_id = result
                    .get("document_id")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                world.last_tool_result = Some(Ok(result.clone()));
            }
        }
        Err(e) => {
            world.last_tool_result = Some(Err(e.to_string()));
        }
    }
}

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
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "compute_image_stats",
                "arguments": {}
            }
        }))
        .send()
        .await;

    match resp {
        Ok(r) => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            if body.get("error").is_some() {
                world.last_tool_result = Some(Err(body["error"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_string()));
            } else {
                world.last_tool_result = Some(Ok(body["result"].clone()));
            }
        }
        Err(e) => {
            world.last_tool_result = Some(Err(e.to_string()));
        }
    }
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
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let mut args = serde_json::json!({
        "image_path": image_path
    });
    if let Some(doc_id) = document_id {
        args["document_id"] = serde_json::json!(doc_id);
    }

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "compute_image_stats",
                "arguments": args
            }
        }))
        .send()
        .await;

    match resp {
        Ok(r) => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            if body.get("error").is_some() {
                world.last_tool_result = Some(Err(body["error"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_string()));
                world.last_image_stats = None;
            } else {
                let result = body["result"].clone();
                world.last_image_stats = Some(result.clone());
                world.last_tool_result = Some(Ok(result));
            }
        }
        Err(e) => {
            world.last_tool_result = Some(Err(e.to_string()));
            world.last_image_stats = None;
        }
    }
}
