//! Cucumber `World` for the zwo-camera BDD suite.
//!
//! Each scenario spawns the zwo-camera binary (built with the `simulation`
//! backend so the SDK yields one `ASI2600MM-Pro-Simulated` camera) and drives it
//! through the typed `ascom-alpaca` Camera client over real HTTP — mirroring the
//! qhy-camera / qhy-focuser pattern. The EFW filter wheel is Phase F (`@wip`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::camera::GuideDirection;
use ascom_alpaca::api::{Camera, TypedDevice};
use ascom_alpaca::ASCOMErrorCode;
use ascom_alpaca::Client as AlpacaClient;
use bdd_infra::ServiceHandle;
use cucumber::World;
use tempfile::TempDir;

#[derive(Debug, Default, World)]
pub struct CameraWorld {
    pub handle: Option<ServiceHandle>,
    pub camera: Option<Arc<dyn Camera>>,
    pub temp_dir: Option<TempDir>,

    // Config knob set by a Given step before the service starts.
    pub empty_backend: bool,

    // Result stashes ("When does, Then asserts").
    pub last_error_code: Option<u16>,
    pub last_response: Option<serde_json::Value>,
    pub last_actions: Option<Vec<String>>,
}

impl CameraWorld {
    fn write_config(&mut self) -> String {
        let config = serde_json::json!({
            "devices": {},
            "filterwheel": { "enabled": false },
            // Port 0 → OS-assigned; the real port is read from the `bound_addr=`
            // line on stdout by ServiceHandle.
            "server": { "port": 0 },
        });
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("temp dir"));
        let path = dir.path().join("zwo-camera.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config");
        path.to_str().expect("utf8 config path").to_string()
    }

    /// Spawn the service binary and acquire the typed Camera client.
    pub async fn start(&mut self) {
        let config_path = self.write_config();
        let handle = if self.empty_backend {
            ServiceHandle::start_with_args(
                env!("CARGO_PKG_NAME"),
                &["--config", &config_path, "--simulation-empty"],
            )
            .await
        } else {
            ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await
        };
        self.handle = Some(handle);
        self.acquire().await;
    }

    async fn acquire(&mut self) {
        let port = self.handle.as_ref().expect("service handle").port;
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        for _ in 0..80 {
            let client = AlpacaClient::new_from_addr(addr);
            if let Ok(devices) = client.get_devices().await {
                let mut camera = None;
                for device in devices {
                    // A `match` (not `if let`) so this still compiles cleanly once
                    // the Phase-F filter-wheel variant joins `TypedDevice`.
                    #[allow(clippy::single_match)]
                    match device {
                        TypedDevice::Camera(c) => camera = Some(c),
                        #[allow(unreachable_patterns)]
                        _ => {}
                    }
                }
                if self.empty_backend {
                    // Zero cameras is the expected, healthy state here (C0).
                    self.camera = camera;
                    return;
                }
                if camera.is_some() {
                    self.camera = camera;
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if !self.empty_backend {
            panic!("zwo-camera did not register a Camera device within 20s");
        }
    }

    pub fn camera(&self) -> Arc<dyn Camera> {
        Arc::clone(self.camera.as_ref().expect("camera not acquired"))
    }

    pub fn base_url(&self) -> String {
        self.handle
            .as_ref()
            .expect("service handle")
            .base_url
            .clone()
    }

    /// The management API answers a `get_devices` request (server is healthy).
    pub async fn management_responds(&self) -> bool {
        let port = self.handle.as_ref().expect("service handle").port;
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        AlpacaClient::new_from_addr(addr)
            .get_devices()
            .await
            .is_ok()
    }

    /// Start a small, long-running exposure and leave it in flight (the driver
    /// honours the requested duration as the integration time).
    pub async fn start_in_flight(&mut self) {
        let camera = self.camera();
        let _ = camera.set_bin_x(1).await;
        let _ = camera.set_num_x(64).await;
        let _ = camera.set_num_y(64).await;
        let _ = camera.set_start_x(0).await;
        let _ = camera.set_start_y(0).await;
        camera
            .start_exposure(Duration::from_secs(30), true)
            .await
            .expect("start in-flight exposure");
        // Let the detached task enter the blocking capture.
        tokio::time::sleep(Duration::from_millis(120)).await;
    }

    pub async fn wait_image_ready(&self) {
        for _ in 0..240 {
            if self.camera().image_ready().await.unwrap_or(false) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("exposure did not complete within 6s");
    }

    /// Drive a `StartExposure` and stash the ASCOM error code (`None` on success).
    /// Sets bin/ROI via the typed client first; a negative duration (which a
    /// `std::time::Duration` cannot hold) goes via raw HTTP.
    #[allow(clippy::too_many_arguments)]
    pub async fn try_start_exposure(
        &mut self,
        bin_x: u8,
        bin_y: u8,
        num_x: u32,
        num_y: u32,
        start_x: u32,
        start_y: u32,
        duration: f64,
        light: bool,
    ) {
        let camera = self.camera();
        let _ = camera.set_bin_x(bin_x).await;
        let _ = camera.set_bin_y(bin_y).await;
        let _ = camera.set_num_x(num_x).await;
        let _ = camera.set_num_y(num_y).await;
        let _ = camera.set_start_x(start_x).await;
        let _ = camera.set_start_y(start_y).await;

        if duration < 0.0 {
            let code = raw_start_exposure(&self.base_url(), 0, duration, light).await;
            self.last_error_code = (code != 0).then_some(code);
        } else {
            match camera
                .start_exposure(Duration::from_secs_f64(duration), light)
                .await
            {
                Ok(()) => self.last_error_code = None,
                Err(e) => self.last_error_code = Some(e.code.raw()),
            }
        }
    }

    /// Drive a `PulseGuide` and stash the ASCOM error code (`None` on success).
    pub async fn try_pulse_guide(&mut self, direction: GuideDirection, millis: u64) {
        match self
            .camera()
            .pulse_guide(direction, Duration::from_millis(millis))
            .await
        {
            Ok(()) => self.last_error_code = None,
            Err(e) => self.last_error_code = Some(e.code.raw()),
        }
    }

    /// Call a vendor config action; stash the parsed JSON (`last_response`) on
    /// success, or the ASCOM error code (`last_error_code`) on failure.
    pub async fn call_action(&mut self, action: &str, params: &str) {
        match self
            .camera()
            .action(action.to_string(), params.to_string())
            .await
        {
            Ok(body) => {
                self.last_error_code = None;
                self.last_response =
                    Some(serde_json::from_str(&body).expect("action returned invalid JSON"));
            }
            Err(e) => {
                self.last_error_code = Some(e.code.raw());
                self.last_response = None;
            }
        }
    }

    /// The `config` object from a `config.get` response.
    pub async fn config_get(&mut self) -> serde_json::Value {
        self.call_action("config.get", "").await;
        self.last_response
            .as_ref()
            .and_then(|r| r.get("config").cloned())
            .expect("config.get response missing `config`")
    }
}

/// Map an ASCOM error-code *name* (as written in the feature files) to its raw
/// `u16`, so Then steps can assert "rejected with ASCOM <NAME>".
pub fn ascom_code(name: &str) -> u16 {
    match name {
        "INVALID_VALUE" => ASCOMErrorCode::INVALID_VALUE.raw(),
        "NOT_CONNECTED" => ASCOMErrorCode::NOT_CONNECTED.raw(),
        "NOT_IMPLEMENTED" => ASCOMErrorCode::NOT_IMPLEMENTED.raw(),
        "INVALID_OPERATION" => ASCOMErrorCode::INVALID_OPERATION.raw(),
        other => panic!("unknown ASCOM error code name: {other}"),
    }
}

/// Drive `StartExposure` over raw HTTP — the only way to submit a negative
/// `Duration` (the typed client takes a `std::time::Duration`). Returns the
/// response `ErrorNumber` (0 = success).
async fn raw_start_exposure(base_url: &str, device: u32, duration_secs: f64, light: bool) -> u16 {
    let url = format!("{base_url}/api/v1/camera/{device}/startexposure");
    let form = [
        ("Duration", duration_secs.to_string()),
        ("Light", if light { "True" } else { "False" }.to_string()),
        ("ClientID", "1".to_string()),
        ("ClientTransactionID", "1".to_string()),
    ];
    match reqwest::Client::new().put(&url).form(&form).send().await {
        Ok(resp) => {
            let json: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!({}));
            json["ErrorNumber"].as_u64().unwrap_or(0) as u16
        }
        Err(e) => panic!("raw startexposure request failed: {e}"),
    }
}
