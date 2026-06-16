//! BDD test world for sentinel service (binary-spawning pattern)

use std::path::PathBuf;
use std::time::Duration;

use bdd_infra::ServiceHandle;
use cucumber::World;
use tempfile::TempDir;

#[derive(Debug, Default, World)]
pub struct SentinelWorld {
    // Service handles
    pub filemonitor: Option<ServiceHandle>,
    pub sentinel: Option<ServiceHandle>,

    // Temp file management
    pub temp_dir: Option<TempDir>,
    pub temp_file_path: Option<PathBuf>,

    // Filemonitor config accumulation
    pub fm_rules: Vec<serde_json::Value>,
    pub fm_polling_interval: u64,

    // Sentinel config accumulation
    pub sentinel_monitor_name: String,
    pub sentinel_polling_interval: u64,
    pub sentinel_transitions: Vec<serde_json::Value>,
    pub sentinel_has_notifiers: bool,
    pub sentinel_monitors: Vec<serde_json::Value>,

    // Result capture
    pub last_response_body: Option<String>,
    pub last_status_code: Option<u16>,
    pub last_error: Option<String>,

    // TLS test state
    pub tls_pki_dir: Option<TempDir>,

    // Local Pushover API stub so notification scenarios never hit the real
    // api.pushover.net (slow, non-hermetic, rejects test credentials).
    pub pushover_stub: Option<tokio::task::JoinHandle<()>>,
    pub pushover_stub_url: Option<String>,
}

impl SentinelWorld {
    /// Create a temp file with the given content and store its path.
    pub fn create_temp_file(&mut self, content: &str) -> PathBuf {
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let file_path = dir.path().join("monitored.txt");
        std::fs::write(&file_path, content).expect("failed to write temp file");
        self.temp_file_path = Some(file_path.clone());
        file_path
    }

    /// Build filemonitor JSON config from accumulated state.
    pub fn build_filemonitor_config(&self) -> serde_json::Value {
        let file_path = self
            .temp_file_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "nonexistent.txt".to_string());

        let polling = if self.fm_polling_interval > 0 {
            self.fm_polling_interval
        } else {
            60
        };

        serde_json::json!({
            "device": {
                "name": "Test",
                "unique_id": "sentinel-bdd-test",
                "description": "BDD test device"
            },
            "file": {
                "path": file_path,
                "polling_interval": format!("{polling}s")
            },
            "parsing": {
                "rules": self.fm_rules,
                "case_sensitive": false
            },
            "server": {
                "port": 0,
                "device_number": 0,
                "discovery_port": null
            }
        })
    }

    /// Build sentinel JSON config pointing at the given filemonitor port.
    pub fn build_sentinel_config(&self) -> serde_json::Value {
        let polling = if self.sentinel_polling_interval > 0 {
            self.sentinel_polling_interval
        } else {
            1
        };

        let mut config = serde_json::json!({
            "monitors": self.sentinel_monitors,
            "notifiers": [],
            "transitions": self.sentinel_transitions,
            "dashboard": {
                "enabled": true,
                "port": 0,
                "history_size": 100
            }
        });

        if self.sentinel_has_notifiers {
            let mut pushover = serde_json::json!({
                "type": "pushover",
                "api_token": "test-token",
                "user_key": "test-user"
            });
            // Point the notifier at the local stub instead of api.pushover.net.
            if let Some(url) = &self.pushover_stub_url {
                pushover["api_url"] = serde_json::json!(url);
            }
            config["notifiers"] = serde_json::json!([pushover]);
        }

        // Set polling interval on all monitors
        if let Some(monitors) = config["monitors"].as_array_mut() {
            for m in monitors.iter_mut() {
                m["polling_interval"] = serde_json::json!(format!("{polling}s"));
            }
        }

        config
    }

    /// Start filemonitor binary with the accumulated config.
    pub async fn start_filemonitor(&mut self) {
        let config_json = self.build_filemonitor_config();
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("filemonitor_config.json");
        std::fs::write(&config_path, config_json.to_string())
            .expect("failed to write filemonitor config");

        let handle = ServiceHandle::start("filemonitor", config_path.to_str().unwrap()).await;

        self.filemonitor = Some(handle);
    }

    /// Add a monitor config entry pointing at the running filemonitor.
    pub fn add_filemonitor_monitor(&mut self, name: &str) {
        let fm = self.filemonitor.as_ref().expect("filemonitor not started");
        self.sentinel_monitor_name = name.to_string();
        self.sentinel_monitors.push(serde_json::json!({
            "type": "alpaca_safety_monitor",
            "name": name,
            "host": "127.0.0.1",
            "port": fm.port,
            "device_number": 0,
            "polling_interval": "1s"
        }));
    }

    /// Start a local stub that mimics the Pushover API, replying 200 to any
    /// request. Lets notification scenarios exercise the dispatch path without
    /// the real api.pushover.net round-trip (slow, network-dependent, and the
    /// source of the flaky "history is empty" race the fixed sleep used to mask).
    pub async fn start_pushover_stub(&mut self) {
        use axum::Router;

        let app = Router::new().fallback(|| async {
            axum::Json(serde_json::json!({ "status": 1, "request": "stub" }))
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind pushover stub listener");
        let addr = listener
            .local_addr()
            .expect("pushover stub listener has no local addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        self.pushover_stub = Some(handle);
        self.pushover_stub_url = Some(format!("http://{addr}/1/messages.json"));
    }

    /// Start sentinel binary with the accumulated config.
    pub async fn start_sentinel(&mut self) {
        // Stand up the Pushover stub before sentinel so its URL can be baked
        // into the config the child process loads.
        if self.sentinel_has_notifiers && self.pushover_stub_url.is_none() {
            self.start_pushover_stub().await;
        }
        let config_json = self.build_sentinel_config();
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("sentinel_config.json");
        std::fs::write(&config_path, config_json.to_string())
            .expect("failed to write sentinel config");

        let handle =
            ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;

        self.sentinel = Some(handle);
    }

    /// Try to start sentinel, capturing errors instead of panicking.
    pub async fn try_start_sentinel(&mut self, config_path: &str) {
        match ServiceHandle::try_start(env!("CARGO_PKG_NAME"), config_path).await {
            Ok(handle) => {
                self.sentinel = Some(handle);
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(e);
            }
        }
    }

    /// Build the dashboard URL for a given path.
    pub fn dashboard_url(&self, path: &str) -> String {
        let sentinel = self.sentinel.as_ref().expect("sentinel not started");
        format!("{}{}", sentinel.base_url, path)
    }

    /// Wait until sentinel has polled at least once (last_poll_epoch_ms > 0).
    pub async fn wait_for_poll(&self) {
        let client = reqwest::Client::new();
        let url = self.dashboard_url("/api/status");

        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(resp) = client.get(&url).send().await {
                if let Ok(json) = resp.json::<Vec<serde_json::Value>>().await {
                    if json.iter().any(|m| {
                        m.get("last_poll_epoch_ms")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0)
                            > 0
                    }) {
                        return;
                    }
                }
            }
        }
        panic!("sentinel did not poll within 30 seconds");
    }

    /// Poll `/api/status` until `name` reports `expected_state`, or ~15s elapses.
    /// Returns the last observed state for that monitor (so a timeout produces a
    /// useful assertion message). Replaces a blind fixed sleep, removing the race
    /// against filemonitor + sentinel polling latency.
    pub async fn wait_for_status(&self, name: &str, expected_state: &str) -> Option<String> {
        let mut last = None;
        for _ in 0..60 {
            for monitor in self.get_status().await {
                if monitor["name"].as_str() == Some(name) {
                    let state = monitor["state"].as_str().map(str::to_string);
                    if state.as_deref() == Some(expected_state) {
                        return state;
                    }
                    last = state;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        last
    }

    /// Poll `/api/history` until a record matches `predicate`, or ~15s elapses.
    /// Returns the final history snapshot regardless, so the caller can assert and
    /// print it on failure. Waits for the notification record to actually land
    /// rather than assuming a fixed delay is enough.
    pub async fn wait_for_history<F>(&self, predicate: F) -> Vec<serde_json::Value>
    where
        F: Fn(&serde_json::Value) -> bool,
    {
        let mut history = self.get_history().await;
        for _ in 0..60 {
            if history.iter().any(&predicate) {
                return history;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            history = self.get_history().await;
        }
        history
    }

    /// GET a dashboard endpoint and return the response body as string.
    pub async fn http_get(&mut self, path: &str) {
        let client = reqwest::Client::new();
        let url = self.dashboard_url(path);
        match client.get(&url).send().await {
            Ok(resp) => {
                self.last_status_code = Some(resp.status().as_u16());
                self.last_response_body = Some(resp.text().await.unwrap_or_default());
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
            }
        }
    }

    /// GET /api/status and parse as JSON array.
    pub async fn get_status(&self) -> Vec<serde_json::Value> {
        let client = reqwest::Client::new();
        let url = self.dashboard_url("/api/status");
        let resp = client
            .get(&url)
            .send()
            .await
            .expect("failed to GET /api/status");
        resp.json().await.expect("failed to parse status JSON")
    }

    /// GET /api/history and parse as JSON array.
    pub async fn get_history(&self) -> Vec<serde_json::Value> {
        let client = reqwest::Client::new();
        let url = self.dashboard_url("/api/history");
        let resp = client
            .get(&url)
            .send()
            .await
            .expect("failed to GET /api/history");
        resp.json().await.expect("failed to parse history JSON")
    }
}
