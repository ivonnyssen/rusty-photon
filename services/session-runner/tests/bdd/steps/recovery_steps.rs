//! BDD step definitions for the resume contract (design § Re-entrancy
//! Contract): a session interrupted mid-run — the engine killed, `rp`
//! itself gone, or a safety monitor turning unsafe — continues from the
//! persisted blackboard when re-invoked with recovery context, without
//! repeating recorded work.
//!
//! The safety scenario exercises `rp`'s own recovery re-invocation
//! end-to-end (unsafe terminates the run, safe re-invokes). The other
//! two interrupt the session in ways `rp` cannot recover from yet — an
//! engine kill and an `rp` restart both need `rp`-side startup recovery,
//! which is designed but not implemented — so their steps POST `/invoke`
//! directly, standing in for it: same ids, same forwarded `config`, a
//! non-null `recovery` object.

use std::time::Duration;

use cucumber::{given, then, when};

use bdd_infra::rp_harness::{OmniSimHandle, SafetyMonitorConfig};

use crate::steps::infrastructure::{
    ensure_omnisim, start_rp_service, start_session_runner_service,
};
use crate::steps::trigger_steps::settled_event_count;
use crate::world::SessionRunnerWorld;

/// How long a poll-until-observed step waits before failing the scenario.
const OBSERVATION_BUDGET: Duration = Duration::from_secs(30);

#[when(expr = "the blackboard records at least {int} frames")]
async fn blackboard_records_frames(world: &mut SessionRunnerWorld, frames: u64) {
    let deadline = std::time::Instant::now() + OBSERVATION_BUDGET;
    while std::time::Instant::now() < deadline {
        if world.blackboard_frames().await.is_some_and(|f| f >= frames) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "the blackboard never recorded {frames} frames within {OBSERVATION_BUDGET:?} \
         (last: {:?})",
        world.blackboard_frames().await
    );
}

#[given("a safety monitor guards the session")]
async fn safety_monitor_guards_session(world: &mut SessionRunnerWorld) {
    ensure_omnisim(world).await;
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
    // Fast polling so rp detects the flips in test time, not the
    // production default (10 s).
    world.safety_poll_interval = Some(Duration::from_millis(250));
}

#[when("the safety monitor reports unsafe")]
async fn safety_monitor_reports_unsafe(_world: &mut SessionRunnerWorld) {
    OmniSimHandle::set_safety_monitor_is_safe(false)
        .await
        .expect("failed to flip OmniSim's safety monitor to unsafe");
}

#[when("the safety monitor reports safe again")]
async fn safety_monitor_reports_safe(_world: &mut SessionRunnerWorld) {
    OmniSimHandle::set_safety_monitor_is_safe(true)
        .await
        .expect("failed to flip OmniSim's safety monitor to safe");
}

#[then(expr = "rp reports the session as {string} within {int} seconds")]
async fn rp_reports_session_status(world: &mut SessionRunnerWorld, expected: String, seconds: u64) {
    let client = reqwest::Client::new();
    let url = format!("{}/api/session/status", world.rp_url());
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    let mut last = None;
    while std::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                last = body
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                if last.as_deref() == Some(expected.as_str()) {
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("rp never reported the session as '{expected}' within {seconds}s (last: {last:?})");
}

#[then("the blackboard is kept")]
async fn blackboard_is_kept(world: &mut SessionRunnerWorld) {
    assert!(
        world.blackboard_path().exists(),
        "an interrupted session must keep its blackboard for the recovery invocation"
    );
}

#[when("the session-runner is killed")]
async fn session_runner_is_killed(world: &mut SessionRunnerWorld) {
    world
        .session_runner
        .as_mut()
        .expect("session-runner not started")
        .kill()
        .await;
    world.session_runner = None;
}

#[when("the session-runner is restarted")]
async fn session_runner_is_restarted(world: &mut SessionRunnerWorld) {
    assert!(
        world.session_runner.is_none(),
        "restart follows a kill — the previous instance is still recorded as running"
    );
    // Reuses the scenario's state_dir, so the new process finds the old
    // one's blackboard (and its workflows_dir, so the document resolves).
    start_session_runner_service(world).await;
}

#[when("rp is killed")]
async fn rp_is_killed(world: &mut SessionRunnerWorld) {
    world.rp.as_mut().expect("rp not started").kill().await;
    world.rp = None;
    // The SSE client died with rp; drop it so a later "watching rp's event
    // stream" step attaches a fresh one to the restarted instance.
    world.sse_client = None;
}

#[when("rp is restarted")]
async fn rp_is_restarted(world: &mut SessionRunnerWorld) {
    assert!(
        world.rp.is_none(),
        "restart follows a kill — the previous instance is still recorded as running"
    );
    // Same accumulated config (equipment + the orchestrator registration),
    // fresh process on a fresh port. rp's session state is in-memory, so
    // the restarted instance knows nothing of the interrupted session —
    // exactly the outage being simulated.
    start_rp_service(world).await;
}

#[when("the session is re-invoked with recovery context")]
async fn reinvoke_with_recovery(world: &mut SessionRunnerWorld) {
    // The engine is down (or terminated the run), so the persisted frame
    // counter is stable — note it: the resumed run must capture exactly
    // the remaining `plan - frames` exposures.
    let frames = world
        .blackboard_frames()
        .await
        .expect("no persisted frame counter to resume from");
    world.frames_before_resume = Some(frames);

    let handle = world
        .session_runner
        .as_ref()
        .expect("session-runner not started");
    let body = serde_json::json!({
        "workflow_id": world.workflow_id(),
        "session_id": world.session_id(),
        "mcp_server_url": format!("{}/mcp", world.rp_url()),
        "recovery": { "reason": "test-injected interruption" },
        "config": world
            .orchestrator_config
            .clone()
            .expect("no orchestrator registration recorded"),
    });
    let response = reqwest::Client::new()
        .post(format!("{}/invoke", handle.base_url))
        .json(&body)
        .send()
        .await
        .expect("failed to POST /invoke");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "the recovery invocation was not acknowledged: {:?}",
        response.text().await
    );
}

#[then("the session-runner is still healthy and the blackboard is kept")]
async fn runner_healthy_blackboard_kept(world: &mut SessionRunnerWorld) {
    // With rp dead the tool transport is gone, so run progress is
    // physically impossible and termination itself cannot be observed
    // from outside — the scenario proves it downstream, where the
    // resumed run captures exactly the remaining frames. What this step
    // pins is the engine's *reaction* to rp's loss: the process must
    // survive and must not tear down its persisted state. Both
    // invariants are asserted continuously across the window in which
    // the failed tool call lands (within moments of the kill), so a
    // crash or a blackboard deletion is caught rather than slipping
    // between a sleep and a single check.
    let handle = world
        .session_runner
        .as_ref()
        .expect("session-runner not started");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        let response = reqwest::get(format!("{}/health", handle.base_url))
            .await
            .expect("session-runner did not answer /health after rp died");
        assert!(response.status().is_success(), "{}", response.status());
        assert!(
            world.blackboard_path().exists(),
            "a terminated run must keep its blackboard for the recovery invocation"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[then(expr = "the blackboard is deleted within {int} seconds")]
async fn blackboard_deleted_within(world: &mut SessionRunnerWorld, seconds: u64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        if !world.blackboard_path().exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!(
        "the blackboard still exists after {seconds}s — the session did not complete \
         (or the completion was not acknowledged)"
    );
}

#[then(expr = "the SSE stream should show between {int} and {int} {string} events")]
async fn sse_shows_between(
    world: &mut SessionRunnerWorld,
    minimum: usize,
    maximum: usize,
    event_type: String,
) {
    let count = settled_event_count(world, &event_type, minimum).await;
    assert!(
        (minimum..=maximum).contains(&count),
        "expected between {minimum} and {maximum} '{event_type}' events on the SSE \
         stream, saw {count}"
    );
}

#[then(expr = "the SSE stream should show only the remaining {string} events")]
async fn sse_shows_remaining(world: &mut SessionRunnerWorld, event_type: String) {
    let plan = world
        .orchestrator_config
        .as_ref()
        .and_then(|c| c.pointer("/parameters/plan"))
        .and_then(serde_json::Value::as_u64)
        .expect("the registered workflow carries no `plan` parameter");
    let frames = world
        .frames_before_resume
        .expect("no pre-resume frame count — add the re-invoke step first");
    let remaining = plan.checked_sub(frames).unwrap_or_else(|| {
        panic!("the blackboard records more frames ({frames}) than the plan ({plan})")
    });
    let remaining = usize::try_from(remaining).expect("plan fits usize");

    let count = settled_event_count(world, &event_type, remaining).await;
    assert_eq!(
        count, remaining,
        "expected exactly the {remaining} remaining '{event_type}' events \
         ({plan} planned, {frames} already recorded), saw {count}"
    );
}
