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
    SafetyMonitorConfig, SseClient, WebhookReceiver,
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
    /// Safety monitors gating the session (recovery.feature's safety
    /// interruption scenario).
    pub safety_monitors: Vec<SafetyMonitorConfig>,
    /// Override rp's `safety.poll_interval`; pinned short so unsafe/safe
    /// transitions are detected in test time.
    pub safety_poll_interval: Option<Duration>,
    pub plugin_configs: Vec<Value>,
    /// The orchestrator registration's `config` object (workflow name +
    /// parameters), kept so the recovery scenarios can re-invoke the
    /// session with the exact object rp forwarded on the first invocation.
    pub orchestrator_config: Option<Value>,

    // --- Recovery scenario state ---
    /// The blackboard's frame counter, read just before a recovery
    /// re-invocation — the resumed run must capture exactly
    /// `plan - frames_before_resume` more exposures.
    pub frames_before_resume: Option<u64>,

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
        for sm in &self.safety_monitors {
            builder.add_safety_monitor(sm.clone());
        }
        if let Some(interval) = self.safety_poll_interval {
            builder.with_safety_poll_interval(interval);
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

    /// The `session_id` from the last `/api/session/start` response.
    pub fn session_id(&self) -> String {
        self.start_response_field("session_id")
    }

    /// The `workflow_id` from the last `/api/session/start` response.
    pub fn workflow_id(&self) -> String {
        self.start_response_field("workflow_id")
    }

    fn start_response_field(&self, field: &str) -> String {
        self.last_api_body
            .as_ref()
            .and_then(|body| body.get(field))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("no `{field}` captured — start a session via the REST API"))
            .to_owned()
    }

    /// The spawned session-runner's blackboard file for the current session.
    pub fn blackboard_path(&self) -> std::path::PathBuf {
        self.state_dir
            .as_ref()
            .expect("session-runner must be started before reading its state_dir")
            .path()
            .join(format!("{}.json", self.session_id()))
    }

    /// The persisted `session.frames` counter, when the blackboard file
    /// exists and carries one.
    pub async fn blackboard_frames(&self) -> Option<u64> {
        let bytes = tokio::fs::read(self.blackboard_path()).await.ok()?;
        let session: Value = serde_json::from_slice(&bytes).ok()?;
        // The engine's expression layer stores numbers as f64 (`2.0`,
        // not `2`), so `as_u64()` would reject every real counter;
        // accept exactly-integral values and fail loud on anything else.
        let frames = session.get("frames")?.as_f64()?;
        assert!(
            frames >= 0.0 && frames.fract() == 0.0,
            "the blackboard's `frames` is not a whole non-negative number: {frames}"
        );
        Some(frames as u64)
    }
}
