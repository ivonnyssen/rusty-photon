#![allow(dead_code)]
//! BDD test world for rp service
//!
//! Manages the lifecycle of external processes (OmniSim, rp) and
//! in-process test doubles (webhook receiver, test orchestrator)
//! needed for integration testing.
//!
//! The shared types (`OmniSimHandle`, `WebhookReceiver`, `TestOrchestrator`,
//! `McpTestClient`, and the rp config builder) live in the `bdd-infra` crate
//! under the `rp-harness` feature. See `bdd_infra::rp_harness`.

use std::sync::Arc;
use std::time::Duration;

use bdd_infra::rp_harness::{
    CameraConfig, CoverCalibratorConfig, FilterWheelConfig, McpTestClient, OmniSimHandle,
    OrchestratorInvocation, ReceivedEvent, RpConfigBuilder, TestOrchestrator, WebhookReceiver,
};
use bdd_infra::ServiceHandle;
use cucumber::World;
use serde_json::Value;
use tokio::sync::RwLock;

impl std::fmt::Debug for RpWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpWorld").finish_non_exhaustive()
    }
}

#[derive(Default, World)]
pub struct RpWorld {
    // --- Infrastructure handles ---
    /// Running OmniSim process
    pub omnisim: Option<OmniSimHandle>,
    /// Running rp process
    pub rp: Option<ServiceHandle>,
    /// Test webhook receiver (in-process HTTP server acting as an event plugin)
    pub webhook_receiver: Option<WebhookReceiver>,
    /// Test orchestrator (in-process HTTP server acting as an orchestrator plugin)
    pub orchestrator: Option<TestOrchestrator>,
    /// Persistent MCP client for the current scenario
    pub mcp_client: Option<McpTestClient>,

    // --- Configuration building ---
    /// Camera configs accumulated via Given steps
    pub cameras: Vec<CameraConfig>,
    /// Filter wheel configs accumulated via Given steps
    pub filter_wheels: Vec<FilterWheelConfig>,
    /// CoverCalibrator configs accumulated via Given steps
    pub cover_calibrators: Vec<CoverCalibratorConfig>,
    /// Plugin configs accumulated via Given steps
    pub plugin_configs: Vec<Value>,

    // --- Webhook receiver state ---
    /// Events collected by the test webhook receiver
    pub received_events: Arc<RwLock<Vec<ReceivedEvent>>>,
    /// Webhook acknowledgment config (estimated_duration, max_duration)
    pub webhook_ack_config: Option<(Duration, Duration)>,

    // --- Orchestrator state ---
    /// Invocations received by the test orchestrator
    pub orchestrator_invocations: Arc<RwLock<Vec<OrchestratorInvocation>>>,
    /// Whether the test orchestrator was cancelled
    pub orchestrator_cancelled: Arc<RwLock<bool>>,

    // --- MCP client state ---
    /// Last captured image path (for compute_image_stats chaining)
    pub last_image_path: Option<String>,
    /// Last captured document id (for compute_image_stats chaining)
    pub last_document_id: Option<String>,
    /// Last image stats result
    pub last_image_stats: Option<Value>,
    /// Last tool call result
    pub last_tool_result: Option<Result<Value, String>>,
    /// Last tool list result
    pub last_tool_list: Option<Vec<String>>,
    /// Current filter from get_filter
    pub current_filter: Option<String>,

    // --- REST API state ---
    /// Last REST API response status code
    pub last_api_status: Option<u16>,
    /// Last REST API response body
    pub last_api_body: Option<Value>,
    /// Session status from GET /api/session/status
    pub session_status: Option<String>,

    // --- Test flat-calibration orchestrator config ---
    /// Filter name → count, used by the in-process `TestOrchestrator` when
    /// configured with `OrchestratorBehavior::FlatCalibration(...)`.
    pub flat_plan: Vec<(String, u32)>,

    // --- TLS test state ---
    /// Temp directory holding generated PKI (CA + service certs)
    pub tls_pki_dir: Option<tempfile::TempDir>,
    /// Stored CA cert PEM for idempotency comparison
    pub tls_ca_cert_pem: Option<String>,
    /// Last HTTPS response status for TLS validation tests
    pub tls_https_status: Option<u16>,

    // --- ACME test state ---
    /// Last command output (for ACME CLI tests)
    pub last_command_output: Option<std::process::Output>,

    // --- Auth test state ---
    /// Plaintext password used for test auth
    pub auth_password: Option<String>,
    /// Hash output from rp hash-password CLI
    pub auth_hash_output: Option<String>,
}

impl RpWorld {
    /// The base URL for the OmniSim Alpaca simulator.
    /// Panics if OmniSim has not been started yet.
    pub fn omnisim_url(&self) -> String {
        self.omnisim
            .as_ref()
            .expect("OmniSim must be started before accessing its URL")
            .base_url
            .clone()
    }

    /// The base URL for the rp REST API
    pub fn rp_url(&self) -> String {
        self.rp
            .as_ref()
            .map(|h| h.base_url.clone())
            .unwrap_or_else(|| "http://localhost:11115".to_string())
    }

    /// The MCP endpoint URL for rp
    pub fn rp_mcp_url(&self) -> String {
        format!("{}/mcp", self.rp_url())
    }

    /// Get the persistent MCP client, panicking if not connected.
    pub fn mcp(&self) -> &McpTestClient {
        self.mcp_client
            .as_ref()
            .expect("MCP client not connected — add 'Given an MCP client connected to rp' step")
    }

    /// Build the rp config JSON from accumulated Given steps via [`RpConfigBuilder`].
    pub fn build_config(&self) -> Value {
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

    /// Wait for rp to become healthy (retry GET /health).
    /// Timeout: 120 × 250ms = 30s (sanitizer-instrumented binaries start slower).
    pub async fn wait_for_rp_healthy(&self) -> bool {
        bdd_infra::rp_harness::wait_for_rp_healthy(&self.rp_url()).await
    }

    /// Wait for a specific number of events of a given type
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

    /// Wait for the session status to reach an expected value.
    /// Timeout: 40 × 250ms = 10s.
    pub async fn wait_for_session_status(&self, expected: &str) -> bool {
        let client = reqwest::Client::new();
        let url = format!("{}/api/session/status", self.rp_url());
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Ok(resp) = client.get(&url).send().await {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    if body.get("status").and_then(|v| v.as_str()) == Some(expected) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Wait for at least one orchestrator invocation to be recorded.
    /// Timeout: 40 × 250ms = 10s.
    pub async fn wait_for_orchestrator_invocation(&self) -> bool {
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let inv = self.orchestrator_invocations.read().await;
            if !inv.is_empty() {
                return true;
            }
        }
        false
    }
}
