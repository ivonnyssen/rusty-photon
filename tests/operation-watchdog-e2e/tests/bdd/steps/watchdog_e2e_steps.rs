//! Steps for the operation-watchdog end-to-end suite.

use cucumber::{given, then, when};

use crate::world::{target_canned_wcs, PlateSolverMode, WatchdogE2eWorld};
use bdd_infra::rp_harness::{PlateSolverStub, StubBehavior};

/// Sentinel names its operation watchdog this in dashboard history records.
const WATCHDOG_NAME: &str = "Operation Watchdog";

// --- Given: plate-solver behavior (selected before rp starts) ---

#[given("rp's plate solver hangs so a centering operation never completes")]
async fn plate_solver_hangs(world: &mut WatchdogE2eWorld) {
    let stub = PlateSolverStub::start(StubBehavior::Hang).await;
    world.plate_solver_stub = Some(stub);
    world.plate_solver_mode = PlateSolverMode::Hang;
}

#[given("rp's plate solver returns the target field center immediately")]
async fn plate_solver_returns_target(world: &mut WatchdogE2eWorld) {
    let stub = PlateSolverStub::start(target_canned_wcs()).await;
    world.plate_solver_stub = Some(stub);
    world.plate_solver_mode = PlateSolverMode::Canned;
}

// --- Given: bring the stack up ---

#[given("a running rp and sentinel with the operation watchdog enabled")]
async fn running_stack(world: &mut WatchdogE2eWorld) {
    world.start_stack().await;
}

// --- When ---

#[when("the operator starts centering on a target")]
async fn start_wedged_centering(world: &mut WatchdogE2eWorld) {
    world.spawn_wedge_centering().await;
}

#[when("the operator centers on the target and it converges")]
async fn centering_converges(world: &mut WatchdogE2eWorld) {
    world.run_centering_to_completion().await;
}

#[when("rp stops responding")]
async fn rp_stops(world: &mut WatchdogE2eWorld) {
    world.stop_rp().await;
}

// --- Then ---

#[then("the watchdog escalates the centering operation")]
async fn watchdog_escalates_centering(world: &mut WatchdogE2eWorld) {
    let history = world
        .wait_for_history(|r| {
            r["monitor_name"].as_str() == Some(WATCHDOG_NAME)
                && r["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("centering"))
        })
        .await;
    assert!(
        history.iter().any(|r| {
            r["monitor_name"].as_str() == Some(WATCHDOG_NAME)
                && r["message"].as_str().is_some_and(|m| m.contains("centering"))
        }),
        "expected a '{WATCHDOG_NAME}' escalation mentioning the centering operation; history: {history:#?}"
    );
}

#[then("the corrective ladder runs the restart command")]
async fn ladder_runs_restart(world: &mut WatchdogE2eWorld) {
    let history = world
        .wait_for_history(|r| {
            r["monitor_name"].as_str() == Some(WATCHDOG_NAME)
                && r["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("restart=ran"))
        })
        .await;
    assert!(
        history.iter().any(|r| {
            r["monitor_name"].as_str() == Some(WATCHDOG_NAME)
                && r["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("restart=ran"))
        }),
        "expected the escalation message to report 'restart=ran'; history: {history:#?}"
    );
}

#[then("the restart command leaves its marker file")]
async fn restart_marker_present(world: &mut WatchdogE2eWorld) {
    let marker = world.restart_marker_path();
    for _ in 0..40 {
        if marker.exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    panic!("restart marker {marker:?} was never created — the restart rung did not shell out");
}

#[then("the watchdog records no escalation for the centering operation")]
async fn no_centering_escalation(world: &mut WatchdogE2eWorld) {
    // The convergent call must have succeeded — otherwise "no escalation" would
    // be vacuously true for the wrong reason.
    let result = world
        .centering_result
        .as_ref()
        .expect("centering was never invoked");
    assert!(
        result.is_ok(),
        "centering was expected to converge, but returned an error: {result:?}"
    );

    // Let any erroneous escalation land before asserting none did.
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let history = world.get_history().await;
    let escalations: Vec<_> = history
        .iter()
        .filter(|r| {
            r["monitor_name"].as_str() == Some(WATCHDOG_NAME)
                && r["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("centering"))
        })
        .collect();
    assert!(
        escalations.is_empty(),
        "expected no watchdog escalation for a converging centering op, got: {escalations:#?}"
    );
}

#[then("the watchdog reports rp unresponsive")]
async fn watchdog_reports_unresponsive(world: &mut WatchdogE2eWorld) {
    let history = world
        .wait_for_history(|r| {
            r["monitor_name"].as_str() == Some(WATCHDOG_NAME)
                && r["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("unresponsive"))
        })
        .await;
    assert!(
        history.iter().any(|r| {
            r["monitor_name"].as_str() == Some(WATCHDOG_NAME)
                && r["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("unresponsive"))
        }),
        "expected a '{WATCHDOG_NAME}' escalation reporting rp unresponsive; history: {history:#?}"
    );
}
