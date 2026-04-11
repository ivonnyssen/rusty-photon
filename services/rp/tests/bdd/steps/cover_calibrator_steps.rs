//! BDD step definitions for CoverCalibrator MCP tools

use cucumber::{given, then, when};

use crate::steps::infrastructure::OmniSimHandle;
use crate::steps::tool_steps::start_rp;
use crate::world::{CoverCalibratorConfig, RpWorld};

// --- Given steps ---

#[given("rp is running with a cover calibrator on the simulator")]
async fn rp_running_with_cover_calibrator(world: &mut RpWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
    add_cover_calibrator(world);
    start_rp(world).await;
}

#[given(expr = "rp is running with a cover calibrator at {string} device {int}")]
async fn rp_running_with_cover_calibrator_at(world: &mut RpWorld, url: String, device_number: i32) {
    world.cover_calibrators.push(CoverCalibratorConfig {
        id: "flat-panel".to_string(),
        alpaca_url: url,
        device_number: device_number as u32,
    });
    start_rp(world).await;
}

// --- When steps ---

#[when(expr = "the MCP client calls \"close_cover\" with calibrator {string}")]
async fn mcp_call_close_cover(world: &mut RpWorld, calibrator_id: String) {
    call_calibrator_tool(world, "close_cover", &calibrator_id, None).await;
}

#[when(expr = "the MCP client calls \"open_cover\" with calibrator {string}")]
async fn mcp_call_open_cover(world: &mut RpWorld, calibrator_id: String) {
    call_calibrator_tool(world, "open_cover", &calibrator_id, None).await;
}

#[when(expr = "the MCP client calls \"calibrator_on\" with calibrator {string}")]
async fn mcp_call_calibrator_on(world: &mut RpWorld, calibrator_id: String) {
    call_calibrator_tool(world, "calibrator_on", &calibrator_id, None).await;
}

#[when(
    expr = "the MCP client calls \"calibrator_on\" with calibrator {string} and brightness {int}"
)]
async fn mcp_call_calibrator_on_brightness(
    world: &mut RpWorld,
    calibrator_id: String,
    brightness: i32,
) {
    call_calibrator_tool(
        world,
        "calibrator_on",
        &calibrator_id,
        Some(brightness as u32),
    )
    .await;
}

#[when(expr = "the MCP client calls \"calibrator_off\" with calibrator {string}")]
async fn mcp_call_calibrator_off(world: &mut RpWorld, calibrator_id: String) {
    call_calibrator_tool(world, "calibrator_off", &calibrator_id, None).await;
}

#[when("the MCP client calls \"close_cover\" with no calibrator_id")]
async fn mcp_call_close_cover_no_id(world: &mut RpWorld) {
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "close_cover",
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

#[then("the tool call should succeed")]
fn tool_call_succeeded(world: &mut RpWorld) {
    let result = world.last_tool_result.as_ref().expect("no tool result");

    assert!(
        result.is_ok(),
        "expected tool call to succeed, got error: {:?}",
        result
    );
}

// --- Helpers ---

pub fn add_cover_calibrator(world: &mut RpWorld) {
    if world.cover_calibrators.is_empty() {
        let url = world.omnisim_url();
        world.cover_calibrators.push(CoverCalibratorConfig {
            id: "flat-panel".to_string(),
            alpaca_url: url,
            device_number: 0,
        });
    }
}

async fn call_calibrator_tool(
    world: &mut RpWorld,
    tool_name: &str,
    calibrator_id: &str,
    brightness: Option<u32>,
) {
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();

    let mut args = serde_json::json!({
        "calibrator_id": calibrator_id
    });
    if let Some(b) = brightness {
        args["brightness"] = serde_json::json!(b);
    }

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": tool_name,
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
            } else {
                world.last_tool_result = Some(Ok(body["result"].clone()));
            }
        }
        Err(e) => {
            world.last_tool_result = Some(Err(e.to_string()));
        }
    }
}
