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
    /// Override `cover_calibrator.poll_interval` in the emitted rp
    /// config. `None` ⇒ rp's default (3 s). The BDD harness pins this
    /// to a short duration (~100 ms) so cover/calibrator scenarios
    /// don't sit through 3-second polls; production rp deployments use
    /// the upstream default. The OmniSim profile we ship at
    /// `crates/bdd-infra/omnisim-config/...` keeps the simulator-side
    /// transitions short too — both knobs need to be small for the
    /// scenario wall-clock to drop.
    pub poll_interval: Option<std::time::Duration>,
}

/// Focuser equipment entry. `min_position` / `max_position` are the
/// operator-supplied safe-travel bounds enforced by `move_focuser`.
#[derive(Debug, Clone)]
pub struct FocuserConfig {
    pub id: String,
    pub alpaca_url: String,
    pub device_number: u32,
    pub min_position: Option<i32>,
    pub max_position: Option<i32>,
}

/// Singular mount equipment entry. `rp` deployments have at most one
/// mount — piggyback rigs share one across multiple optical trains —
/// so the builder field below is `Option<MountConfig>`, not a `Vec`.
#[derive(Debug, Clone)]
pub struct MountConfig {
    pub alpaca_url: String,
    pub device_number: u32,
    /// Optional post-`Slewing == false` settle time. `None` ⇒ rp's
    /// default (zero). Per-call `settle_after` on `slew` overrides.
    pub settle_after_slew: Option<std::time::Duration>,
}

/// Safety-monitor equipment entry.
#[derive(Debug, Clone)]
pub struct SafetyMonitorConfig {
    pub id: String,
    pub alpaca_url: String,
    pub device_number: u32,
}

/// Plate-solver service config — emitted as the top-level
/// `plate_solver` block in rp's JSON config (parallel to `mount`,
/// `guider`, etc.; the plate solver is an rp-managed service, not
/// equipment).
#[derive(Debug, Clone)]
pub struct PlateSolverConfig {
    pub url: String,
    /// rp HTTP-client outer timeout (the connection-side backstop).
    /// `None` ⇒ rp's default (`60s`).
    pub timeout: Option<std::time::Duration>,
    /// Operator-set search radius applied when the per-call MCP
    /// parameter is omitted. `None` ⇒ omit from rp config (wrapper
    /// falls through to ASTAP's own default).
    pub default_search_radius_deg: Option<f64>,
}

/// Accumulates equipment and plugin entries, then emits rp's JSON config.
#[derive(Debug, Default, Clone)]
pub struct RpConfigBuilder {
    pub cameras: Vec<CameraConfig>,
    pub filter_wheels: Vec<FilterWheelConfig>,
    pub cover_calibrators: Vec<CoverCalibratorConfig>,
    pub focusers: Vec<FocuserConfig>,
    /// Safety monitors gating the session (see rp.md § Safety).
    pub safety_monitors: Vec<SafetyMonitorConfig>,
    /// Override `safety.poll_interval` in the emitted rp config.
    /// `None` ⇒ rp's default (10 s). Safety scenarios pin this short
    /// (~250 ms) so unsafe/safe transitions are detected quickly.
    pub safety_poll_interval: Option<std::time::Duration>,
    /// Singular mount — at most one per `rp` deployment.
    pub mount: Option<MountConfig>,
    /// Optional plate-solver service config. `None` ⇒ omit the
    /// top-level `plate_solver` block from the emitted config so
    /// rp's `plate_solve` MCP tool reports "not configured".
    pub plate_solver: Option<PlateSolverConfig>,
    /// Optional `(latitude_degrees, longitude_degrees)` site block.
    /// Required for ephemeris-driven scenarios (planner, twilight,
    /// alt/az MCP tools) and for exercising the mount-side site
    /// validation path. None ⇒ rp's `site` field stays absent.
    pub site: Option<(f64, f64)>,
    pub plugin_configs: Vec<Value>,
    /// Override `session.data_directory`. When `None`, the builder
    /// generates a fresh per-call path. The cross-restart BDD scenarios
    /// need to pin the same path across two `start_rp` calls.
    pub data_directory: Option<String>,
    /// Override `imaging.cache_max_mib` / `cache_max_images`. When `None`,
    /// rp's defaults apply (1024 MiB / 8 images).
    pub imaging_overrides: Option<(usize, usize)>,
    /// Override the `centering` block's `(solve_time_estimate,
    /// slew_overhead_estimate)`. When `None`, the block is omitted and
    /// rp's defaults apply (30 s / 10 s). Shrinking these lets a test
    /// drive a sub-second `centering_started` `max_duration_ms` for the
    /// operation watchdog (the advisory outer-loop deadline is
    /// `max_attempts × (duration + solve_time_estimate +
    /// slew_overhead_estimate)`).
    pub centering: Option<(std::time::Duration, std::time::Duration)>,
}

impl RpConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_camera(&mut self, camera: CameraConfig) -> &mut Self {
        self.cameras.push(camera);
        self
    }

    pub fn add_filter_wheel(&mut self, fw: FilterWheelConfig) -> &mut Self {
        self.filter_wheels.push(fw);
        self
    }

    pub fn add_cover_calibrator(&mut self, cc: CoverCalibratorConfig) -> &mut Self {
        self.cover_calibrators.push(cc);
        self
    }

    pub fn add_focuser(&mut self, foc: FocuserConfig) -> &mut Self {
        self.focusers.push(foc);
        self
    }

    pub fn add_safety_monitor(&mut self, sm: SafetyMonitorConfig) -> &mut Self {
        self.safety_monitors.push(sm);
        self
    }

    /// Override rp's safety poll interval (overwrites any prior call).
    /// When unset, the emitted `safety` block is empty and rp's default
    /// (10 s) applies.
    pub fn with_safety_poll_interval(&mut self, interval: std::time::Duration) -> &mut Self {
        self.safety_poll_interval = Some(interval);
        self
    }

    /// Set the singular mount config (overwrites any prior call).
    pub fn with_mount(&mut self, mount: MountConfig) -> &mut Self {
        self.mount = Some(mount);
        self
    }

    /// Set the plate-solver service config (overwrites any prior
    /// call). When unset, the emitted rp config has no
    /// `plate_solver` block and the `plate_solve` MCP tool reports
    /// "not configured".
    pub fn with_plate_solver(&mut self, plate_solver: PlateSolverConfig) -> &mut Self {
        self.plate_solver = Some(plate_solver);
        self
    }

    /// Set the observer site (latitude/longitude in degrees). Used by
    /// ephemeris and planner scenarios; also required to exercise
    /// the mount-side site validation rule on connect.
    pub fn with_site(&mut self, latitude_degrees: f64, longitude_degrees: f64) -> &mut Self {
        self.site = Some((latitude_degrees, longitude_degrees));
        self
    }

    pub fn add_plugin(&mut self, plugin: Value) -> &mut Self {
        self.plugin_configs.push(plugin);
        self
    }

    /// Pin `session.data_directory` to an explicit path. Used by the
    /// cross-restart BDD scenarios to keep two consecutive rp processes
    /// pointing at the same on-disk archive.
    pub fn with_data_directory(&mut self, path: impl Into<String>) -> &mut Self {
        self.data_directory = Some(path.into());
        self
    }

    /// Override the imaging-cache budgets (`cache_max_mib`,
    /// `cache_max_images`). Used by tests that want to drive evictions
    /// (e.g. setting `cache_max_images = 1` so the second capture evicts
    /// the first).
    pub fn with_imaging(&mut self, cache_max_mib: usize, cache_max_images: usize) -> &mut Self {
        self.imaging_overrides = Some((cache_max_mib, cache_max_images));
        self
    }

    /// Override the `centering` deadline estimates (`solve_time_estimate`,
    /// `slew_overhead_estimate`). Used by the operation-watchdog e2e to
    /// shrink the advisory `centering_started` `max_duration_ms` so the
    /// Sentinel watchdog's per-operation timer fires in a couple of
    /// seconds instead of the ~40 s the defaults imply.
    pub fn with_centering(
        &mut self,
        solve_time_estimate: std::time::Duration,
        slew_overhead_estimate: std::time::Duration,
    ) -> &mut Self {
        self.centering = Some((solve_time_estimate, slew_overhead_estimate));
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
                let mut obj = serde_json::json!({
                    "id": cc.id,
                    "alpaca_url": cc.alpaca_url,
                    "device_number": cc.device_number,
                });
                if let Some(poll) = cc.poll_interval {
                    obj["poll_interval"] = serde_json::json!(format!("{}ms", poll.as_millis()));
                }
                obj
            })
            .collect();

        let focusers: Vec<Value> = self
            .focusers
            .iter()
            .map(|f| {
                let mut obj = serde_json::json!({
                    "id": f.id,
                    "alpaca_url": f.alpaca_url,
                    "device_number": f.device_number,
                });
                if let Some(min) = f.min_position {
                    obj["min_position"] = serde_json::json!(min);
                }
                if let Some(max) = f.max_position {
                    obj["max_position"] = serde_json::json!(max);
                }
                obj
            })
            .collect();

        let safety_monitors: Vec<Value> = self
            .safety_monitors
            .iter()
            .map(|sm| {
                serde_json::json!({
                    "id": sm.id,
                    "alpaca_url": sm.alpaca_url,
                    "device_number": sm.device_number,
                })
            })
            .collect();

        let mut safety = serde_json::json!({});
        if let Some(poll) = self.safety_poll_interval {
            safety["poll_interval"] = serde_json::json!(format!("{}ms", poll.as_millis()));
        }

        let pid = std::process::id();
        let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);

        let data_directory = self.data_directory.clone().unwrap_or_else(|| {
            std::env::temp_dir()
                .join(format!("rp-test-data-{}-{}", pid, seq))
                .to_string_lossy()
                .to_string()
        });

        let mount_value: Value = match &self.mount {
            Some(m) => {
                let mut obj = serde_json::json!({
                    "alpaca_url": m.alpaca_url,
                    "device_number": m.device_number,
                });
                if let Some(d) = m.settle_after_slew {
                    obj["settle_after_slew"] = serde_json::json!(format!("{}ms", d.as_millis()));
                }
                obj
            }
            None => Value::Null,
        };

        let mut config = serde_json::json!({
            "session": {
                "data_directory": data_directory,
                "session_state_file": std::env::temp_dir()
                    .join(format!("rp-test-session-{}-{}.json", pid, seq))
                    .to_string_lossy()
                    .to_string(),
                "file_naming_pattern": "{target}_{filter}_{duration}s_{sequence:04}"
            },
            "equipment": {
                "cameras": cameras,
                "mount": mount_value,
                "focusers": focusers,
                "filter_wheels": filter_wheels,
                "cover_calibrators": cover_calibrators,
                "safety_monitors": safety_monitors
            },
            "plugins": self.plugin_configs,
            "targets": [],
            "planner": {
                "min_altitude_degrees": 20,
                "dawn_buffer_minutes": 30,
                "prefer_transiting": true,
                "minimize_filter_changes": true
            },
            "safety": safety,
            "server": {
                "port": 0,
                "bind_address": "127.0.0.1"
            }
        });

        if let Some((max_mib, max_images)) = self.imaging_overrides {
            config["imaging"] = serde_json::json!({
                "cache_max_mib": max_mib,
                "cache_max_images": max_images,
            });
        }

        if let Some((lat, lon)) = self.site {
            config["site"] = serde_json::json!({
                "latitude_degrees": lat,
                "longitude_degrees": lon,
            });
        }

        if let Some(ps) = &self.plate_solver {
            let mut block = serde_json::json!({
                "url": ps.url,
            });
            if let Some(t) = ps.timeout {
                block["timeout"] = serde_json::json!(format!("{}ms", t.as_millis()));
            }
            if let Some(r) = ps.default_search_radius_deg {
                block["default_search_radius_deg"] = serde_json::json!(r);
            }
            config["plate_solver"] = block;
        }

        if let Some((solve, slew_overhead)) = self.centering {
            config["centering"] = serde_json::json!({
                "solve_time_estimate": format!("{}ms", solve.as_millis()),
                "slew_overhead_estimate": format!("{}ms", slew_overhead.as_millis()),
            });
        }

        config
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
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
    fn filter_wheel_camera_id_follows_first_camera() {
        let mut b = RpConfigBuilder::new();
        b.add_camera(CameraConfig {
            id: "imaging-cam".to_string(),
            alpaca_url: "http://127.0.0.1:1234".to_string(),
            device_number: 0,
        });
        b.add_filter_wheel(FilterWheelConfig {
            id: "main-fw".to_string(),
            alpaca_url: "http://127.0.0.1:1234".to_string(),
            device_number: 0,
            filters: vec!["Luminance".to_string()],
        });
        let cfg = b.build();
        assert_eq!(
            cfg["equipment"]["filter_wheels"][0]["camera_id"],
            "imaging-cam"
        );
    }

    #[test]
    fn site_block_omitted_by_default() {
        let cfg = RpConfigBuilder::new().build();
        assert!(
            cfg.get("site").is_none(),
            "expected site key to be absent when not set, got: {:?}",
            cfg.get("site")
        );
    }

    #[test]
    fn with_site_emits_site_block() {
        let mut b = RpConfigBuilder::new();
        b.with_site(47.6062, -122.3321);
        let cfg = b.build();
        let site = cfg.get("site").expect("site block must be present");
        assert_eq!(site["latitude_degrees"], 47.6062);
        assert_eq!(site["longitude_degrees"], -122.3321);
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
    fn empty_builder_emits_null_mount() {
        let cfg = RpConfigBuilder::new().build();
        assert!(cfg["equipment"]["mount"].is_null());
    }

    #[test]
    fn with_mount_emits_typed_block() {
        let mut b = RpConfigBuilder::new();
        b.with_mount(MountConfig {
            alpaca_url: "http://127.0.0.1:11122".to_string(),
            device_number: 0,
            settle_after_slew: Some(std::time::Duration::from_millis(150)),
        });
        let cfg = b.build();
        let mount = &cfg["equipment"]["mount"];
        assert_eq!(mount["alpaca_url"], "http://127.0.0.1:11122");
        assert_eq!(mount["device_number"], 0);
        assert_eq!(mount["settle_after_slew"], "150ms");
    }

    #[test]
    fn with_mount_omits_settle_when_none() {
        let mut b = RpConfigBuilder::new();
        b.with_mount(MountConfig {
            alpaca_url: "http://127.0.0.1:11122".to_string(),
            device_number: 0,
            settle_after_slew: None,
        });
        let cfg = b.build();
        assert!(cfg["equipment"]["mount"]["settle_after_slew"].is_null());
    }

    #[test]
    fn plate_solver_block_omitted_by_default() {
        let cfg = RpConfigBuilder::new().build();
        assert!(
            cfg.get("plate_solver").is_none(),
            "expected plate_solver key to be absent when not set, got: {:?}",
            cfg.get("plate_solver")
        );
    }

    #[test]
    fn with_plate_solver_emits_url_only_block() {
        let mut b = RpConfigBuilder::new();
        b.with_plate_solver(PlateSolverConfig {
            url: "http://127.0.0.1:11131".to_string(),
            timeout: None,
            default_search_radius_deg: None,
        });
        let cfg = b.build();
        let ps = &cfg["plate_solver"];
        assert_eq!(ps["url"], "http://127.0.0.1:11131");
        assert!(
            ps.get("timeout").is_none(),
            "expected timeout to be omitted when None"
        );
        assert!(
            ps.get("default_search_radius_deg").is_none(),
            "expected default_search_radius_deg to be omitted when None"
        );
    }

    #[test]
    fn with_plate_solver_emits_timeout_and_default_search_radius() {
        let mut b = RpConfigBuilder::new();
        b.with_plate_solver(PlateSolverConfig {
            url: "http://127.0.0.1:11131".to_string(),
            timeout: Some(std::time::Duration::from_secs(30)),
            default_search_radius_deg: Some(3.5),
        });
        let cfg = b.build();
        let ps = &cfg["plate_solver"];
        assert_eq!(ps["url"], "http://127.0.0.1:11131");
        assert_eq!(ps["timeout"], "30000ms");
        assert_eq!(ps["default_search_radius_deg"], 3.5);
    }

    #[test]
    fn centering_block_omitted_by_default() {
        let cfg = RpConfigBuilder::new().build();
        assert!(
            cfg.get("centering").is_none(),
            "expected centering key absent when not set, got: {:?}",
            cfg.get("centering")
        );
    }

    #[test]
    fn with_centering_emits_humantime_block() {
        let mut b = RpConfigBuilder::new();
        b.with_centering(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(500),
        );
        let cfg = b.build();
        let c = &cfg["centering"];
        assert_eq!(c["solve_time_estimate"], "1000ms");
        assert_eq!(c["slew_overhead_estimate"], "500ms");
    }

    #[test]
    fn safety_block_empty_and_no_monitors_by_default() {
        let cfg = RpConfigBuilder::new().build();
        assert_eq!(cfg["safety"], serde_json::json!({}));
        assert!(cfg["equipment"]["safety_monitors"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn safety_monitor_and_poll_interval_are_emitted() {
        let mut b = RpConfigBuilder::new();
        b.add_safety_monitor(SafetyMonitorConfig {
            id: "weather-watcher".to_string(),
            alpaca_url: "http://127.0.0.1:32323".to_string(),
            device_number: 0,
        });
        b.with_safety_poll_interval(std::time::Duration::from_millis(250));
        let cfg = b.build();
        let sm = &cfg["equipment"]["safety_monitors"][0];
        assert_eq!(sm["id"], "weather-watcher");
        assert_eq!(sm["alpaca_url"], "http://127.0.0.1:32323");
        assert_eq!(sm["device_number"], 0);
        assert_eq!(cfg["safety"]["poll_interval"], "250ms");
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
