#![allow(dead_code)]
//! BDD test world for rp service
//!
//! Manages the lifecycle of external processes (OmniSim, rp) and
//! in-process test doubles (webhook receiver, test orchestrator)
//! needed for integration testing.

use std::sync::Arc;
use std::time::Duration;

use cucumber::World;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::steps::infrastructure::{
    OmniSimHandle, ServiceHandle, TestOrchestrator, WebhookReceiver,
};

/// Collected event received by the test webhook receiver
#[derive(Debug, Clone)]
pub struct ReceivedEvent {
    pub event_id: String,
    pub event_type: String,
    pub timestamp: String,
    pub payload: Value,
    pub received_at: std::time::Instant,
}

/// Orchestrator invocation received by the test orchestrator
#[derive(Debug, Clone)]
pub struct OrchestratorInvocation {
    pub workflow_id: String,
    pub session_id: String,
    pub mcp_server_url: String,
    pub recovery: Option<Value>,
}

#[derive(Debug, Default, World)]
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

    // --- Configuration building ---
    /// Camera configs accumulated via Given steps
    pub cameras: Vec<CameraConfig>,
    /// Filter wheel configs accumulated via Given steps
    pub filter_wheels: Vec<FilterWheelConfig>,
    /// Plugin configs accumulated via Given steps
    pub plugin_configs: Vec<Value>,

    // --- Webhook receiver state ---
    /// Events collected by the test webhook receiver
    pub received_events: Arc<RwLock<Vec<ReceivedEvent>>>,
    /// Webhook acknowledgment config (estimated_duration, max_duration)
    pub webhook_ack_config: Option<(u64, u64)>,

    // --- Orchestrator state ---
    /// Invocations received by the test orchestrator
    pub orchestrator_invocations: Arc<RwLock<Vec<OrchestratorInvocation>>>,
    /// Whether the test orchestrator was cancelled
    pub orchestrator_cancelled: Arc<RwLock<bool>>,

    // --- MCP client state ---
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

    // --- Flat calibration orchestrator config ---
    /// Filter name → count for the test flat-calibration orchestrator
    pub flat_plan: Vec<(String, u32, f64)>,
}

/// Camera configuration for test setup
#[derive(Debug, Clone)]
pub struct CameraConfig {
    pub id: String,
    pub alpaca_url: String,
    pub device_number: u32,
}

/// Filter wheel configuration for test setup
#[derive(Debug, Clone)]
pub struct FilterWheelConfig {
    pub id: String,
    pub alpaca_url: String,
    pub device_number: u32,
    pub filters: Vec<String>,
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

    /// Build the rp config JSON from accumulated Given steps
    pub fn build_config(&self) -> Value {
        let cameras: Vec<Value> = self
            .cameras
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "name": c.id,
                    "alpaca_url": c.alpaca_url,
                    "device_type": "camera",
                    "device_number": c.device_number,
                    "cooler_target_c": -10,
                    "gain": 100,
                    "offset": 50
                })
            })
            .collect();

        let filter_wheels: Vec<Value> = self
            .filter_wheels
            .iter()
            .map(|fw| {
                serde_json::json!({
                    "id": fw.id,
                    "camera_id": self.cameras.first().map(|c| c.id.as_str()).unwrap_or("main-cam"),
                    "alpaca_url": fw.alpaca_url,
                    "device_number": fw.device_number,
                    "filters": fw.filters
                })
            })
            .collect();

        let _webhook_url = self
            .webhook_receiver
            .as_ref()
            .map(|w| w.url.clone())
            .unwrap_or_default();

        let _orchestrator_url = self
            .orchestrator
            .as_ref()
            .map(|o| o.invoke_url.clone())
            .unwrap_or_default();

        serde_json::json!({
            "session": {
                "data_directory": std::env::temp_dir().join("rp-test-data").to_string_lossy().to_string(),
                "session_state_file": std::env::temp_dir().join("rp-test-session.json").to_string_lossy().to_string(),
                "file_naming_pattern": "{target}_{filter}_{duration}s_{sequence:04}"
            },
            "equipment": {
                "cameras": cameras,
                "mount": null,
                "focusers": [],
                "filter_wheels": filter_wheels,
                "safety_monitors": []
            },
            "plugins": self.plugin_configs,
            "targets": [],
            "planner": {
                "min_altitude_degrees": 20,
                "dawn_buffer_minutes": 30,
                "prefer_transiting": true,
                "minimize_filter_changes": true
            },
            "safety": {
                "polling_interval_secs": 10,
                "park_on_unsafe": true,
                "resume_on_safe": true,
                "resume_delay_secs": 300
            },
            "server": {
                "port": 0,
                "bind_address": "127.0.0.1"
            }
        })
    }

    /// Wait for rp to become healthy (retry GET /health).
    /// Timeout: 120 × 250ms = 30s (sanitizer-instrumented binaries start slower).
    pub async fn wait_for_rp_healthy(&self) -> bool {
        let client = reqwest::Client::new();
        let url = format!("{}/health", self.rp_url());
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Ok(resp) = client.get(&url).send().await {
                if resp.status().as_u16() == 200 {
                    return true;
                }
            }
        }
        false
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
