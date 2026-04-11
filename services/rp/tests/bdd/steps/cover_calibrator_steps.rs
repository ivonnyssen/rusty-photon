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

// --- Diagnostic steps ---

/// Exercise OmniSim's CoverCalibrator directly (no rp in the middle)
/// to isolate timing behavior. Prints detailed per-call timing to stderr.
#[when("the cover calibrator is exercised directly against OmniSim with timing")]
async fn diag_exercise_cover_directly(world: &mut RpWorld) {
    let base = world.omnisim_url();
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();

    // Step 1: Connect
    let t0 = std::time::Instant::now();
    let resp = client
        .put(format!("{}/api/v1/covercalibrator/0/connected", base))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("Connected=true")
        .send()
        .await
        .expect("connect failed");
    let body = resp.text().await.unwrap_or_default();
    eprintln!(
        "[DIAG] connect: {:?} | response: {}",
        t0.elapsed(),
        &body[..body.len().min(200)]
    );

    // Step 2: Read initial cover state
    let t1 = std::time::Instant::now();
    let resp = client
        .get(format!("{}/api/v1/covercalibrator/0/coverstate", base))
        .send()
        .await
        .expect("coverstate failed");
    let body = resp.text().await.unwrap_or_default();
    eprintln!(
        "[DIAG] initial coverstate: {:?} | response: {}",
        t1.elapsed(),
        &body[..body.len().min(200)]
    );

    // Step 3: Call OpenCover (in case it starts Closed, we open first to test close)
    let t2 = std::time::Instant::now();
    let resp = client
        .put(format!("{}/api/v1/covercalibrator/0/opencover", base))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("")
        .send()
        .await
        .expect("opencover failed");
    let body = resp.text().await.unwrap_or_default();
    eprintln!(
        "[DIAG] opencover PUT: {:?} | response: {}",
        t2.elapsed(),
        &body[..body.len().min(200)]
    );

    // Step 4: Poll coverstate until Open (max 60s)
    let poll_start = std::time::Instant::now();
    let mut poll_count = 0u32;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        poll_count += 1;
        let t = std::time::Instant::now();
        let resp = client
            .get(format!("{}/api/v1/covercalibrator/0/coverstate", base))
            .send()
            .await
            .expect("poll failed");
        let body = resp.text().await.unwrap_or_default();
        let http_time = t.elapsed();
        eprintln!(
            "[DIAG] poll #{} coverstate: {:?} (http: {:?}) | {}",
            poll_count,
            poll_start.elapsed(),
            http_time,
            &body[..body.len().min(200)]
        );

        // Check if Value is 3 (Open)
        if body.contains("\"Value\":3") {
            eprintln!(
                "[DIAG] cover reached Open after {} polls, {:?} total",
                poll_count,
                poll_start.elapsed()
            );
            break;
        }
        if poll_start.elapsed() > std::time::Duration::from_secs(60) {
            eprintln!(
                "[DIAG] TIMEOUT waiting for Open after {:?}",
                poll_start.elapsed()
            );
            break;
        }
    }

    // Step 5: Call CloseCover
    let t3 = std::time::Instant::now();
    let resp = client
        .put(format!("{}/api/v1/covercalibrator/0/closecover", base))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("")
        .send()
        .await
        .expect("closecover failed");
    let body = resp.text().await.unwrap_or_default();
    eprintln!(
        "[DIAG] closecover PUT: {:?} | response: {}",
        t3.elapsed(),
        &body[..body.len().min(200)]
    );

    // Step 6: Poll coverstate until Closed (max 60s)
    let poll_start2 = std::time::Instant::now();
    let mut poll_count2 = 0u32;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        poll_count2 += 1;
        let t = std::time::Instant::now();
        let resp = client
            .get(format!("{}/api/v1/covercalibrator/0/coverstate", base))
            .send()
            .await
            .expect("poll failed");
        let body = resp.text().await.unwrap_or_default();
        let http_time = t.elapsed();
        eprintln!(
            "[DIAG] poll #{} coverstate: {:?} (http: {:?}) | {}",
            poll_count2,
            poll_start2.elapsed(),
            http_time,
            &body[..body.len().min(200)]
        );

        if body.contains("\"Value\":1") {
            eprintln!(
                "[DIAG] cover reached Closed after {} polls, {:?} total",
                poll_count2,
                poll_start2.elapsed()
            );
            break;
        }
        if poll_start2.elapsed() > std::time::Duration::from_secs(60) {
            eprintln!(
                "[DIAG] TIMEOUT waiting for Closed after {:?}",
                poll_start2.elapsed()
            );
            break;
        }
    }

    let total = start.elapsed();
    eprintln!("[DIAG] Total diagnostic time: {:?}", total);
    world.diag_cover_duration = Some(total);
}

#[then("the direct cover operation should have completed within 30 seconds")]
fn diag_check_timing(world: &mut RpWorld) {
    let duration = world.diag_cover_duration.expect("diagnostic did not run");
    eprintln!("[DIAG] Asserting duration {:?} < 30s", duration);
    assert!(
        duration < std::time::Duration::from_secs(30),
        "Direct OmniSim cover operations took {:?}, expected < 30s. \
         See [DIAG] output above for per-call timing.",
        duration
    );
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
