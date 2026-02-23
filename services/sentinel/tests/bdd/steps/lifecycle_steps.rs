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
    let config = Config {
        monitors: world.lifecycle_monitor_configs.clone(),
        dashboard: DashboardConfig {
            enabled: false,
            ..DashboardConfig::default()
        },
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

// --- When steps ---

#[when("the sentinel is built")]
async fn sentinel_is_built(world: &mut SentinelWorld) {
    let builder = build_sentinel_builder(world);
    world.lifecycle_build_succeeded = Some(builder.build().await.is_ok());
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
