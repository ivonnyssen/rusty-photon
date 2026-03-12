//! BDD step definitions for MCP tool execution feature

use cucumber::{given, then, when};

use crate::steps::infrastructure::{OmniSimHandle, RpHandle};
use crate::world::{CameraConfig, FilterWheelConfig, RpWorld};

// --- Given steps ---

#[given("rp is running with a camera on the simulator")]
async fn rp_running_with_camera(world: &mut RpWorld) {
    ensure_omnisim(world).await;
    add_camera(world);
    start_rp(world).await;
}

#[given("rp is running with a filter wheel on the simulator")]
async fn rp_running_with_filter_wheel(world: &mut RpWorld) {
    ensure_omnisim(world).await;
    add_filter_wheel(world);
    start_rp(world).await;
}

#[given("rp is running with a camera and filter wheel on the simulator")]
async fn rp_running_with_camera_and_filter_wheel(world: &mut RpWorld) {
    ensure_omnisim(world).await;
    add_camera(world);
    add_filter_wheel(world);
    start_rp(world).await;
}

#[given("an MCP client connected to rp")]
async fn mcp_client_connected(_world: &mut RpWorld) {
    // The MCP client is implicit — tool steps use HTTP POST to /mcp
    // with JSON-RPC payloads. No persistent connection needed for
    // the streamable HTTP transport.
}

// --- When steps ---

#[when(expr = "the MCP client calls \"capture\" with camera {string} for {int} second")]
async fn mcp_call_capture(world: &mut RpWorld, camera_id: String, duration_secs: i32) {
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
                    "duration_secs": duration_secs
                }
            }
        }))
        .send()
        .await;

    match resp {
        Ok(r) => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            if body.get("error").is_some() {
                world.last_tool_result = Some(Err(body["error"].to_string()));
            } else {
                world.last_tool_result = Some(Ok(body["result"].clone()));
            }
        }
        Err(e) => {
            world.last_tool_result = Some(Err(e.to_string()));
        }
    }
}

#[when(expr = "the MCP client calls \"set_filter\" with filter wheel {string} and filter {string}")]
async fn mcp_call_set_filter(world: &mut RpWorld, fw_id: String, filter_name: String) {
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "set_filter",
                "arguments": {
                    "filter_wheel_id": fw_id,
                    "filter_name": filter_name
                }
            }
        }))
        .send()
        .await;

    match resp {
        Ok(r) => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            if body.get("error").is_some() {
                world.last_tool_result = Some(Err(body["error"].to_string()));
            } else {
                world.last_tool_result = Some(Ok(body["result"].clone()));
            }
        }
        Err(e) => {
            world.last_tool_result = Some(Err(e.to_string()));
        }
    }
}

#[when(expr = "the MCP client calls \"get_filter\" with filter wheel {string}")]
async fn mcp_call_get_filter(world: &mut RpWorld, fw_id: String) {
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "get_filter",
                "arguments": {
                    "filter_wheel_id": fw_id
                }
            }
        }))
        .send()
        .await;

    match resp {
        Ok(r) => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            if body.get("error").is_some() {
                world.last_tool_result = Some(Err(body["error"].to_string()));
            } else {
                let result = &body["result"];
                world.current_filter = result
                    .get("filter_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                world.last_tool_result = Some(Ok(result.clone()));
            }
        }
        Err(e) => {
            world.last_tool_result = Some(Err(e.to_string()));
        }
    }
}

#[when("the MCP client lists available tools")]
async fn mcp_list_tools(world: &mut RpWorld) {
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await;

    match resp {
        Ok(r) => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            let tools = body["result"]["tools"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            world.last_tool_list = Some(tools);
        }
        Err(_) => {
            world.last_tool_list = Some(vec![]);
        }
    }
}

// --- Then steps ---

#[then("the tool result should contain an image path")]
fn result_has_image_path(world: &mut RpWorld) {
    let result = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("tool call failed");

    assert!(
        result.get("image_path").and_then(|v| v.as_str()).is_some(),
        "expected image_path in tool result, got: {:?}",
        result
    );
}

#[then("the tool result should contain a document id")]
fn result_has_document_id(world: &mut RpWorld) {
    let result = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("tool call failed");

    assert!(
        result.get("document_id").and_then(|v| v.as_str()).is_some(),
        "expected document_id in tool result, got: {:?}",
        result
    );
}

#[then(expr = "the current filter should be {string}")]
fn current_filter_is(world: &mut RpWorld, expected: String) {
    assert_eq!(
        world.current_filter.as_deref(),
        Some(expected.as_str()),
        "expected filter '{}' but got '{:?}'",
        expected,
        world.current_filter
    );
}

#[then("the tool call should return an error")]
fn tool_call_returned_error(world: &mut RpWorld) {
    let result = world.last_tool_result.as_ref().expect("no tool result");

    assert!(
        result.is_err(),
        "expected tool call to return an error, got: {:?}",
        result
    );
}

#[then(expr = "the tool list should include {string}")]
fn tool_list_includes(world: &mut RpWorld, tool_name: String) {
    let tools = world.last_tool_list.as_ref().expect("no tool list");

    assert!(
        tools.contains(&tool_name),
        "expected tool '{}' in catalog, got: {:?}",
        tool_name,
        tools
    );
}

// --- Helpers (pub for reuse in other step files) ---

pub async fn ensure_omnisim(world: &mut RpWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
}

pub fn add_camera(world: &mut RpWorld) {
    if world.cameras.is_empty() {
        let url = world.omnisim_url();
        world.cameras.push(CameraConfig {
            id: "main-cam".to_string(),
            alpaca_url: url,
            device_number: 0,
        });
    }
}

pub fn add_filter_wheel(world: &mut RpWorld) {
    if world.filter_wheels.is_empty() {
        let url = world.omnisim_url();
        world.filter_wheels.push(FilterWheelConfig {
            id: "main-fw".to_string(),
            alpaca_url: url,
            device_number: 0,
            filters: vec![
                "Luminance".to_string(),
                "Red".to_string(),
                "Green".to_string(),
                "Blue".to_string(),
            ],
        });
    }
}

pub async fn start_rp(world: &mut RpWorld) {
    if world.rp.as_ref().is_some_and(|h| h.child.is_some()) {
        return;
    }

    // Use port 0 so the rp process binds to an OS-assigned port.
    // The actual port is discovered by parsing the process stdout.
    world.rp = Some(RpHandle {
        child: None,
        base_url: String::new(),
        port: 0,
        config_path: String::new(),
        stdout_drain: None,
    });

    let config = world.build_config();
    let config_path = std::env::temp_dir()
        .join(format!(
            "rp-test-config-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .to_string();
    tokio::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap())
        .await
        .expect("failed to write config");

    // Start rp — it discovers its own port from stdout
    world.rp = Some(RpHandle::start(&config_path).await);

    assert!(
        world.wait_for_rp_healthy().await,
        "rp did not become healthy within timeout"
    );
}
