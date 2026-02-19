//! BDD step definitions for monitoring feature

use std::sync::Arc;

use cucumber::{given, then, when};

use sentinel::alpaca_client::AlpacaSafetyMonitor;
use sentinel::config::MonitorConfig;
use sentinel::io::{HttpClient, HttpResponse};
use sentinel::monitor::MonitorState;
use sentinel::SentinelError;

use crate::world::SentinelWorld;

fn test_config() -> MonitorConfig {
    MonitorConfig::AlpacaSafetyMonitor {
        name: "Test Monitor".to_string(),
        host: "localhost".to_string(),
        port: 11111,
        device_number: 0,
        polling_interval_seconds: 30,
    }
}

/// A mock HTTP client that returns a fixed response for GET requests
struct FixedGetClient {
    response: Result<HttpResponse, String>,
}

#[async_trait::async_trait]
impl HttpClient for FixedGetClient {
    async fn get(&self, _url: &str) -> sentinel::Result<HttpResponse> {
        match &self.response {
            Ok(r) => Ok(r.clone()),
            Err(msg) => Err(SentinelError::Http(msg.clone())),
        }
    }

    async fn put_form(
        &self,
        _url: &str,
        _params: &[(&str, &str)],
    ) -> sentinel::Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: r#"{"ErrorNumber": 0, "ErrorMessage": ""}"#.to_string(),
        })
    }

    async fn post_form(
        &self,
        _url: &str,
        _params: &[(&str, &str)],
    ) -> sentinel::Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: "{}".to_string(),
        })
    }
}

#[given("a safety monitor that reports safe")]
fn monitor_reports_safe(world: &mut SentinelWorld) {
    let client = FixedGetClient {
        response: Ok(HttpResponse {
            status: 200,
            body: r#"{"Value": true, "ErrorNumber": 0, "ErrorMessage": ""}"#.to_string(),
        }),
    };
    let monitor = AlpacaSafetyMonitor::new(&test_config(), Arc::new(client));
    world.monitor = Some(Box::new(monitor));
}

#[given("a safety monitor that reports unsafe")]
fn monitor_reports_unsafe(world: &mut SentinelWorld) {
    let client = FixedGetClient {
        response: Ok(HttpResponse {
            status: 200,
            body: r#"{"Value": false, "ErrorNumber": 0, "ErrorMessage": ""}"#.to_string(),
        }),
    };
    let monitor = AlpacaSafetyMonitor::new(&test_config(), Arc::new(client));
    world.monitor = Some(Box::new(monitor));
}

#[given("a safety monitor that is unreachable")]
fn monitor_unreachable(world: &mut SentinelWorld) {
    let client = FixedGetClient {
        response: Err("connection refused".to_string()),
    };
    let monitor = AlpacaSafetyMonitor::new(&test_config(), Arc::new(client));
    world.monitor = Some(Box::new(monitor));
}

#[given("a safety monitor that returns an ASCOM error")]
fn monitor_ascom_error(world: &mut SentinelWorld) {
    let client = FixedGetClient {
        response: Ok(HttpResponse {
            status: 200,
            body: r#"{"Value": false, "ErrorNumber": 1031, "ErrorMessage": "Not connected"}"#
                .to_string(),
        }),
    };
    let monitor = AlpacaSafetyMonitor::new(&test_config(), Arc::new(client));
    world.monitor = Some(Box::new(monitor));
}

#[given("a safety monitor that accepts connections")]
fn monitor_accepts_connections(world: &mut SentinelWorld) {
    let client = FixedGetClient {
        response: Ok(HttpResponse {
            status: 200,
            body: r#"{"Value": true, "ErrorNumber": 0, "ErrorMessage": ""}"#.to_string(),
        }),
    };
    let monitor = AlpacaSafetyMonitor::new(&test_config(), Arc::new(client));
    world.monitor = Some(Box::new(monitor));
}

#[when("the monitor is polled")]
async fn poll_monitor(world: &mut SentinelWorld) {
    let monitor = world.monitor.as_ref().expect("monitor not set");
    world.last_state = Some(monitor.poll().await);
}

#[when("the monitor connects")]
async fn connect_monitor(world: &mut SentinelWorld) {
    let monitor = world.monitor.as_ref().expect("monitor not set");
    world.last_result = Some(monitor.connect().await.map(|_| "ok".to_string()));
}

#[when("the monitor disconnects")]
async fn disconnect_monitor(world: &mut SentinelWorld) {
    let monitor = world.monitor.as_ref().expect("monitor not set");
    world.last_result = Some(monitor.disconnect().await.map(|_| "ok".to_string()));
}

#[then(expr = "the state should be {string}")]
fn state_should_be(world: &mut SentinelWorld, expected: String) {
    let state = world.last_state.expect("no state from poll");
    let expected_state = match expected.as_str() {
        "Safe" => MonitorState::Safe,
        "Unsafe" => MonitorState::Unsafe,
        "Unknown" => MonitorState::Unknown,
        other => panic!("Unknown state: {}", other),
    };
    assert_eq!(state, expected_state);
}

#[then("the connection should succeed")]
fn connection_succeeds(world: &mut SentinelWorld) {
    let result = world.last_result.as_ref().expect("no result");
    result.as_ref().unwrap();
}

#[then("the disconnection should succeed")]
fn disconnection_succeeds(world: &mut SentinelWorld) {
    let result = world.last_result.as_ref().expect("no result");
    result.as_ref().unwrap();
}
