//! Step definitions for server_registration.feature
//!
//! Requires the `mock` feature flag. Tests run sequentially because the ASCOM
//! Alpaca discovery service binds to a fixed address.

use std::sync::Arc;

use crate::world::PpbaWorld;
use async_trait::async_trait;
use cucumber::{given, then, when};
use ppba_driver::config::{ObservingConditionsConfig, SerialConfig, ServerConfig, SwitchConfig};
use ppba_driver::io::{SerialPair, SerialPortFactory};
use ppba_driver::{Config, Result};
use std::time::Duration;

/// Minimal serial port factory for server startup tests.
struct StubSerialPortFactory;

#[async_trait]
impl SerialPortFactory for StubSerialPortFactory {
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        unreachable!("open() should not be called during server startup tests")
    }

    async fn port_exists(&self, _port: &str) -> bool {
        true
    }
}

fn test_config(switch_enabled: bool, oc_enabled: bool) -> Config {
    Config {
        serial: SerialConfig {
            port: "/dev/mock".to_string(),
            polling_interval_ms: 60000,
            ..Default::default()
        },
        server: ServerConfig { port: 0 },
        switch: SwitchConfig {
            enabled: switch_enabled,
            ..Default::default()
        },
        observingconditions: ObservingConditionsConfig {
            enabled: oc_enabled,
            ..Default::default()
        },
    }
}

async fn spawn_server(config: Config) -> (u16, tokio::task::JoinHandle<()>) {
    let factory: Arc<dyn SerialPortFactory> = Arc::new(StubSerialPortFactory);

    let bound = ppba_driver::ServerBuilder::new(config)
        .with_factory(factory)
        .build()
        .await
        .expect("Server failed to bind");

    let port = bound.listen_addr().port();

    let handle = tokio::spawn(async move {
        let _ = bound.start().await;
    });

    (port, handle)
}

async fn get_status(port: u16, path: &str) -> u16 {
    let url = format!("http://127.0.0.1:{}{}", port, path);
    reqwest::get(&url).await.unwrap().status().as_u16()
}

async fn get_json(port: u16, path: &str) -> serde_json::Value {
    let url = format!("http://127.0.0.1:{}{}", port, path);
    let text = reqwest::get(&url).await.unwrap().text().await.unwrap();
    serde_json::from_str(&text).unwrap()
}

// ============================================================================
// Given steps
// ============================================================================

#[given("a server config with switch enabled and OC enabled")]
fn server_config_both_enabled(world: &mut PpbaWorld) {
    world.config = Some(test_config(true, true));
}

#[given("a server config with switch enabled and OC disabled")]
fn server_config_switch_only(world: &mut PpbaWorld) {
    world.config = Some(test_config(true, false));
}

#[given("a server config with switch disabled and OC enabled")]
fn server_config_oc_only(world: &mut PpbaWorld) {
    world.config = Some(test_config(false, true));
}

#[given("a server config with switch disabled and OC disabled")]
fn server_config_none(world: &mut PpbaWorld) {
    world.config = Some(test_config(false, false));
}

#[given(expr = "a server config with switch name {string}")]
fn server_config_with_switch_name(world: &mut PpbaWorld, name: String) {
    let mut config = test_config(true, false);
    config.switch.name = name;
    world.config = Some(config);
}

#[given(expr = "a server config with OC name {string}")]
fn server_config_with_oc_name(world: &mut PpbaWorld, name: String) {
    let mut config = test_config(false, true);
    config.observingconditions.name = name;
    world.config = Some(config);
}

// ============================================================================
// When steps
// ============================================================================

#[when("I start the server")]
async fn start_server(world: &mut PpbaWorld) {
    let config = world.config.take().expect("config not set");
    let (port, handle) = spawn_server(config).await;
    world.server_port = Some(port);
    world.server_handle = Some(handle);
}

// ============================================================================
// Then steps
// ============================================================================

#[then("the switch endpoint should respond with 200")]
async fn switch_endpoint_responds_200(world: &mut PpbaWorld) {
    let port = world.server_port.expect("server not started");
    let status = get_status(port, "/api/v1/switch/0/name").await;
    assert_eq!(status, 200, "Switch name endpoint should respond with 200");
}

#[then("the switch endpoint should not respond with 200")]
async fn switch_endpoint_not_200(world: &mut PpbaWorld) {
    let port = world.server_port.expect("server not started");
    let status = get_status(port, "/api/v1/switch/0/name").await;
    assert_ne!(status, 200, "Switch should not be registered");
}

#[then("the OC endpoint should respond with 200")]
async fn oc_endpoint_responds_200(world: &mut PpbaWorld) {
    let port = world.server_port.expect("server not started");
    let status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_eq!(status, 200, "OC name endpoint should respond with 200");
}

#[then("the OC endpoint should not respond with 200")]
async fn oc_endpoint_not_200(world: &mut PpbaWorld) {
    let port = world.server_port.expect("server not started");
    let status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_ne!(status, 200, "OC should not be registered");
}

#[then(expr = "the switch name endpoint should return {string}")]
async fn switch_name_endpoint_returns(world: &mut PpbaWorld, expected: String) {
    let port = world.server_port.expect("server not started");
    let body = get_json(port, "/api/v1/switch/0/name").await;
    assert_eq!(body["Value"], expected);
}

#[then(expr = "the OC name endpoint should return {string}")]
async fn oc_name_endpoint_returns(world: &mut PpbaWorld, expected: String) {
    let port = world.server_port.expect("server not started");
    let body = get_json(port, "/api/v1/observingconditions/0/name").await;
    assert_eq!(body["Value"], expected);
}

#[then("the server should be reachable on the bound port")]
async fn server_reachable_on_bound_port(world: &mut PpbaWorld) {
    let port = world.server_port.expect("server not started");
    assert_ne!(port, 0, "OS should have assigned a real port");
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(stream.is_ok(), "Server should be reachable on bound port");
}
