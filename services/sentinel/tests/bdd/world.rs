//! BDD test world for sentinel service (binary-spawning pattern)

use std::path::PathBuf;
use std::time::Duration;

use bdd_infra::ServiceHandle;
use cucumber::World;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

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

    // Operation watchdog: a controllable stub standing in for rp's SSE
    // stream, plus the URL sentinel's watchdog should subscribe to.
    pub rp_event_stub: Option<RpEventStub>,
    pub watchdog_rp_url: Option<String>,

    // Corrective ladder: a stub Alpaca service the watchdog can health-check
    // and abort, plus the Alpaca base URL baked into the watchdog `services`
    // config. Present only for the abort scenario.
    pub mount_stub: Option<MountServiceStub>,
    pub mount_service_url: Option<String>,

    // Service restart API: entries for the top-level `services` map, plus the
    // marker file the scripted restart command writes (proof it ran).
    pub supervised_services: serde_json::Map<String, serde_json::Value>,
    pub restart_marker: Option<PathBuf>,

    // Service health supervision: a stub HTTP service whose /health answer is
    // flippable between 200 and 503 at runtime.
    pub health_stub: Option<FlippableHealthStub>,
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
                "history_size": 100
            },
            "server": {
                "port": 0
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

        // The top-level supervised-services map (the restart endpoint's and the
        // corrective ladder's shared registry).
        if !self.supervised_services.is_empty() {
            config["services"] = serde_json::Value::Object(self.supervised_services.clone());
        }

        // Wire the operation watchdog when a watched rp URL is set. Buffers are
        // zeroed so the tracked deadline equals the operation's
        // `max_duration_ms` exactly, keeping the BDD fast and deterministic;
        // reconnect is tight so the "unresponsive" path resolves quickly.
        if let Some(rp_url) = &self.watchdog_rp_url {
            let mut watchdog = serde_json::json!({
                "rp_url": rp_url,
                "reconnect_max_attempts": 2,
                "reconnect_backoff": "1s",
                "default_buffer": "0s",
                "notifiers": ["pushover"],
                "operations": { "slew": { "buffer": "0s" } }
            });
            // When a corrective service stub is configured, make `slew` run the
            // abort ladder against it (responsive service + abort verb => the
            // ladder stops at a clean abort, so `restart_command` stays null and
            // the BDD never shells out). The service lives in the top-level
            // `services` map (shared with the restart endpoint) — inserted, not
            // assigned, so `supervised_services` entries added by restart-API
            // steps survive; its restart budget is per-service, kept tight so a
            // regression that reaches the restart rung fails fast.
            if let Some(svc_url) = &self.mount_service_url {
                watchdog["operations"]["slew"]["on_expiry"] =
                    serde_json::json!("abort_then_restart");
                watchdog["operations"]["slew"]["service"] = serde_json::json!("mount");
                config["services"]["mount"] = serde_json::json!({
                    "base_url": svc_url,
                    "restart_command": null,
                    "max_restart_duration": "2s"
                });
            }
            config["operation_watchdog"] = watchdog;
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

    /// Absolute path of the marker file the scripted restart command writes —
    /// recorded so the Then-step can assert the command actually ran.
    pub fn restart_marker_path(&mut self) -> PathBuf {
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let path = dir.path().join("restart-marker.txt");
        self.restart_marker = Some(path.clone());
        path
    }

    /// Add an entry to the top-level `services` map (the restart endpoint's
    /// registry). `restart_command: None` serializes as `null` (not restartable).
    pub fn add_supervised_service(
        &mut self,
        name: &str,
        restart_command: Option<String>,
        health_command: Option<String>,
        max_restart_duration: Option<&str>,
    ) {
        let mut entry = serde_json::json!({ "restart_command": restart_command });
        if let Some(health) = health_command {
            entry["health_command"] = serde_json::json!(health);
        }
        if let Some(budget) = max_restart_duration {
            entry["max_restart_duration"] = serde_json::json!(budget);
        }
        self.supervised_services.insert(name.to_string(), entry);
    }

    /// POST a dashboard endpoint (empty body) and capture status + body.
    pub async fn http_post(&mut self, path: &str) {
        let client = reqwest::Client::new();
        let url = self.dashboard_url(path);
        match client.post(&url).send().await {
            Ok(resp) => {
                self.last_status_code = Some(resp.status().as_u16());
                self.last_response_body = Some(resp.text().await.unwrap_or_default());
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
            }
        }
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

    /// Start the controllable rp SSE stub with the given pre-formatted SSE
    /// frames and point the watchdog at it.
    pub async fn start_rp_event_stub(&mut self, frames: Vec<String>) {
        let stub = RpEventStub::start(frames).await;
        self.watchdog_rp_url = Some(stub.base_url().to_string());
        self.rp_event_stub = Some(stub);
    }

    /// Start a stub Alpaca telescope service the corrective ladder can probe
    /// (reports connected) and abort (records the call), and wire the watchdog
    /// `services` config at its URL.
    pub async fn start_mount_service_stub(&mut self) {
        let stub = MountServiceStub::start().await;
        self.mount_service_url = Some(format!("{}/api/v1", stub.base_url()));
        self.mount_stub = Some(stub);
    }

    /// Start the flippable health stub answering 200 (healthy) or 503.
    pub async fn start_health_stub(&mut self, healthy: bool) {
        self.health_stub = Some(FlippableHealthStub::start(healthy).await);
    }

    /// Add a `services` entry supervised by health polling at the running
    /// health stub. Fast timings (200ms cadence, threshold 2, 1s..2s backoff)
    /// keep the scenarios well under their wait ceilings.
    pub fn add_health_supervised_service(&mut self, name: &str, restart_command: Option<String>) {
        let stub = self.health_stub.as_ref().expect("health stub not started");
        self.supervised_services.insert(
            name.to_string(),
            serde_json::json!({
                "restart_command": restart_command,
                "max_restart_duration": "2s",
                "health": {
                    "url": stub.health_url(),
                    "poll_interval": "200ms",
                    "failure_threshold": 2,
                    "restart_backoff": "1s",
                    "restart_backoff_max": "2s"
                }
            }),
        );
    }

    /// GET /api/services and parse as JSON array.
    pub async fn get_services(&self) -> Vec<serde_json::Value> {
        let client = reqwest::Client::new();
        let url = self.dashboard_url("/api/services");
        let resp = client
            .get(&url)
            .send()
            .await
            .expect("failed to GET /api/services");
        resp.json().await.expect("failed to parse services JSON")
    }

    /// Poll `/api/services` until `name` reports `expected` health, or ~15s
    /// elapses. Returns the last observed health for the assertion message.
    pub async fn wait_for_service_health(&self, name: &str, expected: &str) -> Option<String> {
        let mut last = None;
        for _ in 0..60 {
            for service in self.get_services().await {
                if service["name"].as_str() == Some(name) {
                    let health = service["health"].as_str().map(str::to_string);
                    if health.as_deref() == Some(expected) {
                        return health;
                    }
                    last = health;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        last
    }

    /// Poll the restart marker file until it holds at least `min` lines (one
    /// per restart-command run) or `ceiling` elapses. Returns the final count.
    pub async fn wait_for_marker_lines(&self, min: usize, ceiling: Duration) -> usize {
        let marker = self.restart_marker.as_ref().expect("no marker path");
        let deadline = tokio::time::Instant::now() + ceiling;
        loop {
            let count = std::fs::read_to_string(marker)
                .map(|c| c.lines().count())
                .unwrap_or(0);
            if count >= min || tokio::time::Instant::now() >= deadline {
                return count;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// A stub HTTP service whose `GET /health` answer flips between 200 (healthy)
/// and 503 at runtime — the supervised target for the health-supervision BDD.
#[derive(Debug)]
pub struct FlippableHealthStub {
    base_url: String,
    healthy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl FlippableHealthStub {
    pub async fn start(initially_healthy: bool) -> Self {
        use axum::http::StatusCode;
        use axum::routing::get;
        use axum::{Json, Router};
        use std::sync::atomic::Ordering;

        let healthy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(initially_healthy));
        let flag = std::sync::Arc::clone(&healthy);
        let app = Router::new().route(
            "/health",
            get(move || {
                let flag = std::sync::Arc::clone(&flag);
                async move {
                    if flag.load(Ordering::SeqCst) {
                        (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
                    } else {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(serde_json::json!({ "status": "error" })),
                        )
                    }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind health stub");
        let addr = listener.local_addr().expect("health stub has no addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self {
            base_url: format!("http://{addr}"),
            healthy,
            handle,
        }
    }

    pub fn health_url(&self) -> String {
        format!("{}/health", self.base_url)
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.healthy
            .store(healthy, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for FlippableHealthStub {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// A stub Alpaca telescope service for the corrective-ladder BDD: it answers
/// `GET .../telescope/0/connected` as responsive and records every
/// `PUT .../telescope/0/abortslew` so the test can assert the watchdog aborted
/// the right device.
#[derive(Debug)]
pub struct MountServiceStub {
    base_url: String,
    abort_count: std::sync::Arc<std::sync::atomic::AtomicU32>,
    handle: tokio::task::JoinHandle<()>,
}

impl MountServiceStub {
    pub async fn start() -> Self {
        use axum::routing::{get, put};
        use axum::{Json, Router};
        use std::sync::atomic::Ordering;

        let abort_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count = std::sync::Arc::clone(&abort_count);
        let app = Router::new()
            .route(
                "/api/v1/telescope/0/connected",
                get(|| async {
                    Json(serde_json::json!({
                        "Value": true, "ErrorNumber": 0, "ErrorMessage": ""
                    }))
                }),
            )
            .route(
                "/api/v1/telescope/0/abortslew",
                put(move || {
                    let count = std::sync::Arc::clone(&count);
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!({ "ErrorNumber": 0, "ErrorMessage": "" }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind mount service stub");
        let addr = listener
            .local_addr()
            .expect("mount service stub has no addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self {
            base_url: format!("http://{addr}"),
            abort_count,
            handle,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Number of `abortslew` calls received so far.
    pub fn abort_count(&self) -> u32 {
        self.abort_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for MountServiceStub {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// A minimal, controllable Server-Sent-Events server standing in for rp's
/// `GET /api/events/subscribe`. Every accepted connection receives the
/// pre-scripted frames (as one chunked-transfer body) and is then held open —
/// no disconnect — so the sentinel watchdog tracks exactly the operations the
/// script describes. Built on raw tokio TCP so it pulls in no new dependency.
#[derive(Debug)]
pub struct RpEventStub {
    base_url: String,
    cancel: CancellationToken,
}

impl RpEventStub {
    /// Bind on an ephemeral loopback port and serve `frames` (each an SSE
    /// block without its trailing blank line) to every connection.
    pub async fn start(frames: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind rp event stub");
        let addr = listener.local_addr().expect("rp event stub has no addr");
        let cancel = CancellationToken::new();
        let body: String = frames.iter().map(|f| format!("{f}\n\n")).collect();
        let server_cancel = cancel.clone();

        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = server_cancel.cancelled() => break,
                    res = listener.accept() => res,
                };
                let Ok((mut sock, _)) = accepted else { break };
                let body = body.clone();
                let conn_cancel = server_cancel.clone();
                tokio::spawn(async move {
                    // Drain the request so the client's write completes.
                    let mut buf = [0u8; 2048];
                    let _ = sock.read(&mut buf).await;
                    let chunk = format!("{:x}\r\n{}\r\n", body.len(), body);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: text/event-stream\r\n\
                         Cache-Control: no-cache\r\n\
                         Transfer-Encoding: chunked\r\n\r\n{chunk}"
                    );
                    if sock.write_all(response.as_bytes()).await.is_err() {
                        return;
                    }
                    let _ = sock.flush().await;
                    // Hold the connection open until the stub is shut down.
                    conn_cancel.cancelled().await;
                });
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            cancel,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for RpEventStub {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
