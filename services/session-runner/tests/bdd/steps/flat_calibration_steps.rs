//! BDD step definitions for the calibrator-flats workflow-document port.
//!
//! The scenarios spawn three processes: OmniSim (Alpaca simulator), rp
//! (equipment gateway + session orchestrator), and session-runner (the
//! generic document engine under test). All three are coordinated via
//! `bdd_infra::rp_harness` helpers; this file holds only the Gherkin step
//! wiring and the session-runner-specific config/registration builders.

use std::time::Duration;

use bdd_infra::rp_harness::{start_rp, write_temp_config_file, OmniSimHandle, WebhookReceiver};
use bdd_infra::ServiceHandle;
use cucumber::{given, then, when};

use crate::world::SessionRunnerWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given("a running Alpaca simulator")]
async fn running_alpaca_simulator(world: &mut SessionRunnerWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
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
    register_session_runner_plugin(world);
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

async fn configure_default_equipment(world: &mut SessionRunnerWorld) {
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
                poll_interval: Some(std::time::Duration::from_millis(100)),
            });
    }
}

/// Start the session-runner service under test: an ephemeral port, the
/// package's shipped `workflows/` directory (the cucumber runner's cwd is
/// the package dir — `bdd_main!` chdirs to `BDD_PACKAGE_DIR` under Bazel),
/// and a scenario-scoped temp `state_dir`.
async fn start_session_runner_service(world: &mut SessionRunnerWorld) {
    if world.session_runner.is_some() {
        return;
    }

    let workflows_dir = std::env::current_dir()
        .expect("cannot read the cwd")
        .join("workflows");
    assert!(
        workflows_dir.is_dir(),
        "shipped workflows directory not found at {}",
        workflows_dir.display()
    );
    let state_dir = tempfile::tempdir().expect("cannot create a state_dir");

    let config = serde_json::json!({
        "port": 0,
        "workflows_dir": workflows_dir,
        "state_dir": state_dir.path(),
    });
    let config_path = write_temp_config_file("session-runner-config", &config).await;
    world.state_dir = Some(state_dir);

    world.session_runner = Some(ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await);
}

/// Register session-runner as rp's orchestrator plugin. The registration's
/// `config` object — forwarded verbatim by rp at invocation — names the
/// shipped document and carries the invocation parameters. Tolerance `1.0`
/// and `max_iterations = 1` mirror the Rust calibrator-flats suite: these
/// scenarios verify end-to-end plumbing, not convergence math (the engine's
/// unit tests own that).
fn register_session_runner_plugin(world: &mut SessionRunnerWorld) {
    let handle = world
        .session_runner
        .as_ref()
        .expect("session-runner not started");
    let invoke_url = format!("{}/invoke", handle.base_url);

    let filters: Vec<serde_json::Value> = world
        .flat_plan
        .iter()
        .map(|(name, count)| serde_json::json!({ "name": name, "count": count }))
        .collect();

    world.plugin_configs.push(serde_json::json!({
        "name": "session-runner",
        "type": "orchestrator",
        "invoke_url": invoke_url,
        "requires_tools": [],
        "config": {
            "workflow": "calibrator_flats",
            "parameters": {
                "camera_id": "main-cam",
                "filter_wheel_id": "main-fw",
                "calibrator_id": "flat-panel",
                "target_adu_fraction": 0.5,
                "tolerance": 1.0,
                "max_iterations": 1,
                "initial_duration": "100ms",
                "filters": filters
            }
        }
    }));
}

async fn start_rp_service(world: &mut SessionRunnerWorld) {
    if world.rp.as_ref().is_some_and(|h| h.is_running()) {
        return;
    }

    let config = world.build_rp_config();
    world.rp = Some(start_rp(&config).await);

    assert!(
        world.wait_for_rp_healthy().await,
        "rp did not become healthy within timeout"
    );
}

async fn ensure_webhook_receiver(world: &mut SessionRunnerWorld) {
    if world.webhook_receiver.is_some() {
        return;
    }
    let (estimated, max) = world
        .webhook_ack_config
        .unwrap_or((Duration::from_secs(5), Duration::from_secs(10)));
    let events = world.received_events.clone();
    world.webhook_receiver = Some(WebhookReceiver::start(events, estimated, max).await);
}

fn add_event_plugin(world: &mut SessionRunnerWorld, events: Vec<String>) {
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
