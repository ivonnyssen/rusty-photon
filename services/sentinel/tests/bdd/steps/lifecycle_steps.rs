//! BDD step definitions for sentinel builder and lifecycle feature

use std::sync::Arc;
use std::time::Duration;

use cucumber::{given, then, when};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use sentinel::config::{Config, DashboardConfig, MonitorConfig};
use sentinel::io::{HttpClient, HttpResponse};
use sentinel::monitor::{Monitor, MonitorState};
use sentinel::notifier::{Notification, Notifier};
use sentinel::SentinelBuilder;

use crate::world::SentinelWorld;

// --- Test doubles ---

/// A recorded HTTP request
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub url: String,
    pub params: Vec<(String, String)>,
}

/// An HTTP client that records all requests and returns canned OK responses
#[derive(Debug, Default)]
pub struct RecordingHttpClient {
    pub requests: Arc<RwLock<Vec<RecordedRequest>>>,
}

#[async_trait::async_trait]
impl HttpClient for RecordingHttpClient {
    async fn get(&self, url: &str) -> sentinel::Result<HttpResponse> {
        self.requests.write().await.push(RecordedRequest {
            method: "GET".to_string(),
            url: url.to_string(),
            params: vec![],
        });
        Ok(HttpResponse {
            status: 200,
            body: r#"{"Value": true, "ErrorNumber": 0, "ErrorMessage": ""}"#.to_string(),
        })
    }

    async fn put_form(&self, url: &str, params: &[(&str, &str)]) -> sentinel::Result<HttpResponse> {
        self.requests.write().await.push(RecordedRequest {
            method: "PUT".to_string(),
            url: url.to_string(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });
        Ok(HttpResponse {
            status: 200,
            body: r#"{"ErrorNumber": 0, "ErrorMessage": ""}"#.to_string(),
        })
    }

    async fn post_form(
        &self,
        url: &str,
        params: &[(&str, &str)],
    ) -> sentinel::Result<HttpResponse> {
        self.requests.write().await.push(RecordedRequest {
            method: "POST".to_string(),
            url: url.to_string(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });
        Ok(HttpResponse {
            status: 200,
            body: "{}".to_string(),
        })
    }
}

/// A minimal Monitor implementation for injection tests
#[derive(Debug)]
struct StubMonitor {
    monitor_name: String,
}

#[async_trait::async_trait]
impl Monitor for StubMonitor {
    fn name(&self) -> &str {
        &self.monitor_name
    }

    async fn poll(&self) -> MonitorState {
        MonitorState::Safe
    }

    async fn connect(&self) -> sentinel::Result<()> {
        Ok(())
    }

    async fn disconnect(&self) -> sentinel::Result<()> {
        Ok(())
    }

    fn polling_interval(&self) -> Duration {
        Duration::from_secs(30)
    }
}

/// A minimal Notifier implementation for injection tests
#[derive(Debug)]
struct StubNotifier {
    notifier_type: String,
}

#[async_trait::async_trait]
impl Notifier for StubNotifier {
    fn type_name(&self) -> &str {
        &self.notifier_type
    }

    async fn notify(&self, _notification: &Notification) -> sentinel::Result<()> {
        Ok(())
    }
}

// --- Helper ---

fn expected_connect_url(config: &MonitorConfig) -> String {
    match config {
        MonitorConfig::AlpacaSafetyMonitor {
            host,
            port,
            device_number,
            ..
        } => format!(
            "http://{}:{}/api/v1/safetymonitor/{}/connected",
            host, port, device_number
        ),
    }
}

fn build_sentinel_builder(world: &mut SentinelWorld) -> SentinelBuilder {
    let dashboard = if let Some(port) = world.lifecycle_dashboard_port {
        DashboardConfig {
            enabled: true,
            port,
            ..DashboardConfig::default()
        }
    } else {
        DashboardConfig {
            enabled: false,
            ..DashboardConfig::default()
        }
    };

    let config = Config {
        monitors: world.lifecycle_monitor_configs.clone(),
        dashboard,
        ..Config::default()
    };

    let http = world
        .lifecycle_http
        .get_or_insert_with(|| Arc::new(RecordingHttpClient::default()))
        .clone();

    let mut builder = SentinelBuilder::new(config).with_http_client(http as Arc<dyn HttpClient>);

    if let Some(monitors) = world.lifecycle_injected_monitors.take() {
        builder = builder.with_monitors(monitors);
    }
    if let Some(notifiers) = world.lifecycle_injected_notifiers.take() {
        builder = builder.with_notifiers(notifiers);
    }
    if let Some(cancel) = world.lifecycle_cancel.take() {
        builder = builder.with_cancellation_token(cancel);
    }

    builder
}

// --- Given steps ---

#[given("an empty sentinel config")]
fn empty_sentinel_config(world: &mut SentinelWorld) {
    world.lifecycle_monitor_configs = Vec::new();
}

#[given(expr = "a sentinel config with monitor {string} at {word}:{int} device {int}")]
fn sentinel_config_with_monitor(
    world: &mut SentinelWorld,
    name: String,
    host: String,
    port: i32,
    device_number: i32,
) {
    world
        .lifecycle_monitor_configs
        .push(MonitorConfig::AlpacaSafetyMonitor {
            name,
            host,
            port: port as u16,
            device_number: device_number as u32,
            polling_interval_seconds: 30,
        });
}

#[given("a pre-cancelled cancellation token")]
fn pre_cancelled_token(world: &mut SentinelWorld) {
    let token = CancellationToken::new();
    token.cancel();
    world.lifecycle_cancel = Some(token);
}

#[given(expr = "an injected monitor named {string}")]
fn injected_monitor(world: &mut SentinelWorld, name: String) {
    let stub: Arc<dyn Monitor> = Arc::new(StubMonitor { monitor_name: name });
    world
        .lifecycle_injected_monitors
        .get_or_insert_with(Vec::new)
        .push(stub);
}

#[given(expr = "an injected notifier of type {string}")]
fn injected_notifier(world: &mut SentinelWorld, notifier_type: String) {
    let stub: Arc<dyn Notifier> = Arc::new(StubNotifier { notifier_type });
    world
        .lifecycle_injected_notifiers
        .get_or_insert_with(Vec::new)
        .push(stub);
}

#[given("an empty sentinel config with dashboard enabled on a free port")]
async fn empty_config_with_dashboard(world: &mut SentinelWorld) {
    world.lifecycle_monitor_configs = Vec::new();
    // Bind to port 0 to get a free port, then drop the listener so the builder can use it
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    world.lifecycle_dashboard_port = Some(port);
}

#[given("the dashboard port is already in use")]
async fn dashboard_port_already_in_use(world: &mut SentinelWorld) {
    let port = world
        .lifecycle_dashboard_port
        .expect("dashboard port must be set first");
    let blocker = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();
    world.lifecycle_port_blocker = Some(blocker);
}

// --- When steps ---

#[when("the sentinel is built")]
async fn sentinel_is_built(world: &mut SentinelWorld) {
    let builder = build_sentinel_builder(world);
    match builder.build().await {
        Ok(sentinel) => {
            world.lifecycle_dashboard_bound = Some(sentinel.has_dashboard());
            world.lifecycle_build_succeeded = Some(true);
        }
        Err(_) => {
            world.lifecycle_build_succeeded = Some(false);
        }
    }
}

#[when("the sentinel is built and started")]
async fn sentinel_is_built_and_started(world: &mut SentinelWorld) {
    let builder = build_sentinel_builder(world);
    match builder.build().await {
        Ok(sentinel) => {
            world.lifecycle_build_succeeded = Some(true);
            world.lifecycle_start_succeeded = Some(sentinel.start().await.is_ok());
        }
        Err(_) => {
            world.lifecycle_build_succeeded = Some(false);
            world.lifecycle_start_succeeded = Some(false);
        }
    }
}

#[when("the sentinel is built and started with dashboard")]
async fn sentinel_is_built_and_started_with_dashboard(world: &mut SentinelWorld) {
    // Use a fresh cancellation token so the dashboard stays alive long enough to serve requests
    let cancel = CancellationToken::new();
    world.lifecycle_cancel = Some(cancel.clone());

    let builder = build_sentinel_builder(world);
    match builder.build().await {
        Ok(sentinel) => {
            world.lifecycle_build_succeeded = Some(true);
            world.lifecycle_dashboard_bound = Some(sentinel.has_dashboard());

            // Spawn start() in background so we can hit the dashboard while it's running
            let handle = tokio::spawn(async move { sentinel.start().await });

            // Wait for the dashboard to become ready (retry a few times)
            let port = world
                .lifecycle_dashboard_port
                .expect("dashboard port must be set");
            let url = format!("http://127.0.0.1:{}/health", port);
            let client = reqwest::Client::new();
            let mut health_ok = false;
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if let Ok(resp) = client.get(&url).send().await {
                    if resp.status().as_u16() == 200 {
                        health_ok = true;
                        break;
                    }
                }
            }
            world.lifecycle_dashboard_health_ok = Some(health_ok);

            // Cancel to trigger graceful shutdown
            cancel.cancel();
            let result = handle.await.expect("start task should not panic");
            world.lifecycle_start_succeeded = Some(result.is_ok());
        }
        Err(_) => {
            world.lifecycle_build_succeeded = Some(false);
            world.lifecycle_start_succeeded = Some(false);
        }
    }
}

// --- Then steps ---

#[then("the build should succeed")]
fn build_should_succeed(world: &mut SentinelWorld) {
    assert_eq!(
        world.lifecycle_build_succeeded,
        Some(true),
        "Expected build to succeed"
    );
}

#[then("the lifecycle should complete successfully")]
fn lifecycle_should_complete(world: &mut SentinelWorld) {
    assert_eq!(
        world.lifecycle_build_succeeded,
        Some(true),
        "Expected build to succeed"
    );
    assert_eq!(
        world.lifecycle_start_succeeded,
        Some(true),
        "Expected start to succeed"
    );
}

#[then(expr = "monitor {string} should have been connected")]
async fn monitor_should_have_been_connected(world: &mut SentinelWorld, name: String) {
    let http = world
        .lifecycle_http
        .as_ref()
        .expect("no recording HTTP client");
    let requests = http.requests.read().await;

    let monitor_config = world
        .lifecycle_monitor_configs
        .iter()
        .find(|m| m.name() == name)
        .unwrap_or_else(|| panic!("no monitor config with name '{}'", name));

    let url = expected_connect_url(monitor_config);

    let found = requests.iter().any(|r| {
        r.method == "PUT"
            && r.url == url
            && r.params
                .iter()
                .any(|(k, v)| k == "Connected" && v == "true")
    });

    assert!(
        found,
        "Expected monitor '{}' to have been connected via PUT to {}",
        name, url
    );
}

#[then(expr = "monitor {string} should have been disconnected")]
async fn monitor_should_have_been_disconnected(world: &mut SentinelWorld, name: String) {
    let http = world
        .lifecycle_http
        .as_ref()
        .expect("no recording HTTP client");
    let requests = http.requests.read().await;

    let monitor_config = world
        .lifecycle_monitor_configs
        .iter()
        .find(|m| m.name() == name)
        .unwrap_or_else(|| panic!("no monitor config with name '{}'", name));

    let url = expected_connect_url(monitor_config);

    let found = requests.iter().any(|r| {
        r.method == "PUT"
            && r.url == url
            && r.params
                .iter()
                .any(|(k, v)| k == "Connected" && v == "false")
    });

    assert!(
        found,
        "Expected monitor '{}' to have been disconnected via PUT to {}",
        name, url
    );
}

#[then(expr = "monitor {string} should have connected to {string}")]
async fn monitor_should_have_connected_to_url(
    world: &mut SentinelWorld,
    _name: String,
    expected_url: String,
) {
    let http = world
        .lifecycle_http
        .as_ref()
        .expect("no recording HTTP client");
    let requests = http.requests.read().await;

    let found = requests.iter().any(|r| {
        r.method == "PUT"
            && r.url == expected_url
            && r.params
                .iter()
                .any(|(k, v)| k == "Connected" && v == "true")
    });

    assert!(
        found,
        "Expected a PUT connect request to {}, but recorded requests: {:?}",
        expected_url, *requests
    );
}

#[then("the dashboard should be bound")]
fn dashboard_should_be_bound(world: &mut SentinelWorld) {
    assert_eq!(
        world.lifecycle_dashboard_bound,
        Some(true),
        "Expected dashboard to be bound"
    );
}

#[then("the dashboard should not be bound")]
fn dashboard_should_not_be_bound(world: &mut SentinelWorld) {
    assert_eq!(
        world.lifecycle_dashboard_bound,
        Some(false),
        "Expected dashboard not to be bound"
    );
}

#[then("the dashboard health endpoint should return OK")]
fn dashboard_health_should_return_ok(world: &mut SentinelWorld) {
    assert_eq!(
        world.lifecycle_dashboard_health_ok,
        Some(true),
        "Expected dashboard /health to return 200"
    );
}
