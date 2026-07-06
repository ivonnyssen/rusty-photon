//! BDD step definitions for the event-subscription contract: the engine's
//! SSE intake and the `wait.until_event` instruction, exercised through
//! purpose-built fixture documents (`tests/fixtures/workflows/`).

use std::time::Duration;

use cucumber::{given, then};

use crate::steps::infrastructure::{
    configure_default_equipment, register_orchestrator, start_rp_service,
    start_session_runner_service,
};
use crate::world::SessionRunnerWorld;

#[given(
    expr = "rp is running with a camera and the session-runner orchestrator running the {string} workflow"
)]
async fn rp_running_with_fixture_workflow(world: &mut SessionRunnerWorld, workflow: String) {
    configure_default_equipment(world).await;
    start_session_runner_service(world).await;
    let parameters = match workflow.as_str() {
        "wait_for_exposure_event" => Some(serde_json::json!({ "camera_id": "main-cam" })),
        "wait_for_missing_event" => None,
        other => panic!("no registration parameters defined for fixture workflow `{other}`"),
    };
    register_orchestrator(world, &workflow, parameters);
    start_rp_service(world).await;
}

#[then(expr = "the session ends within {int} seconds")]
async fn session_ends_within(world: &mut SessionRunnerWorld, seconds: u64) {
    assert!(
        world
            .wait_for_session_idle(Duration::from_secs(seconds))
            .await,
        "expected the session to return to idle within {seconds}s"
    );
}
