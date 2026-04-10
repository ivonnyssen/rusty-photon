//! BDD step definitions for get_camera_info MCP tool

use cucumber::{then, when};

use crate::world::RpWorld;

// --- When steps ---

#[when(expr = "the MCP client calls \"get_camera_info\" with camera {string}")]
async fn mcp_call_get_camera_info(world: &mut RpWorld, camera_id: String) {
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "get_camera_info",
                "arguments": {
                    "camera_id": camera_id
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
                world.last_tool_result = Some(Ok(body["result"].clone()));
            }
        }
        Err(e) => {
            world.last_tool_result = Some(Err(e.to_string()));
        }
    }
}

#[when("the MCP client calls \"get_camera_info\" with no camera_id")]
async fn mcp_call_get_camera_info_no_id(world: &mut RpWorld) {
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "get_camera_info",
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

#[then(expr = "the tool result should contain {string} as a positive integer")]
fn result_contains_positive_integer(world: &mut RpWorld, field: String) {
    let result = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("tool call failed");

    let value = result
        .get(&field)
        .unwrap_or_else(|| panic!("expected '{}' in tool result, got: {:?}", field, result));

    let num = value
        .as_u64()
        .or_else(|| value.as_i64().map(|v| v as u64))
        .unwrap_or_else(|| panic!("expected '{}' to be an integer, got: {:?}", field, value));

    assert!(num > 0, "expected '{}' to be positive, got: {}", field, num);
}

#[then(expr = "the tool result should contain {string}")]
fn result_contains_field(world: &mut RpWorld, field: String) {
    let result = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("tool call failed");

    assert!(
        result.get(&field).is_some(),
        "expected '{}' in tool result, got: {:?}",
        field,
        result
    );
}
