//! BDD step definitions for safety enforcement (rp.md § Safety): a
//! SafetyMonitor unsafe transition interrupts the active session and
//! gates `/mcp`; the safe transition re-invokes the orchestrator with
//! recovery context.
//!
//! The monitor is OmniSim's safety-monitor simulator; its reported
//! `IsSafe` is flipped at runtime through OmniSim's private
//! `issafesetting` endpoint.

use std::time::Duration;

use cucumber::{given, then, when};

use bdd_infra::rp_harness::{OmniSimHandle, SafetyMonitorConfig};

use crate::world::RpWorld;

/// How long a poll-until-observed step waits before failing the scenario.
const OBSERVATION_BUDGET: Duration = Duration::from_secs(5);

#[given("a safety monitor on the simulator")]
async fn safety_monitor_on_simulator(world: &mut RpWorld) {
    crate::steps::tool_steps::ensure_omnisim(world).await;
    // Start from a known-safe reading regardless of what a previous
    // scenario (or crashed run) left in the simulator's memory.
    OmniSimHandle::set_safety_monitor_is_safe(true)
        .await
        .expect("failed to reset OmniSim's safety monitor to safe");
    world.safety_monitors.push(SafetyMonitorConfig {
        id: "weather-watcher".to_string(),
        alpaca_url: world.omnisim_url(),
        device_number: 0,
    });
    // Fast polling so transitions are detected in test time, not the
    // production default (10 s).
    world.safety_poll_interval = Some(Duration::from_millis(250));
}

#[when("the safety monitor reports unsafe")]
async fn safety_monitor_reports_unsafe(_world: &mut RpWorld) {
    OmniSimHandle::set_safety_monitor_is_safe(false)
        .await
        .expect("failed to flip OmniSim's safety monitor to unsafe");
}

#[when("the safety monitor reports safe again")]
async fn safety_monitor_reports_safe(_world: &mut RpWorld) {
    OmniSimHandle::set_safety_monitor_is_safe(true)
        .await
        .expect("failed to flip OmniSim's safety monitor to safe");
}

#[then(expr = "the test orchestrator should have been re-invoked with recovery reason {string}")]
async fn orchestrator_reinvoked_with_recovery(world: &mut RpWorld, reason: String) {
    let deadline = std::time::Instant::now() + OBSERVATION_BUDGET;
    loop {
        {
            let invocations = world.orchestrator_invocations.read().await;
            if invocations.len() >= 2 {
                let recovery = invocations
                    .last()
                    .and_then(|inv| inv.recovery.clone())
                    .expect("the re-invocation carries no `recovery` key at all");
                assert_eq!(
                    recovery.get("reason").and_then(|v| v.as_str()),
                    Some(reason.as_str()),
                    "unexpected recovery object: {recovery}"
                );
                return;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the orchestrator was not re-invoked within {OBSERVATION_BUDGET:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[then("the recovery invocation should carry the original workflow and session ids")]
async fn recovery_invocation_carries_original_ids(world: &mut RpWorld) {
    let invocations = world.orchestrator_invocations.read().await;
    let first = invocations.first().expect("no invocations recorded");
    let last = invocations.last().expect("no invocations recorded");
    assert_eq!(
        (&first.workflow_id, &first.session_id),
        (&last.workflow_id, &last.session_id),
        "the recovery invocation must reuse the interrupted session's ids"
    );
}

#[then(expr = "the MCP endpoint should reject requests with 503 within {int} seconds")]
async fn mcp_rejects_with_503(world: &mut RpWorld, seconds: u64) {
    assert!(
        poll_mcp_gate(world, true, Duration::from_secs(seconds)).await,
        "the MCP endpoint never answered 503 while conditions were unsafe"
    );
}

#[then(expr = "the MCP endpoint should accept requests again within {int} seconds")]
async fn mcp_accepts_again(world: &mut RpWorld, seconds: u64) {
    assert!(
        poll_mcp_gate(world, false, Duration::from_secs(seconds)).await,
        "the MCP endpoint kept answering 503 after conditions returned to safe"
    );
}

/// Poll `POST /mcp` until its status is (or stops being) 503. The body is
/// a JSON-RPC `initialize` so an ungated rp answers with a normal MCP
/// response; the step only discriminates on the 503 gate, not on rmcp's
/// protocol details.
async fn poll_mcp_gate(world: &mut RpWorld, expect_gated: bool, budget: Duration) -> bool {
    let client = reqwest::Client::new();
    let url = world.rp_mcp_url();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "bdd-gate-probe", "version": "0" }
        }
    });
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if let Ok(resp) = client
            .post(&url)
            .header("accept", "application/json, text/event-stream")
            .json(&body)
            .send()
            .await
        {
            let gated = resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE;
            if gated == expect_gated {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}
