#![allow(dead_code)]
//! BDD test world for the calibrator-flats service.
//!
//! Holds the three external processes (OmniSim, rp, calibrator-flats) plus
//! an in-process webhook receiver. The shared harness types come from
//! `bdd_infra::rp_harness`; everything below is just the per-scenario
//! accumulator state for this service's tests.

use std::sync::Arc;
use std::time::Duration;

use bdd_infra::rp_harness::{
    CameraConfig, CoverCalibratorConfig, FilterWheelConfig, ReceivedEvent, RpConfigBuilder,
    WebhookReceiver,
};
use bdd_infra::ServiceHandle;
use cucumber::World;
use serde_json::Value;
use tokio::sync::RwLock;

impl std::fmt::Debug for CalibratorFlatsWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CalibratorFlatsWorld")
            .finish_non_exhaustive()
    }
}

#[derive(Default, World)]
pub struct CalibratorFlatsWorld {
    // --- Infrastructure handles ---
    pub omnisim: Option<bdd_infra::rp_harness::OmniSimHandle>,
    pub rp: Option<ServiceHandle>,
    pub calibrator_flats: Option<ServiceHandle>,
    pub webhook_receiver: Option<WebhookReceiver>,

    // --- rp config building ---
    pub cameras: Vec<CameraConfig>,
    pub filter_wheels: Vec<FilterWheelConfig>,
    pub cover_calibrators: Vec<CoverCalibratorConfig>,
    pub plugin_configs: Vec<Value>,

    // --- Webhook state ---
    pub received_events: Arc<RwLock<Vec<ReceivedEvent>>>,
    pub webhook_ack_config: Option<(u64, u64)>,

    // --- Flat calibration plan ---
    /// Filter name → count for the calibrator-flats service config.
    pub flat_plan: Vec<(String, u32)>,

    // --- REST API state ---
    pub last_api_status: Option<u16>,
    pub last_api_body: Option<Value>,
}

impl CalibratorFlatsWorld {
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
