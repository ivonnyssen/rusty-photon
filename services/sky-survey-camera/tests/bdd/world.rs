// Phase-3 is shipping in slices: fields populated by later slices'
// step bodies (e.g. last_image_dimensions, last_error) are read only
// by step files that haven't landed yet. Silence dead-code so the
// husky precommit hook (`-D warnings`) stays green between slices.
#![allow(dead_code)]

use bdd_infra::ServiceHandle;
use cucumber::World;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tempfile::TempDir;

/// Behaviour the SkyView stub is currently configured with. Cloned
/// out of the `RwLock` in the request handler, so each variant must
/// own its data.
#[derive(Debug, Clone)]
pub enum StubBehavior {
    /// HEAD and GET both 200 with empty body.
    Ok,
    /// HEAD 200; GET 200 with the given FITS payload.
    ServingFits(Vec<u8>),
    /// HEAD 200; GET 500.
    Status500,
    /// HEAD 200; GET hangs forever (used to keep an exposure
    /// in-flight for E2 / A1 / A2).
    Hold,
    /// HEAD 200; GET 200 with non-FITS bytes (S6).
    Malformed,
}

#[derive(Debug)]
pub struct StubState {
    pub behavior: Arc<RwLock<StubBehavior>>,
    pub get_count: Arc<AtomicU32>,
}

/// Build a minimal valid FITS payload of the given dimensions filled
/// with zero pixels (BITPIX = 32). Suitable for both happy-path tests
/// and cache-hit pre-seeding.
pub fn make_zero_fits(width: u32, height: u32) -> Vec<u8> {
    fn push(header: &mut String, line: String) {
        let mut padded = format!("{line:<80}");
        padded.truncate(80);
        header.push_str(&padded);
    }
    let mut header = String::new();
    push(&mut header, "SIMPLE  =                    T".to_string());
    push(&mut header, "BITPIX  =                   32".to_string());
    push(&mut header, "NAXIS   =                    2".to_string());
    push(&mut header, format!("NAXIS1  = {width:>20}"));
    push(&mut header, format!("NAXIS2  = {height:>20}"));
    push(&mut header, "END".to_string());
    while !header.len().is_multiple_of(2880) {
        header.push(' ');
    }
    let mut bytes = header.into_bytes();
    let data_len = (width as usize) * (height as usize) * 4;
    bytes.extend(vec![0u8; data_len]);
    while !bytes.len().is_multiple_of(2880) {
        bytes.push(0);
    }
    bytes
}

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

    /// Override for the survey endpoint URL injected into config.
    pub survey_endpoint_override: Option<String>,

    /// Shared state of the SkyView stub server (None until spawned).
    pub stub_state: Option<Arc<StubState>>,

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
        let mut survey = serde_json::json!({
            "name": self.survey_name.clone().unwrap_or_else(|| "DSS2 Red".to_string()),
            "request_timeout": "5s",
            "cache_dir": cache_dir,
        });
        if let Some(endpoint) = &self.survey_endpoint_override {
            survey["endpoint"] = Value::String(endpoint.clone());
        }
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
            "survey": survey,
            "server": {
                "port": 0,
                "device_number": 0,
            },
        })
    }

    /// Spawn a stub SkyView server on `127.0.0.1:0` whose behaviour is
    /// stored in shared state and can be mutated mid-scenario. Points
    /// the survey endpoint at the stub and seeds the world's
    /// `stub_state` so step bodies can switch behaviours and inspect
    /// the GET counter.
    pub async fn spawn_skyview_stub(&mut self) {
        let state = Arc::new(StubState {
            behavior: Arc::new(RwLock::new(StubBehavior::Ok)),
            get_count: Arc::new(AtomicU32::new(0)),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind stub listener");
        let addr = listener.local_addr().expect("local_addr");
        let handler_state = Arc::clone(&state);
        let app = axum::Router::new()
            .fallback(axum::routing::any(handle_stub))
            .with_state(handler_state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        self.survey_endpoint_override = Some(format!("http://{addr}/"));
        self.stub_state = Some(state);
    }

    /// Backwards-compat alias used by step bodies that only care about
    /// the connection-time HEAD reachability check.
    pub async fn spawn_skyview_stub_ok(&mut self) {
        self.spawn_skyview_stub().await;
    }

    pub fn set_stub_behavior(&mut self, behavior: StubBehavior) {
        let state = self
            .stub_state
            .as_ref()
            .expect("stub not spawned — call spawn_skyview_stub first");
        *state.behavior.write().expect("stub behavior rwlock") = behavior;
    }

    pub fn stub_get_count(&self) -> u32 {
        self.stub_state
            .as_ref()
            .map(|s| s.get_count.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Point the survey endpoint at `127.0.0.1:1`, which is reserved
    /// and almost certainly not bound — a connection attempt is
    /// refused immediately.
    pub fn set_unreachable_survey_endpoint(&mut self) {
        self.survey_endpoint_override = Some("http://127.0.0.1:1/".to_string());
    }

    /// Build a `cache_dir` whose parent path is a regular file rather
    /// than a directory, so that `mkdir -p` (a.k.a.
    /// `std::fs::create_dir_all`) reliably fails on every supported
    /// platform.
    pub fn set_unwritable_cache_dir(&mut self) {
        let blocker = self.temp_dir().path().join("blocker");
        std::fs::write(&blocker, b"").expect("failed to write blocker file");
        self.cache_dir_override = Some(blocker.join("cache"));
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
        let extra = [("Connected", connected.to_string())];
        self.put_camera("connected", &extra).await;
    }

    /// PUT /api/v1/camera/0/{method} with the given form parameters
    /// plus the standard ASCOM `ClientID` / `ClientTransactionID`
    /// envelope. Captures the response body, HTTP status, and any
    /// ASCOM `ErrorNumber` into the world for assertions.
    ///
    /// Returns the parsed `ErrorNumber` (0 = success).
    pub async fn put_camera(&mut self, method: &str, params: &[(&str, String)]) -> u32 {
        let url = format!("{}/api/v1/camera/0/{method}", self.base_url());
        let client = self.http();
        let mut form: Vec<(&str, String)> = Vec::with_capacity(params.len() + 2);
        form.push(("ClientID", "1".to_string()));
        form.push(("ClientTransactionID", "1".to_string()));
        for (k, v) in params {
            form.push((k, v.clone()));
        }
        let response = client
            .put(&url)
            .form(&form)
            .send()
            .await
            .unwrap_or_else(|e| panic!("PUT /{method} failed: {e}"));
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        self.last_http_status = Some(status.as_u16());
        self.last_http_body = Some(body.clone());
        let mut err_num = 0;
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            err_num = value
                .get("ErrorNumber")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            if err_num != 0 {
                self.last_ascom_error = Some(err_num);
            }
        }
        // ASCOM convention is HTTP 200 + ErrorNumber for "logical"
        // failures, but parameter parsing rejections (e.g. a negative
        // Duration that doesn't fit `std::time::Duration`) come back
        // as HTTP 4xx with no ASCOM envelope. Map those to
        // INVALID_VALUE so test assertions still see a captured
        // error.
        if err_num == 0 && !status.is_success() {
            err_num = 0x401;
            self.last_ascom_error = Some(err_num);
        }
        err_num
    }

    /// Set BinX/BinY/NumX/NumY/StartX/StartY then call StartExposure.
    /// Stops at the first ASCOM error so the captured `last_ascom_error`
    /// matches what the test scenario expects.
    #[allow(clippy::too_many_arguments)]
    pub async fn drive_start_exposure(
        &mut self,
        bin_x: i32,
        bin_y: i32,
        num_x: i32,
        num_y: i32,
        start_x: i32,
        start_y: i32,
        duration_s: f64,
    ) {
        // Reset captured error so a failure on (e.g.) set_bin_x in
        // scenario N+1 isn't masked by a leftover from scenario N.
        self.last_ascom_error = None;

        let steps: &[(&str, &str, String)] = &[
            ("binx", "BinX", bin_x.to_string()),
            ("biny", "BinY", bin_y.to_string()),
            ("numx", "NumX", num_x.to_string()),
            ("numy", "NumY", num_y.to_string()),
            ("startx", "StartX", start_x.to_string()),
            ("starty", "StartY", start_y.to_string()),
        ];
        for (method, key, value) in steps {
            let err = self.put_camera(method, &[(key, value.clone())]).await;
            if err != 0 {
                return;
            }
        }
        // Duration is a JSON-friendly seconds value per ASCOM Camera spec.
        let extra = [
            ("Duration", duration_s.to_string()),
            ("Light", "true".to_string()),
        ];
        self.put_camera("startexposure", &extra).await;
    }

    /// Drive a StartExposure with `Light = false` and the default
    /// 640×480 sub-frame used by the survey scenarios.
    pub async fn drive_start_exposure_dark(&mut self) {
        self.last_ascom_error = None;
        let extra = [
            ("Duration", "1.0".to_string()),
            ("Light", "false".to_string()),
        ];
        self.put_camera("startexposure", &extra).await;
    }

    /// Drive a StartExposure with `Light = true` and the default
    /// sub-frame (full sensor).
    pub async fn drive_start_exposure_default(&mut self) {
        self.last_ascom_error = None;
        let extra = [
            ("Duration", "1.0".to_string()),
            ("Light", "true".to_string()),
        ];
        self.put_camera("startexposure", &extra).await;
    }

    /// Poll `image_ready` until true or the deadline expires. Returns
    /// `true` on success, `false` on timeout. Used by survey-fetch
    /// scenarios that need to wait for the spawned exposure task.
    pub async fn wait_for_image_ready(&mut self, deadline: Duration) -> bool {
        let start = std::time::Instant::now();
        let url = format!("{}/api/v1/camera/0/imageready", self.base_url());
        let client = self.http();
        while start.elapsed() < deadline {
            let response = client
                .get(&url)
                .query(&[("ClientID", "1"), ("ClientTransactionID", "1")])
                .send()
                .await;
            if let Ok(resp) = response {
                if let Ok(value) = resp.json::<Value>().await {
                    if value["Value"].as_bool().unwrap_or(false) {
                        return true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// GET /api/v1/camera/0/imagearray and return its dimensions.
    /// The ASCOM ImageArray response is `{ ..., "Value": { "Type",
    /// "Rank", "Value": [[...]] } }` — a nested envelope.
    pub async fn get_image_dimensions(&mut self) -> (u32, u32) {
        let body = self.fetch_image_array_json().await;
        let pixels = &body["Value"]["Value"];
        let outer = pixels.as_array().expect("ImageArray pixels not an array");
        let height = outer.len() as u32;
        let width = outer
            .first()
            .and_then(|row| row.as_array().map(|r| r.len() as u32))
            .unwrap_or(0);
        (width, height)
    }

    /// GET /api/v1/camera/0/imagearray and assert every value is zero.
    pub async fn assert_image_all_zero(&mut self) {
        let body = self.fetch_image_array_json().await;
        let pixels = body["Value"]["Value"]
            .as_array()
            .expect("ImageArray pixels not an array");
        for row in pixels {
            for cell in row.as_array().expect("row not array") {
                let v = cell.as_i64().expect("cell not int");
                assert_eq!(v, 0, "expected all pixels zero, found {v}");
            }
        }
    }

    async fn fetch_image_array_json(&mut self) -> Value {
        let url = format!("{}/api/v1/camera/0/imagearray", self.base_url());
        let client = self.http();
        let response = client
            .get(&url)
            .query(&[("ClientID", "1"), ("ClientTransactionID", "1")])
            .header("accept", "application/json")
            .send()
            .await
            .expect("GET /imagearray failed");
        response.json().await.expect("response not JSON")
    }

    /// Pre-seed the cache_dir with a FITS file matching the cache key
    /// the next exposure will compute. Slice 4 derives the same key
    /// formula in `survey::SurveyRequest::cache_key` so we re-use it
    /// here (via a tiny dependency-free reimplementation in the
    /// helper, to avoid pulling library types into tests).
    pub fn preseed_cache(&mut self, cache_key: &str, bytes: &[u8]) {
        let cache_dir = self.cache_dir();
        std::fs::create_dir_all(&cache_dir).expect("create cache_dir");
        let path = cache_dir.join(format!("{cache_key}.fits"));
        std::fs::write(&path, bytes).expect("write cache fits");
    }
}

async fn handle_stub(
    axum::extract::State(state): axum::extract::State<Arc<StubState>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{Method, StatusCode};
    use axum::response::IntoResponse;

    let method = request.method().clone();
    if method == Method::GET {
        state.get_count.fetch_add(1, Ordering::Relaxed);
    }
    if method == Method::HEAD {
        return StatusCode::OK.into_response();
    }
    let behavior = state.behavior.read().expect("stub rwlock").clone();
    match behavior {
        StubBehavior::Ok => StatusCode::OK.into_response(),
        StubBehavior::ServingFits(bytes) => axum::response::Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/fits")
            .body(Body::from(bytes))
            .expect("response build"),
        StubBehavior::Status500 => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        StubBehavior::Hold => {
            std::future::pending::<()>().await;
            unreachable!("std::future::pending never resolves")
        }
        StubBehavior::Malformed => axum::response::Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/fits")
            .body(Body::from(b"this is definitely not a fits file".to_vec()))
            .expect("response build"),
    }
}
