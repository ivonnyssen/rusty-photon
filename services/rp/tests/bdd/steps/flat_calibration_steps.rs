//! BDD step definitions for end-to-end calibrator-flats orchestrator tests.
//!
//! These steps start the real calibrator-flats service as a child process
//! alongside rp and OmniSim, then drive the session via rp's REST API.

use cucumber::{given, when};

use crate::steps::cover_calibrator_steps::add_cover_calibrator;
use crate::steps::infrastructure::ServiceHandle;
use crate::steps::tool_steps::{add_camera, add_filter_wheel, ensure_omnisim, start_rp};
use crate::world::RpWorld;

// --- Given steps ---

#[given(
    expr = "the calibrator-flats service is configured for {int} {string} flats and {int} {string} flats"
)]
async fn configure_calibrator_flats(
    world: &mut RpWorld,
    count1: i32,
    filter1: String,
    count2: i32,
    filter2: String,
) {
    let plan = vec![(filter1, count1 as u32), (filter2, count2 as u32)];
    world.flat_plan = plan;
}

#[given(
    "rp is running with a camera, filter wheel, cover calibrator, and the calibrator-flats orchestrator"
)]
async fn rp_running_with_equipment_and_calibrator_flats(world: &mut RpWorld) {
    ensure_omnisim(world).await;
    add_camera(world);
    add_filter_wheel(world);
    add_cover_calibrator(world);

    // Start calibrator-flats service
    start_calibrator_flats(world).await;

    // Register it as orchestrator plugin in rp config
    let invoke_url = world
        .calibrator_flats
        .as_ref()
        .expect("calibrator-flats not started")
        .invoke_url
        .clone();

    world.plugin_configs.push(serde_json::json!({
        "name": "calibrator-flats",
        "type": "orchestrator",
        "invoke_url": invoke_url,
        "requires_tools": []
    }));

    start_rp(world).await;
}

#[given(
    "rp is running with a camera, filter wheel, cover calibrator, webhook, and the calibrator-flats orchestrator"
)]
async fn rp_running_with_equipment_webhook_and_calibrator_flats(world: &mut RpWorld) {
    ensure_omnisim(world).await;
    add_camera(world);
    add_filter_wheel(world);
    add_cover_calibrator(world);

    // Start calibrator-flats service
    start_calibrator_flats(world).await;

    // Register it as orchestrator plugin in rp config
    let invoke_url = world
        .calibrator_flats
        .as_ref()
        .expect("calibrator-flats not started")
        .invoke_url
        .clone();

    world.plugin_configs.push(serde_json::json!({
        "name": "calibrator-flats",
        "type": "orchestrator",
        "invoke_url": invoke_url,
        "requires_tools": []
    }));

    start_rp(world).await;
}

// --- When steps ---

#[when("the calibrator-flats orchestrator runs to completion")]
async fn calibrator_flats_runs_to_completion(world: &mut RpWorld) {
    // The full workflow includes: close cover (~5s in OmniSim), calibrator on (~2s),
    // per-filter iterative exposure search (up to 5 iterations), batch captures,
    // calibrator off (~2s), open cover (~5s). Allow 120s total.
    let client = reqwest::Client::new();
    let url = format!("{}/api/session/status", world.rp_url());
    for _ in 0..480 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                if body.get("status").and_then(|v| v.as_str()) == Some("idle") {
                    return;
                }
            }
        }
    }
    panic!(
        "calibrator-flats orchestrator did not complete within 120s \
         (expected session to return to idle)"
    );
}

// --- Helpers ---

/// Info about a running calibrator-flats service
#[derive(Debug)]
pub struct CalibratorFlatsHandle {
    pub handle: ServiceHandle,
    pub invoke_url: String,
}

async fn start_calibrator_flats(world: &mut RpWorld) {
    if world.calibrator_flats.is_some() {
        return;
    }

    // Build the calibrator-flats config from the flat plan
    let config = build_calibrator_flats_config(world);
    let config_path = std::env::temp_dir()
        .join(format!(
            "calibrator-flats-config-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .to_string();
    tokio::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap())
        .await
        .expect("failed to write calibrator-flats config");

    // Resolve calibrator-flats manifest dir relative to rp's
    let calibrator_flats_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("calibrator-flats");

    let handle = ServiceHandle::start(
        calibrator_flats_dir.to_str().unwrap(),
        "calibrator-flats",
        &config_path,
    )
    .await;

    let invoke_url = format!("{}/invoke", handle.base_url);

    world.calibrator_flats = Some(CalibratorFlatsHandle { handle, invoke_url });
}

fn build_calibrator_flats_config(world: &RpWorld) -> serde_json::Value {
    let filters: Vec<serde_json::Value> = world
        .flat_plan
        .iter()
        .map(|(name, count)| {
            serde_json::json!({
                "name": name,
                "count": count
            })
        })
        .collect();

    // OmniSim's camera simulator produces low signal levels (~11 ADU at
    // 100ms). Use tolerance=1.0 and max_iterations=1 so the test verifies
    // end-to-end plumbing (3-process coordination, cover lifecycle, session
    // lifecycle) rather than convergence math (covered by unit tests).
    serde_json::json!({
        "camera_id": "main-cam",
        "filter_wheel_id": "main-fw",
        "calibrator_id": "flat-panel",
        "target_adu_fraction": 0.5,
        "tolerance": 1.0,
        "max_iterations": 1,
        "initial_duration_ms": 100,
        "filters": filters
    })
}
