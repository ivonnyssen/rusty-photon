//! Fluent builder for rp's JSON config + helper for the calibrator-flats
//! service config.
//!
//! Everything here is pure Rust — no I/O, no process spawning — so it's
//! trivial to unit-test and cheap to call from `Given` steps.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

/// Per-process counter so each call to [`RpConfigBuilder::build`] produces a
/// distinct `data_directory` and `session_state_file`. Combined with the PID,
/// this prevents two test binaries (e.g. `cargo test -p rp` running alongside
/// `cargo test -p calibrator-flats`) from clobbering each other's session
/// state, and prevents a rp-test-binary's scenario N from inheriting stale
/// session state from scenario N-1 when a prior scenario did not land cleanly
/// on `idle`.
static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);

/// Camera equipment entry.
#[derive(Debug, Clone)]
pub struct CameraConfig {
    pub id: String,
    pub alpaca_url: String,
    pub device_number: u32,
}

/// Filter wheel equipment entry.
#[derive(Debug, Clone)]
pub struct FilterWheelConfig {
    pub id: String,
    pub alpaca_url: String,
    pub device_number: u32,
    pub filters: Vec<String>,
}

/// Cover-calibrator equipment entry.
#[derive(Debug, Clone)]
pub struct CoverCalibratorConfig {
    pub id: String,
    pub alpaca_url: String,
    pub device_number: u32,
}

/// Accumulates equipment and plugin entries, then emits rp's JSON config.
#[derive(Debug, Default, Clone)]
pub struct RpConfigBuilder {
    pub cameras: Vec<CameraConfig>,
    pub filter_wheels: Vec<FilterWheelConfig>,
    pub cover_calibrators: Vec<CoverCalibratorConfig>,
    pub plugin_configs: Vec<Value>,
}

impl RpConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_camera(&mut self, camera: CameraConfig) -> &mut Self {
        self.cameras.push(camera);
        self
    }

    /// Add a `"main-cam"` camera on the given Alpaca URL (device 0) only if
    /// no camera has been configured yet.
    pub fn ensure_default_camera(&mut self, alpaca_url: &str) -> &mut Self {
        if self.cameras.is_empty() {
            self.cameras.push(CameraConfig {
                id: "main-cam".to_string(),
                alpaca_url: alpaca_url.to_string(),
                device_number: 0,
            });
        }
        self
    }

    pub fn add_filter_wheel(&mut self, fw: FilterWheelConfig) -> &mut Self {
        self.filter_wheels.push(fw);
        self
    }

    /// Add a `"main-fw"` filter wheel with LRGB filters on the given Alpaca
    /// URL (device 0) only if no filter wheel has been configured yet.
    pub fn ensure_default_filter_wheel(&mut self, alpaca_url: &str) -> &mut Self {
        if self.filter_wheels.is_empty() {
            self.filter_wheels.push(FilterWheelConfig {
                id: "main-fw".to_string(),
                alpaca_url: alpaca_url.to_string(),
                device_number: 0,
                filters: vec![
                    "Luminance".to_string(),
                    "Red".to_string(),
                    "Green".to_string(),
                    "Blue".to_string(),
                ],
            });
        }
        self
    }

    pub fn add_cover_calibrator(&mut self, cc: CoverCalibratorConfig) -> &mut Self {
        self.cover_calibrators.push(cc);
        self
    }

    /// Add a `"flat-panel"` cover calibrator on the given Alpaca URL (device
    /// 0) only if none has been configured yet.
    pub fn ensure_default_cover_calibrator(&mut self, alpaca_url: &str) -> &mut Self {
        if self.cover_calibrators.is_empty() {
            self.cover_calibrators.push(CoverCalibratorConfig {
                id: "flat-panel".to_string(),
                alpaca_url: alpaca_url.to_string(),
                device_number: 0,
            });
        }
        self
    }

    pub fn add_plugin(&mut self, plugin: Value) -> &mut Self {
        self.plugin_configs.push(plugin);
        self
    }

    /// Serialize into the JSON shape rp's config loader expects.
    pub fn build(&self) -> Value {
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

        let first_camera_id = self
            .cameras
            .first()
            .map(|c| c.id.as_str())
            .unwrap_or("main-cam");

        let filter_wheels: Vec<Value> = self
            .filter_wheels
            .iter()
            .map(|fw| {
                serde_json::json!({
                    "id": fw.id,
                    "camera_id": first_camera_id,
                    "alpaca_url": fw.alpaca_url,
                    "device_number": fw.device_number,
                    "filters": fw.filters
                })
            })
            .collect();

        let cover_calibrators: Vec<Value> = self
            .cover_calibrators
            .iter()
            .map(|cc| {
                serde_json::json!({
                    "id": cc.id,
                    "alpaca_url": cc.alpaca_url,
                    "device_number": cc.device_number
                })
            })
            .collect();

        let pid = std::process::id();
        let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);

        serde_json::json!({
            "session": {
                "data_directory": std::env::temp_dir()
                    .join(format!("rp-test-data-{}-{}", pid, seq))
                    .to_string_lossy()
                    .to_string(),
                "session_state_file": std::env::temp_dir()
                    .join(format!("rp-test-session-{}-{}.json", pid, seq))
                    .to_string_lossy()
                    .to_string(),
                "file_naming_pattern": "{target}_{filter}_{duration}s_{sequence:04}"
            },
            "equipment": {
                "cameras": cameras,
                "mount": null,
                "focusers": [],
                "filter_wheels": filter_wheels,
                "cover_calibrators": cover_calibrators,
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
}

/// Build a JSON config for the calibrator-flats service from a flat plan.
///
/// The resulting config drives the real calibrator-flats orchestrator
/// process against OmniSim's simulated camera/filter wheel/cover calibrator.
/// Tolerance is `1.0` and `max_iterations = 1` so tests verify end-to-end
/// plumbing (3-process coordination, cover lifecycle, session lifecycle)
/// rather than convergence math — the latter is covered by unit tests.
pub fn build_calibrator_flats_config(filters: &[(String, u32)]) -> Value {
    let filter_entries: Vec<Value> = filters
        .iter()
        .map(|(name, count)| {
            serde_json::json!({
                "name": name,
                "count": count,
            })
        })
        .collect();

    serde_json::json!({
        "camera_id": "main-cam",
        "filter_wheel_id": "main-fw",
        "calibrator_id": "flat-panel",
        "target_adu_fraction": 0.5,
        "tolerance": 1.0,
        "max_iterations": 1,
        "initial_duration": "100ms",
        "filters": filter_entries
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_builder_produces_minimal_config() {
        let cfg = RpConfigBuilder::new().build();
        let equipment = cfg.get("equipment").unwrap();
        assert!(equipment
            .get("cameras")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        assert!(equipment
            .get("filter_wheels")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        assert!(equipment
            .get("cover_calibrators")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        assert!(cfg.get("plugins").unwrap().as_array().unwrap().is_empty());
        assert_eq!(cfg["server"]["port"], 0);
        assert_eq!(cfg["server"]["bind_address"], "127.0.0.1");
    }

    #[test]
    fn ensure_default_camera_is_idempotent() {
        let mut b = RpConfigBuilder::new();
        b.ensure_default_camera("http://127.0.0.1:1234");
        b.ensure_default_camera("http://127.0.0.1:9999");
        assert_eq!(b.cameras.len(), 1);
        assert_eq!(b.cameras[0].alpaca_url, "http://127.0.0.1:1234");
        assert_eq!(b.cameras[0].id, "main-cam");
    }

    #[test]
    fn filter_wheel_camera_id_follows_first_camera() {
        let mut b = RpConfigBuilder::new();
        b.add_camera(CameraConfig {
            id: "imaging-cam".to_string(),
            alpaca_url: "http://127.0.0.1:1234".to_string(),
            device_number: 0,
        });
        b.ensure_default_filter_wheel("http://127.0.0.1:1234");
        let cfg = b.build();
        assert_eq!(
            cfg["equipment"]["filter_wheels"][0]["camera_id"],
            "imaging-cam"
        );
    }

    #[test]
    fn default_filter_wheel_has_lrgb() {
        let mut b = RpConfigBuilder::new();
        b.ensure_default_filter_wheel("http://127.0.0.1:1234");
        let filters = b.filter_wheels[0].filters.clone();
        assert_eq!(filters, vec!["Luminance", "Red", "Green", "Blue"]);
    }

    #[test]
    fn add_plugin_accumulates() {
        let mut b = RpConfigBuilder::new();
        b.add_plugin(serde_json::json!({"name": "a", "type": "event"}));
        b.add_plugin(serde_json::json!({"name": "b", "type": "orchestrator"}));
        let cfg = b.build();
        let plugins = cfg["plugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0]["name"], "a");
        assert_eq!(plugins[1]["name"], "b");
    }

    #[test]
    fn calibrator_flats_config_embeds_plan() {
        let plan = vec![("Luminance".to_string(), 2), ("Red".to_string(), 3)];
        let cfg = build_calibrator_flats_config(&plan);
        assert_eq!(cfg["camera_id"], "main-cam");
        assert_eq!(cfg["filter_wheel_id"], "main-fw");
        assert_eq!(cfg["calibrator_id"], "flat-panel");
        assert_eq!(cfg["max_iterations"], 1);
        assert_eq!(cfg["tolerance"], 1.0);
        let filters = cfg["filters"].as_array().unwrap();
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0]["name"], "Luminance");
        assert_eq!(filters[0]["count"], 2);
        assert_eq!(filters[1]["name"], "Red");
        assert_eq!(filters[1]["count"], 3);
    }
}
