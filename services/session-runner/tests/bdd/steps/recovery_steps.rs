//! BDD step definitions for the resume contract (design § Re-entrancy
//! Contract): a session interrupted mid-run — the engine killed, or `rp`
//! itself gone — continues from the persisted blackboard when re-invoked
//! with recovery context, without repeating recorded work.
//!
//! `rp`'s own recovery re-invocation machinery is not implemented yet
//! (`services/rp/src/session.rs` hard-codes `"recovery": null`), so these
//! steps POST `/invoke` directly, standing in for it — same ids, same
//! forwarded `config`, a non-null `recovery` object.

use std::time::Duration;

use cucumber::{then, when};

use crate::steps::infrastructure::{start_rp_service, start_session_runner_service};
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
    // Give the engine a moment to hit the request-level MCP failure and
    // terminate the run (never retried, so this is quick).
    tokio::time::sleep(Duration::from_secs(2)).await;

    let handle = world
        .session_runner
        .as_ref()
        .expect("session-runner not started");
    let response = reqwest::get(format!("{}/health", handle.base_url))
        .await
        .expect("session-runner did not answer /health after rp died");
    assert!(response.status().is_success(), "{}", response.status());

    assert!(
        world.blackboard_path().exists(),
        "a terminated run must keep its blackboard for the recovery invocation"
    );
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
