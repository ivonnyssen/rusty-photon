#![allow(dead_code)]
//! BDD test world for the session-runner service.
//!
//! Holds the three external processes (OmniSim, rp, session-runner) plus
//! an in-process webhook receiver. The shared harness types come from
//! `bdd_infra::rp_harness`; everything below is just the per-scenario
//! accumulator state for this service's tests.

use std::sync::Arc;
use std::time::Duration;

use bdd_infra::rp_harness::{
    CameraConfig, CoverCalibratorConfig, FilterWheelConfig, ReceivedEvent, RpConfigBuilder,
    SseClient, WebhookReceiver,
};
use bdd_infra::ServiceHandle;
use cucumber::World;
use serde_json::Value;
use tokio::sync::RwLock;

#[derive(Default, World, derive_more::Debug)]
#[debug("SessionRunnerWorld {{ .. }}")]
pub struct SessionRunnerWorld {
    // --- Infrastructure handles ---
    pub omnisim: Option<bdd_infra::rp_harness::OmniSimHandle>,
    pub rp: Option<ServiceHandle>,
    pub session_runner: Option<ServiceHandle>,
    pub webhook_receiver: Option<WebhookReceiver>,
    /// A test-side subscriber to rp's SSE stream, for seq-ordered
    /// assertions on what the engine's triggers did.
    pub sse_client: Option<SseClient>,
    /// Blackboard persistence directory for the spawned session-runner;
    /// held here so it outlives the scenario's service process.
    pub state_dir: Option<tempfile::TempDir>,
    /// The spawned session-runner's workflows directory: the shipped
    /// `workflows/` merged with `tests/fixtures/workflows/`, built per
    /// scenario.
    pub workflows_dir: Option<tempfile::TempDir>,

    // --- rp config building ---
    pub cameras: Vec<CameraConfig>,
    pub filter_wheels: Vec<FilterWheelConfig>,
    pub cover_calibrators: Vec<CoverCalibratorConfig>,
    pub plugin_configs: Vec<Value>,

    // --- Webhook state ---
    pub received_events: Arc<RwLock<Vec<ReceivedEvent>>>,
    pub webhook_ack_config: Option<(Duration, Duration)>,

    // --- Flat calibration plan ---
    /// Filter name → count, forwarded as the document's `filters`
    /// parameter in the orchestrator registration's `config`.
    pub flat_plan: Vec<(String, u32)>,

    // --- REST API state ---
    pub last_api_status: Option<u16>,
    pub last_api_body: Option<Value>,
}

impl SessionRunnerWorld {
    pub fn omnisim_url(&self) -> String {
        self.omnisim
            .as_ref()
            .expect("OmniSim must be started before accessing its URL")
            .base_url
            .clone()
    }

    pub fn rp_url(&self) -> String {
        self.rp
            .as_ref()
            .map(|h| h.base_url.clone())
            .expect("rp must be started before accessing its URL")
    }

    /// Build the rp config JSON by feeding accumulated equipment and plugin
    /// entries through [`RpConfigBuilder`].
    pub fn build_rp_config(&self) -> Value {
        let mut builder = RpConfigBuilder::new();
        for camera in &self.cameras {
            builder.add_camera(camera.clone());
        }
        for fw in &self.filter_wheels {
            builder.add_filter_wheel(fw.clone());
        }
        for cc in &self.cover_calibrators {
            builder.add_cover_calibrator(cc.clone());
        }
        for plugin in &self.plugin_configs {
            builder.add_plugin(plugin.clone());
        }
        builder.build()
    }

    /// Wait for rp's `/health` endpoint to return 200.
    pub async fn wait_for_rp_healthy(&self) -> bool {
        bdd_infra::rp_harness::wait_for_rp_healthy(&self.rp_url()).await
    }

    /// Poll rp's session status until it reports `idle`, or `budget`
    /// elapses.
    pub async fn wait_for_session_idle(&self, budget: Duration) -> bool {
        let client = reqwest::Client::new();
        let url = format!("{}/api/session/status", self.rp_url());
        let deadline = std::time::Instant::now() + budget;
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Ok(resp) = client.get(&url).send().await {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    if body.get("status").and_then(|v| v.as_str()) == Some("idle") {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Wait for at least `count` events of the given type. 40 × 250ms = 10s.
    pub async fn wait_for_events(&self, event_type: &str, count: usize) -> bool {
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let events = self.received_events.read().await;
            let matching = events.iter().filter(|e| e.event_type == event_type).count();
            if matching >= count {
                return true;
            }
        }
        false
    }
}
