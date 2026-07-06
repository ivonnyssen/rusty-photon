//! BDD step definitions for the calibrator-flats workflow-document port.
//!
//! The scenarios spawn three processes: OmniSim (Alpaca simulator), rp
//! (equipment gateway + session orchestrator), and session-runner (the
//! generic document engine under test). The process topology lives in
//! [`crate::steps::infrastructure`]; this file holds only the Gherkin
//! step wiring and the flats-specific registration parameters.

use std::time::Duration;

use cucumber::{given, then, when};

use crate::steps::infrastructure::{
    add_event_plugin, configure_default_equipment, ensure_omnisim, ensure_webhook_receiver,
    register_orchestrator, start_rp_service, start_session_runner_service,
};
use crate::world::SessionRunnerWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given("a running Alpaca simulator")]
async fn running_alpaca_simulator(world: &mut SessionRunnerWorld) {
    ensure_omnisim(world).await;
}

#[given(expr = "a flat plan of {int} {string} flats and {int} {string} flats")]
async fn flat_plan(
    world: &mut SessionRunnerWorld,
    count1: u32,
    filter1: String,
    count2: u32,
    filter2: String,
) {
    world.flat_plan = vec![(filter1, count1), (filter2, count2)];
}

#[given(expr = "a test webhook receiver subscribed to {string}")]
async fn webhook_receiver_subscribed_to(world: &mut SessionRunnerWorld, event_type: String) {
    ensure_webhook_receiver(world).await;
    add_event_plugin(world, vec![event_type]);
}

#[given("rp is running with a camera, filter wheel, cover calibrator, and the session-runner orchestrator")]
async fn rp_running_with_equipment_and_session_runner(world: &mut SessionRunnerWorld) {
    configure_default_equipment(world).await;
    start_session_runner_service(world).await;
    register_calibrator_flats(world);
    start_rp_service(world).await;
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when("a session is started via the REST API")]
async fn start_session(world: &mut SessionRunnerWorld) {
    let client = reqwest::Client::new();
    let url = format!("{}/api/session/start", world.rp_url());

    let resp = client
        .post(&url)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("failed to POST /api/session/start");

    world.last_api_status = Some(resp.status().as_u16());
    world.last_api_body = resp.json().await.ok();
}

#[when("the workflow document runs to completion")]
async fn workflow_runs_to_completion(world: &mut SessionRunnerWorld) {
    // Full workflow: close cover (~5s in OmniSim), calibrator on (~2s),
    // per-filter exposure search, batch captures, calibrator off (~2s),
    // open cover (~5s). Allow 120s total, matching the Rust
    // calibrator-flats suite this port must stay equivalent to.
    assert!(
        world.wait_for_session_idle(Duration::from_secs(120)).await,
        "the calibrator flats document did not complete within 120s \
         (expected the session to return to idle)"
    );
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(expr = "the session status should be {string}")]
async fn session_status_is(world: &mut SessionRunnerWorld, expected: String) {
    let client = reqwest::Client::new();
    let url = format!("{}/api/session/status", world.rp_url());

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("failed to GET /api/session/status");

    let body: serde_json::Value = resp.json().await.expect("failed to parse session status");
    let actual = body
        .get("status")
        .and_then(|v| v.as_str())
        .expect("status field missing");

    assert_eq!(
        actual, expected,
        "expected session status '{}' but got '{}'",
        expected, actual
    );
}

#[then(expr = "the test webhook receiver should have received at least {int} {string} event(s)")]
async fn should_receive_at_least_n_events(
    world: &mut SessionRunnerWorld,
    count: usize,
    event_type: String,
) {
    assert!(
        world.wait_for_events(&event_type, count).await,
        "expected at least {} '{}' event(s) within timeout",
        count,
        event_type
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Register the shipped calibrator_flats document as the orchestrator's
/// workflow. Tolerance `1.0` and `max_iterations = 1` mirror the Rust
/// calibrator-flats suite: these scenarios verify end-to-end plumbing,
/// not convergence math (the engine's unit tests own that).
fn register_calibrator_flats(world: &mut SessionRunnerWorld) {
    let filters: Vec<serde_json::Value> = world
        .flat_plan
        .iter()
        .map(|(name, count)| serde_json::json!({ "name": name, "count": count }))
        .collect();
    let parameters = serde_json::json!({
        "camera_id": "main-cam",
        "filter_wheel_id": "main-fw",
        "calibrator_id": "flat-panel",
        "target_adu_fraction": 0.5,
        "tolerance": 1.0,
        "max_iterations": 1,
        "initial_duration": "100ms",
        "filters": filters
    });
    register_orchestrator(world, "calibrator_flats", Some(parameters));
}
