//! BDD step definitions for the end-to-end calibrator-flats orchestrator
//! workflow.
//!
//! The scenarios spawn three processes: OmniSim (Alpaca simulator), rp
//! (equipment gateway + session orchestrator), and calibrator-flats (the
//! orchestrator plugin being tested). All three are coordinated via
//! `bdd_infra::rp_harness` helpers; this file holds only the Gherkin step
//! wiring and the calibrator-flats-specific config builder.

use std::time::Duration;

use bdd_infra::rp_harness::{
    build_calibrator_flats_config, sibling_service_dir, start_rp, write_temp_config_file,
    OmniSimHandle, WebhookReceiver,
};
use bdd_infra::ServiceHandle;
use cucumber::{given, then, when};

use crate::world::CalibratorFlatsWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given("a running Alpaca simulator")]
async fn running_alpaca_simulator(world: &mut CalibratorFlatsWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
}

#[given(
    expr = "the calibrator-flats service is configured for {int} {string} flats and {int} {string} flats"
)]
async fn configure_calibrator_flats(
    world: &mut CalibratorFlatsWorld,
    count1: i32,
    filter1: String,
    count2: i32,
    filter2: String,
) {
    world.flat_plan = vec![(filter1, count1 as u32), (filter2, count2 as u32)];
}

#[given(expr = "a test webhook receiver subscribed to {string}")]
async fn webhook_receiver_subscribed_to(world: &mut CalibratorFlatsWorld, event_type: String) {
    ensure_webhook_receiver(world).await;
    add_event_plugin(world, vec![event_type]);
}

#[given(
    "rp is running with a camera, filter wheel, cover calibrator, and the calibrator-flats orchestrator"
)]
async fn rp_running_with_equipment_and_calibrator_flats(world: &mut CalibratorFlatsWorld) {
    configure_default_equipment(world).await;
    start_calibrator_flats_service(world).await;
    register_calibrator_flats_plugin(world);
    start_rp_service(world).await;
}

#[given(
    "rp is running with a camera, filter wheel, cover calibrator, webhook, and the calibrator-flats orchestrator"
)]
async fn rp_running_with_equipment_webhook_and_calibrator_flats(world: &mut CalibratorFlatsWorld) {
    configure_default_equipment(world).await;
    start_calibrator_flats_service(world).await;
    register_calibrator_flats_plugin(world);
    start_rp_service(world).await;
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when("a session is started via the REST API")]
async fn start_session(world: &mut CalibratorFlatsWorld) {
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

#[when("the calibrator-flats orchestrator runs to completion")]
async fn orchestrator_runs_to_completion(world: &mut CalibratorFlatsWorld) {
    // Full workflow: close cover (~5s in OmniSim), calibrator on (~2s),
    // per-filter iterative exposure search (up to 5 iterations), batch
    // captures, calibrator off (~2s), open cover (~5s). Allow 120s total.
    let client = reqwest::Client::new();
    let url = format!("{}/api/session/status", world.rp_url());
    for _ in 0..480 {
        tokio::time::sleep(Duration::from_millis(250)).await;
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

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(expr = "the session status should be {string}")]
async fn session_status_is(world: &mut CalibratorFlatsWorld, expected: String) {
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

#[then(expr = "the test webhook receiver should have received at least {int} {string} event")]
async fn should_receive_at_least_n_events(
    world: &mut CalibratorFlatsWorld,
    count: i32,
    event_type: String,
) {
    assert!(
        world.wait_for_events(&event_type, count as usize).await,
        "expected at least {} '{}' event(s) within timeout",
        count,
        event_type
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn configure_default_equipment(world: &mut CalibratorFlatsWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
    let alpaca_url = world.omnisim_url();

    if world.cameras.is_empty() {
        world.cameras.push(bdd_infra::rp_harness::CameraConfig {
            id: "main-cam".to_string(),
            alpaca_url: alpaca_url.clone(),
            device_number: 0,
        });
    }
    if world.filter_wheels.is_empty() {
        world
            .filter_wheels
            .push(bdd_infra::rp_harness::FilterWheelConfig {
                id: "main-fw".to_string(),
                alpaca_url: alpaca_url.clone(),
                device_number: 0,
                filters: vec![
                    "Luminance".to_string(),
                    "Red".to_string(),
                    "Green".to_string(),
                    "Blue".to_string(),
                ],
            });
    }
    if world.cover_calibrators.is_empty() {
        world
            .cover_calibrators
            .push(bdd_infra::rp_harness::CoverCalibratorConfig {
                id: "flat-panel".to_string(),
                alpaca_url,
                device_number: 0,
            });
    }
}

async fn start_calibrator_flats_service(world: &mut CalibratorFlatsWorld) {
    if world.calibrator_flats.is_some() {
        return;
    }

    let config = build_calibrator_flats_config(&world.flat_plan);
    let config_path = write_temp_config_file("calibrator-flats-config", &config).await;

    world.calibrator_flats = Some(
        ServiceHandle::start(
            env!("CARGO_MANIFEST_DIR"),
            env!("CARGO_PKG_NAME"),
            &config_path,
        )
        .await,
    );
}

fn register_calibrator_flats_plugin(world: &mut CalibratorFlatsWorld) {
    let handle = world
        .calibrator_flats
        .as_ref()
        .expect("calibrator-flats not started");
    let invoke_url = format!("{}/invoke", handle.base_url);

    world.plugin_configs.push(serde_json::json!({
        "name": "calibrator-flats",
        "type": "orchestrator",
        "invoke_url": invoke_url,
        "requires_tools": []
    }));
}

async fn start_rp_service(world: &mut CalibratorFlatsWorld) {
    if world.rp.as_ref().is_some_and(|h| h.is_running()) {
        return;
    }

    let config = world.build_rp_config();
    let rp_manifest_dir = sibling_service_dir(env!("CARGO_MANIFEST_DIR"), "rp");
    let rp_manifest_dir_str = rp_manifest_dir.to_str().expect("rp manifest path is utf-8");
    world.rp = Some(start_rp(rp_manifest_dir_str, &config).await);

    assert!(
        world.wait_for_rp_healthy().await,
        "rp did not become healthy within timeout"
    );
}

async fn ensure_webhook_receiver(world: &mut CalibratorFlatsWorld) {
    if world.webhook_receiver.is_some() {
        return;
    }
    let (estimated, max) = world.webhook_ack_config.unwrap_or((5, 10));
    let events = world.received_events.clone();
    world.webhook_receiver = Some(WebhookReceiver::start(events, estimated, max).await);
}

fn add_event_plugin(world: &mut CalibratorFlatsWorld, events: Vec<String>) {
    let url = world
        .webhook_receiver
        .as_ref()
        .expect("webhook receiver not started")
        .url
        .clone();

    let already_exists = world
        .plugin_configs
        .iter()
        .any(|p| p.get("name").and_then(|v| v.as_str()) == Some("test-event-plugin"));

    if already_exists {
        if let Some(config) = world
            .plugin_configs
            .iter_mut()
            .find(|p| p.get("name").and_then(|v| v.as_str()) == Some("test-event-plugin"))
        {
            let existing = config
                .get("subscribes_to")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let mut merged = existing;
            for e in events {
                if !merged.contains(&e) {
                    merged.push(e);
                }
            }
            config["subscribes_to"] = serde_json::json!(merged);
        }
    } else {
        world.plugin_configs.push(serde_json::json!({
            "name": "test-event-plugin",
            "type": "event",
            "webhook_url": url,
            "subscribes_to": events
        }));
    }
}
