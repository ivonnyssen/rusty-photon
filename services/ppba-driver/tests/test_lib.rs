//! Tests for lib.rs - server startup and device registration
//!
//! Tests verify that `ServerBuilder` correctly registers devices based on
//! configuration flags and starts the ASCOM Alpaca server.
//!
//! Requires the `mock` feature. All tests are skipped under Miri since it
//! cannot call socket syscalls.
//!
//! Tests run sequentially because the ASCOM Alpaca discovery service binds
//! to a fixed address, so only one server can run at a time.
// The std::Mutex is intentional here: it serializes sequential test runs because
// the ASCOM Alpaca discovery service binds to a fixed address.
#![allow(clippy::await_holding_lock)]
#![cfg(feature = "mock")]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ppba_driver::config::{ObservingConditionsConfig, SerialConfig, ServerConfig, SwitchConfig};
use ppba_driver::io::{SerialPair, SerialPortFactory};
use ppba_driver::{Config, Result};
use std::time::Duration;

// Mutex to serialize tests - the ASCOM Alpaca discovery service binds to a
// fixed address, so only one server can run at a time.
static SERVER_LOCK: Mutex<()> = Mutex::new(());

/// Minimal serial port factory for server startup tests.
/// The server only opens the port when a client connects, so this is never called
/// during device registration tests.
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

/// Create a test config with port 0 (OS-assigned) and device enablement flags.
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

/// Spawn the server with port 0 and return the actual bound port.
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

/// Helper: GET an ASCOM Alpaca endpoint and return the HTTP status code.
async fn get_status(port: u16, path: &str) -> u16 {
    let url = format!("http://127.0.0.1:{}{}", port, path);
    reqwest::get(&url).await.unwrap().status().as_u16()
}

/// Helper: GET an ASCOM Alpaca endpoint and parse the JSON response body.
async fn get_json(port: u16, path: &str) -> serde_json::Value {
    let url = format!("http://127.0.0.1:{}{}", port, path);
    let text = reqwest::get(&url).await.unwrap().text().await.unwrap();
    serde_json::from_str(&text).unwrap()
}

// ============================================================================
// Server startup tests
// ============================================================================

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_starts_with_both_devices_enabled() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let (port, handle) = spawn_server(test_config(true, true)).await;

    let switch_status = get_status(port, "/api/v1/switch/0/name").await;
    assert_eq!(switch_status, 200, "Switch name endpoint should respond");

    let oc_status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_eq!(
        oc_status, 200,
        "ObservingConditions name endpoint should respond"
    );

    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_starts_with_switch_only() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let (port, handle) = spawn_server(test_config(true, false)).await;

    let switch_status = get_status(port, "/api/v1/switch/0/name").await;
    assert_eq!(switch_status, 200, "Switch name endpoint should respond");

    let oc_status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_ne!(
        oc_status, 200,
        "ObservingConditions should not be registered"
    );

    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_starts_with_observingconditions_only() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let (port, handle) = spawn_server(test_config(false, true)).await;

    let switch_status = get_status(port, "/api/v1/switch/0/name").await;
    assert_ne!(switch_status, 200, "Switch should not be registered");

    let oc_status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_eq!(
        oc_status, 200,
        "ObservingConditions name endpoint should respond"
    );

    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_starts_with_no_devices() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let (port, handle) = spawn_server(test_config(false, false)).await;

    let switch_status = get_status(port, "/api/v1/switch/0/name").await;
    assert_ne!(switch_status, 200, "Switch should not be registered");

    let oc_status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_ne!(
        oc_status, 200,
        "ObservingConditions should not be registered"
    );

    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_returns_configured_switch_name() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let mut config = test_config(true, false);
    config.switch.name = "My Custom Switch".to_string();
    let (port, handle) = spawn_server(config).await;

    let body = get_json(port, "/api/v1/switch/0/name").await;
    assert_eq!(body["Value"], "My Custom Switch");

    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_returns_configured_oc_name() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let mut config = test_config(false, true);
    config.observingconditions.name = "My Weather Station".to_string();
    let (port, handle) = spawn_server(config).await;

    let body = get_json(port, "/api/v1/observingconditions/0/name").await;
    assert_eq!(body["Value"], "My Weather Station");

    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_binds_to_os_assigned_port() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let (port, handle) = spawn_server(test_config(true, false)).await;

    assert_ne!(port, 0, "OS should have assigned a real port");

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(stream.is_ok(), "Server should be reachable on bound port");

    handle.abort();
    let _ = handle.await;
}
