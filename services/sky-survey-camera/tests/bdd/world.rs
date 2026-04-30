// Phase-2 scaffold: most fields and helpers are populated/used only after
// step bodies are filled in during phase 3 — silence dead-code warnings
// so the precommit hook (`-D warnings`) stays green.
#![allow(dead_code)]

use bdd_infra::ServiceHandle;
use cucumber::World;
use serde_json::Value;
use std::path::PathBuf;
use tempfile::TempDir;

/// Cucumber World for sky-survey-camera BDD scenarios.
///
/// Phase-2 scaffold: fields are sufficient to drive the scenarios in
/// `tests/features/`. Step bodies are mostly `todo!()` and will panic
/// at runtime — that is the expected phase-2 state per
/// `docs/skills/development-workflow.md`.
#[derive(Debug, Default, World)]
pub struct SkySurveyCameraWorld {
    /// Spawned binary handle (set when the service is started).
    pub service: Option<ServiceHandle>,

    /// Temp dir holding config.json + cache dir for the running scenario.
    pub temp_dir: Option<TempDir>,

    /// Path to the config.json the service was started with.
    pub config_path: Option<PathBuf>,

    /// Optics config under construction by Given steps.
    pub focal_length_mm: Option<f64>,
    pub pixel_size_x_um: Option<f64>,
    pub pixel_size_y_um: Option<f64>,
    pub sensor_width_px: Option<u32>,
    pub sensor_height_px: Option<u32>,

    /// Initial pointing (overridden by POST /sky-survey/position).
    pub initial_ra_deg: f64,
    pub initial_dec_deg: f64,
    pub initial_rotation_deg: f64,

    /// Survey backend choice.
    pub survey_name: Option<String>,

    /// Captured outcomes for Then assertions.
    pub last_http_status: Option<u16>,
    pub last_http_body: Option<String>,
    pub last_ascom_error: Option<u32>,
    pub last_image_dimensions: Option<(u32, u32)>,
    pub last_error: Option<String>,
}

impl SkySurveyCameraWorld {
    /// Build a config JSON value from the accumulated world state.
    /// Used by step definitions in phase 3; defined here so step files
    /// can reference it from the start.
    pub fn build_config_json(&self) -> Value {
        let cache_dir = self
            .temp_dir
            .as_ref()
            .map(|d| d.path().join("cache").to_string_lossy().to_string())
            .unwrap_or_else(|| "/tmp/sky-survey-camera-cache".to_string());

        serde_json::json!({
            "device": {
                "name": "Test Sky Survey Camera",
                "unique_id": "sky-survey-camera-test-001",
                "description": "BDD test instance",
            },
            "optics": {
                "focal_length_mm": self.focal_length_mm.unwrap_or(1000.0),
                "pixel_size_x_um": self.pixel_size_x_um.unwrap_or(3.76),
                "pixel_size_y_um": self.pixel_size_y_um.unwrap_or(3.76),
                "sensor_width_px": self.sensor_width_px.unwrap_or(640),
                "sensor_height_px": self.sensor_height_px.unwrap_or(480),
            },
            "pointing": {
                "initial_ra_deg": self.initial_ra_deg,
                "initial_dec_deg": self.initial_dec_deg,
                "initial_rotation_deg": self.initial_rotation_deg,
            },
            "survey": {
                "name": self.survey_name.clone().unwrap_or_else(|| "DSS2 Red".to_string()),
                "request_timeout": "5s",
                "cache_dir": cache_dir,
            },
            "server": {
                "port": 0,
                "device_number": 0,
            },
        })
    }
}
