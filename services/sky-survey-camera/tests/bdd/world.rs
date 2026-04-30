// Phase-3 is shipping in slices: fields populated by later slices'
// step bodies (e.g. last_image_dimensions, last_error) are read only
// by step files that haven't landed yet. Silence dead-code so the
// husky precommit hook (`-D warnings`) stays green between slices.
#![allow(dead_code)]

use bdd_infra::ServiceHandle;
use cucumber::World;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

/// Cucumber World for sky-survey-camera BDD scenarios.
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

    /// Initial pointing baked into the config (overridden at runtime by
    /// POST /sky-survey/position).
    pub initial_ra_deg: f64,
    pub initial_dec_deg: f64,
    pub initial_rotation_deg: f64,

    /// Override for cache_dir; if set it's substituted into the config
    /// instead of the default `<temp_dir>/cache`. Used to feed
    /// connection-lifecycle scenarios a deliberately non-writable path.
    pub cache_dir_override: Option<PathBuf>,

    /// Survey backend choice.
    pub survey_name: Option<String>,

    /// HTTP client reused across step calls for performance.
    pub http: Option<reqwest::Client>,

    /// Captured outcomes for Then assertions.
    pub last_http_status: Option<u16>,
    pub last_http_body: Option<String>,
    pub last_ascom_error: Option<u32>,
    pub last_image_dimensions: Option<(u32, u32)>,
    pub last_error: Option<String>,
}

impl SkySurveyCameraWorld {
    pub fn http(&mut self) -> reqwest::Client {
        self.http
            .get_or_insert_with(|| {
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()
                    .expect("failed to build reqwest client")
            })
            .clone()
    }

    pub fn temp_dir(&mut self) -> &TempDir {
        self.temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"))
    }

    pub fn cache_dir(&mut self) -> PathBuf {
        if let Some(override_path) = self.cache_dir_override.clone() {
            return override_path;
        }
        self.temp_dir().path().join("cache")
    }

    pub fn build_config_json(&mut self) -> Value {
        let cache_dir = self.cache_dir().to_string_lossy().to_string();
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

    /// Write the accumulated config to `<temp_dir>/config.json` and
    /// spawn the service binary. Stores the handle on the world.
    pub async fn start_service(&mut self) {
        let config = self.build_config_json();
        let config_path = {
            let dir = self.temp_dir();
            let path = dir.path().join("config.json");
            std::fs::write(&path, config.to_string()).expect("failed to write config.json");
            path
        };
        let handle =
            ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;
        self.config_path = Some(config_path);
        self.service = Some(handle);
    }

    pub fn base_url(&self) -> String {
        let handle = self.service.as_ref().expect("service not started");
        format!("http://127.0.0.1:{}", handle.port)
    }

    /// PUT /api/v1/camera/0/connected — toggle ASCOM Connected.
    pub async fn set_camera_connected(&mut self, connected: bool) {
        let url = format!("{}/api/v1/camera/0/connected", self.base_url());
        let client = self.http();
        let response = client
            .put(&url)
            .form(&[
                ("Connected", connected.to_string()),
                ("ClientID", "1".to_string()),
                ("ClientTransactionID", "1".to_string()),
            ])
            .send()
            .await
            .expect("PUT /connected failed");
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        self.last_http_status = Some(status.as_u16());
        self.last_http_body = Some(body.clone());
        if status.is_success() {
            // Surface the ASCOM ErrorNumber from the response envelope so
            // tests can distinguish HTTP 200 + ASCOM error from real
            // success.
            if let Ok(value) = serde_json::from_str::<Value>(&body) {
                let err_num = value
                    .get("ErrorNumber")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if err_num != 0 {
                    self.last_ascom_error = Some(err_num as u32);
                }
            }
        }
    }
}
