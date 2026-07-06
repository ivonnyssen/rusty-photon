//! Shared BDD infrastructure helpers for the session-runner suite: the
//! three-process topology (OmniSim + rp + session-runner) and the
//! orchestrator registration, reused by every feature's step definitions.

use std::time::Duration;

use bdd_infra::rp_harness::{start_rp, write_temp_config_file, OmniSimHandle, WebhookReceiver};
use bdd_infra::ServiceHandle;
use serde_json::Value;

use crate::world::SessionRunnerWorld;

pub async fn ensure_omnisim(world: &mut SessionRunnerWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
}

/// The default equipment set: one camera, one filter wheel, one cover
/// calibrator, all on OmniSim device 0. Scenarios that need less simply
/// don't reference the rest.
pub async fn configure_default_equipment(world: &mut SessionRunnerWorld) {
    ensure_omnisim(world).await;
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

/// Start the session-runner service under test: an ephemeral port, a
/// scenario-scoped temp `state_dir`, and a temp `workflows_dir` merging
/// the package's shipped `workflows/` with the suite's purpose-built
/// documents from `tests/fixtures/workflows/` (the cucumber runner's cwd
/// is the package dir — `bdd_main!` chdirs to `BDD_PACKAGE_DIR` under
/// Bazel).
pub async fn start_session_runner_service(world: &mut SessionRunnerWorld) {
    if world.session_runner.is_some() {
        return;
    }

    let cwd = std::env::current_dir().expect("cannot read the cwd");
    let workflows_dir = tempfile::tempdir().expect("cannot create a workflows_dir");
    let mut copied = 0;
    for source in [cwd.join("workflows"), cwd.join("tests/fixtures/workflows")] {
        let entries = std::fs::read_dir(&source)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", source.display()));
        for entry in entries {
            let path = entry.expect("cannot read a workflows entry").path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let name = path.file_name().expect("a file has a name");
                std::fs::copy(&path, workflows_dir.path().join(name))
                    .unwrap_or_else(|e| panic!("cannot copy {}: {e}", path.display()));
                copied += 1;
            }
        }
    }
    assert!(copied > 0, "no workflow documents found to copy");

    let state_dir = tempfile::tempdir().expect("cannot create a state_dir");
    let config = serde_json::json!({
        "port": 0,
        "workflows_dir": workflows_dir.path(),
        "state_dir": state_dir.path(),
    });
    let config_path = write_temp_config_file("session-runner-config", &config).await;
    world.workflows_dir = Some(workflows_dir);
    world.state_dir = Some(state_dir);

    world.session_runner = Some(ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await);
}

/// Register session-runner as rp's orchestrator plugin. The registration's
/// `config` object — forwarded verbatim by rp at invocation — names the
/// workflow document and carries its invocation parameters.
pub fn register_orchestrator(
    world: &mut SessionRunnerWorld,
    workflow: &str,
    parameters: Option<Value>,
) {
    let handle = world
        .session_runner
        .as_ref()
        .expect("session-runner not started");
    let invoke_url = format!("{}/invoke", handle.base_url);

    let mut config = serde_json::json!({ "workflow": workflow });
    if let Some(parameters) = parameters {
        config["parameters"] = parameters;
    }
    world.plugin_configs.push(serde_json::json!({
        "name": "session-runner",
        "type": "orchestrator",
        "invoke_url": invoke_url,
        "requires_tools": [],
        "config": config
    }));
}

pub async fn start_rp_service(world: &mut SessionRunnerWorld) {
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

pub async fn ensure_webhook_receiver(world: &mut SessionRunnerWorld) {
    if world.webhook_receiver.is_some() {
        return;
    }
    let (estimated, max) = world
        .webhook_ack_config
        .unwrap_or((Duration::from_secs(5), Duration::from_secs(10)));
    let events = world.received_events.clone();
    world.webhook_receiver = Some(WebhookReceiver::start(events, estimated, max).await);
}

pub fn add_event_plugin(world: &mut SessionRunnerWorld, events: Vec<String>) {
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
