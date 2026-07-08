//! BDD step definitions for the shipped deep-sky workflow document
//! (design § `deep_sky.json`): the dispatch loop against rp's real
//! planner, target switching on visibility, the refocus and
//! meridian-flip trigger overlay, and safety resume with
//! re-acquisition.
//!
//! The planner evaluates real ephemeris at wall-clock now, so these
//! steps compute the observing site to fit the clock
//! (`bdd_infra::rp_harness::NightSky`): an equatorial site at the
//! anti-solar longitude is always in deep astronomical night, and
//! celestial-equator targets placed by hour angle sink at a constant
//! ≈ 0.25°/minute — which makes "the first target drops below its
//! floor N seconds from now" exact. The simulated mount is taught the
//! same site (rp hard-errors on mount connect when the mount's
//! reported site disagrees with config) and synced onto the first
//! target so every document slew stays sub-degree (OmniSim slews at
//! real-mount speed).

use std::time::Duration;

use cucumber::gherkin::Step;
use cucumber::{given, then, when};

use bdd_infra::rp_harness::{
    CameraConfig, CannedWcs, ExposurePlanConfig, FocuserConfig, MountConfig, NightSky,
    OmniSimHandle, PlannerTargetConfig, PlateSolverConfig, PlateSolverStub, StubBehavior,
};

use crate::steps::infrastructure::{
    ensure_omnisim, register_orchestrator, start_rp_service, start_session_runner_service,
};
use crate::world::SessionRunnerWorld;

/// How long a poll-until-observed step waits before failing the scenario.
const OBSERVATION_BUDGET: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Given steps: the computed night sky
// ---------------------------------------------------------------------------

#[given("an observing site where it is astronomical night with one planner target")]
async fn night_sky_with_one_target(world: &mut SessionRunnerWorld) {
    push_one_night_target(world, Vec::new());
}

#[given(
    expr = "an observing site where it is astronomical night with one planner target whose \
            exposure plan is a single unfiltered {int}-second frame"
)]
async fn night_sky_with_one_planned_target(world: &mut SessionRunnerWorld, seconds: u64) {
    push_one_night_target(
        world,
        vec![ExposurePlanConfig {
            filter: None,
            duration_secs: seconds as f64,
        }],
    );
}

fn push_one_night_target(world: &mut SessionRunnerWorld, exposures: Vec<ExposurePlanConfig>) {
    let sky = NightSky::at(chrono::Utc::now());
    // Half an hour past transit: ≈ 82.5° altitude and sinking, next
    // meridian crossing ≈ 23.5 sidereal hours away — no flip pressure,
    // viable for hours against the planner-wide 20° floor.
    let target = sky.target_at_hour_angle(0.5);
    world.site = Some((sky.latitude_degrees(), sky.longitude_degrees()));
    world.planner_targets.push(PlannerTargetConfig {
        name: "primary".to_string(),
        ra_hours: target.ra_hours,
        dec_degrees: target.dec_degrees,
        min_altitude_degrees: None,
        exposures,
    });
    world.night_targets.push(target);
}

#[given(
    expr = "an observing site where it is astronomical night and the first of two planner \
            targets sinks below its floor after {int} seconds"
)]
async fn night_sky_with_sinking_target(world: &mut SessionRunnerWorld, seconds: i64) {
    let sky = NightSky::at(chrono::Utc::now());
    // Both targets descend in the west near 45° altitude, 0.05 h of
    // hour angle (0.75° of sky) apart so the switch slew is quick. The
    // sinker is closer to transit, so the planner prefers it while it
    // stays viable; its floor is set to the altitude it will have
    // `seconds` from now, which is when the planner drops it and the
    // backup (on the planner-wide 20° floor, ≈ 90 minutes of margin)
    // takes over.
    let sinker = sky.target_at_hour_angle(3.0);
    let backup = sky.target_at_hour_angle(3.05);
    let floor = sky.altitude_degrees_in(sinker, seconds);
    world.site = Some((sky.latitude_degrees(), sky.longitude_degrees()));
    world.planner_targets.push(PlannerTargetConfig {
        name: "sinker".to_string(),
        ra_hours: sinker.ra_hours,
        dec_degrees: sinker.dec_degrees,
        min_altitude_degrees: Some(floor),
        exposures: Vec::new(),
    });
    world.planner_targets.push(PlannerTargetConfig {
        name: "backup".to_string(),
        ra_hours: backup.ra_hours,
        dec_degrees: backup.dec_degrees,
        min_altitude_degrees: None,
        exposures: Vec::new(),
    });
    world.night_targets.push(sinker);
    world.night_targets.push(backup);
}

#[given("the simulated mount matches the site and points at the first target")]
async fn mount_matches_site_and_target(world: &mut SessionRunnerWorld) {
    ensure_omnisim(world).await;
    let (lat, lon) = world
        .site
        .expect("compute the observing site before configuring the mount");
    let first = *world
        .night_targets
        .first()
        .expect("compute the planner targets before configuring the mount");
    // Capture the site before overwriting it — it is a profile setting
    // the per-scenario restart does not reset, so the after-hook puts
    // it back for whatever suite reuses this OmniSim next.
    world.original_telescope_site = Some(
        OmniSimHandle::get_telescope_site()
            .await
            .expect("failed to read the simulated mount's site"),
    );
    OmniSimHandle::set_telescope_site(lat, lon)
        .await
        .expect("failed to set the simulated mount's site");
    // OmniSim requires tracking for SyncToCoordinates; the sync
    // teleports the mount's coordinate frame onto the target without
    // physical motion, so the document's acquisition slew is
    // effectively zero-distance.
    OmniSimHandle::set_telescope_tracking(true)
        .await
        .expect("failed to enable the simulated mount's tracking");
    OmniSimHandle::sync_telescope_to(first.ra_hours, first.dec_degrees)
        .await
        .expect("failed to sync the simulated mount onto the first target");
}

#[given("a stub plate solver echoing the first target")]
async fn stub_plate_solver_echoing_first_target(world: &mut SessionRunnerWorld) {
    let first = *world
        .night_targets
        .first()
        .expect("compute the planner targets before starting the plate-solver stub");
    // Solved center == target center (WCS is degrees on the wire), so
    // center_on_target converges on its first iteration.
    let stub = PlateSolverStub::start(StubBehavior::Canned(CannedWcs {
        ra_center: first.ra_hours * 15.0,
        dec_center: first.dec_degrees,
        pixel_scale_arcsec: 1.05,
        rotation_deg: 0.0,
        solver: "stub-astap-1.0".to_string(),
    }))
    .await;
    world.plate_solver = Some(PlateSolverConfig {
        url: stub.url.clone(),
        timeout: None,
        default_search_radius_deg: None,
    });
    world.plate_solver_stub = Some(stub);
}

// ---------------------------------------------------------------------------
// Given steps: process topology
// ---------------------------------------------------------------------------

#[given(
    expr = "rp is running with a camera, a mount, and the session-runner orchestrator running \
            the {string} workflow with parameters:"
)]
async fn rp_with_camera_mount_and_workflow(
    world: &mut SessionRunnerWorld,
    workflow: String,
    step: &Step,
) {
    configure_deep_sky_equipment(world, false).await;
    start_session_runner_service(world).await;
    register_deep_sky(world, &workflow, step, false);
    start_rp_service(world).await;
}

#[given(
    expr = "rp is running with a camera, a mount, a focuser, and the session-runner \
            orchestrator running the {string} workflow with parameters:"
)]
async fn rp_with_camera_mount_focuser_and_workflow(
    world: &mut SessionRunnerWorld,
    workflow: String,
    step: &Step,
) {
    configure_deep_sky_equipment(world, true).await;
    start_session_runner_service(world).await;
    register_deep_sky(world, &workflow, step, true);
    start_rp_service(world).await;
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(expr = "the deep-sky session has captured at least {int} frames")]
async fn deep_sky_captured_frames(world: &mut SessionRunnerWorld, frames: u64) {
    let deadline = std::time::Instant::now() + OBSERVATION_BUDGET;
    while std::time::Instant::now() < deadline {
        if world
            .blackboard_counter("total_frames")
            .await
            .is_some_and(|f| f >= frames)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "the blackboard never recorded {frames} total_frames within {OBSERVATION_BUDGET:?} \
         (last: {:?})",
        world.blackboard_counter("total_frames").await
    );
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(expr = "a second {string} event should be observed within {int} seconds")]
async fn second_event_observed_within(
    world: &mut SessionRunnerWorld,
    event_type: String,
    seconds: u64,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        let count = sse_event_seqs(world, &event_type).await.len();
        if count >= 2 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected a second '{event_type}' event within {seconds}s, saw {count}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[then(expr = "at least {int} {string} event(s) should precede the second {string} on the stream")]
async fn events_precede_second(
    world: &mut SessionRunnerWorld,
    minimum: usize,
    needle: String,
    boundary: String,
) {
    let boundary_seq = second_seq(world, &boundary).await;
    let count = sse_event_seqs(world, &needle)
        .await
        .into_iter()
        .filter(|seq| *seq < boundary_seq)
        .count();
    assert!(
        count >= minimum,
        "expected at least {minimum} '{needle}' event(s) before the second '{boundary}' \
         (seq {boundary_seq}), saw {count}"
    );
}

#[then(expr = "at least {int} {string} event(s) should follow the second {string} on the stream")]
async fn events_follow_second(
    world: &mut SessionRunnerWorld,
    minimum: usize,
    needle: String,
    boundary: String,
) {
    let boundary_seq = second_seq(world, &boundary).await;
    let deadline = std::time::Instant::now() + OBSERVATION_BUDGET;
    loop {
        let count = sse_event_seqs(world, &needle)
            .await
            .into_iter()
            .filter(|seq| *seq > boundary_seq)
            .count();
        if count >= minimum {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected at least {minimum} '{needle}' event(s) after the second '{boundary}' \
             (seq {boundary_seq}) within {OBSERVATION_BUDGET:?}, saw {count}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[then(expr = "the SSE stream should show at least {int} {string} events")]
async fn sse_shows_at_least(world: &mut SessionRunnerWorld, minimum: usize, event_type: String) {
    let count = crate::steps::trigger_steps::settled_event_count(world, &event_type, minimum).await;
    assert!(
        count >= minimum,
        "expected at least {minimum} '{event_type}' events on the SSE stream, saw {count}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The deep-sky equipment set: one camera and the singular mount, plus
/// a focuser for the refocus scenarios. No filter wheel — the
/// scenarios leave the document's `filter` parameter at its empty
/// default and give their planner targets unfiltered exposure plans
/// (if any), so `set_filter` never runs.
async fn configure_deep_sky_equipment(world: &mut SessionRunnerWorld, with_focuser: bool) {
    ensure_omnisim(world).await;
    let alpaca_url = world.omnisim_url();

    if world.cameras.is_empty() {
        world.cameras.push(CameraConfig {
            id: "main-cam".to_string(),
            alpaca_url: alpaca_url.clone(),
            device_number: 0,
        });
    }
    if world.mount.is_none() {
        world.mount = Some(MountConfig {
            alpaca_url: alpaca_url.clone(),
            device_number: 0,
            settle_after_slew: None,
        });
    }
    if with_focuser && world.focusers.is_empty() {
        world.focusers.push(FocuserConfig {
            id: "main-focuser".to_string(),
            alpaca_url,
            device_number: 0,
            min_position: None,
            max_position: None,
        });
    }
}

/// Register the shipped deep_sky document as the orchestrator's
/// workflow, with the scenario table's parameters on top of the fixed
/// device ids. Table rows are `| name | value |` (no header); values
/// are coerced in order: boolean, integer, number, else string (so
/// humantime durations like `500ms` stay strings).
fn register_deep_sky(
    world: &mut SessionRunnerWorld,
    workflow: &str,
    step: &Step,
    with_focuser: bool,
) {
    let mut parameters = serde_json::json!({ "camera_id": "main-cam" });
    if with_focuser {
        parameters["focuser_id"] = serde_json::json!("main-focuser");
    }
    let table = step
        .table
        .as_ref()
        .expect("step requires a `| name | value |` parameters table");
    for row in &table.rows {
        assert_eq!(
            row.len(),
            2,
            "parameter row must be `| name | value |`: {row:?}"
        );
        let (name, value) = (&row[0], &row[1]);
        parameters[name] = coerce_parameter(value);
    }
    register_orchestrator(world, workflow, Some(parameters));
}

fn coerce_parameter(value: &str) -> serde_json::Value {
    if let Ok(b) = value.parse::<bool>() {
        return serde_json::json!(b);
    }
    if let Ok(i) = value.parse::<i64>() {
        return serde_json::json!(i);
    }
    if let Ok(f) = value.parse::<f64>() {
        // Rust's f64 parser accepts "NaN"/"inf", which JSON cannot
        // represent (`json!` would map them to null — a silently wrong
        // parameter). Only finite values are numbers; anything else
        // falls through as the literal string, which the engine's
        // parameter type-check then rejects loudly.
        if f.is_finite() {
            return serde_json::json!(f);
        }
    }
    serde_json::json!(value)
}

/// Stream-sequence ids of every SSE frame of the given event type, in
/// arrival order.
async fn sse_event_seqs(world: &SessionRunnerWorld, event_type: &str) -> Vec<u64> {
    let client = world
        .sse_client
        .as_ref()
        .expect("no SSE client — add the 'an SSE client is watching' step");
    client
        .frames()
        .await
        .iter()
        .filter(|f| f.event_type().as_deref() == Some(event_type))
        .map(|f| {
            f.id.unwrap_or_else(|| panic!("a '{event_type}' frame carries no stream sequence id"))
        })
        .collect()
}

/// The stream-sequence id of the second event of the given type —
/// asserted present (add the "a second … should be observed" step
/// first when the second occurrence needs waiting for).
async fn second_seq(world: &SessionRunnerWorld, event_type: &str) -> u64 {
    let seqs = sse_event_seqs(world, event_type).await;
    assert!(
        seqs.len() >= 2,
        "expected at least two '{event_type}' events on the stream, saw {}",
        seqs.len()
    );
    seqs[1]
}
