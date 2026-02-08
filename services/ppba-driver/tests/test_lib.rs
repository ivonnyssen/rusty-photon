//! Tests for lib.rs - server startup and device registration
//!
//! Tests verify that `start_server_with_factory` correctly registers devices
//! based on configuration flags and starts the ASCOM Alpaca server.
//!
//! All tests are skipped under Miri since it cannot call socket syscalls.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ppba_driver::config::{ObservingConditionsConfig, SerialConfig, ServerConfig, SwitchConfig};
use ppba_driver::io::{SerialPair, SerialPortFactory};
use ppba_driver::{Config, Result};

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

/// Create a test config with the given port and device enablement flags.
fn test_config(port: u16, switch_enabled: bool, oc_enabled: bool) -> Config {
    Config {
        serial: SerialConfig {
            port: "/dev/mock".to_string(),
            polling_interval_ms: 60000,
            ..Default::default()
        },
        server: ServerConfig { port },
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

/// Spawn the server on an OS-assigned port, retrying on TOCTOU port conflicts.
///
/// Returns the actual bound port and a JoinHandle that can be aborted.
async fn spawn_server(
    switch_enabled: bool,
    oc_enabled: bool,
) -> (u16, tokio::task::JoinHandle<()>) {
    spawn_server_with_config(|port| test_config(port, switch_enabled, oc_enabled)).await
}

/// Spawn the server with a custom config builder, retrying on port conflicts.
async fn spawn_server_with_config(
    config_fn: impl Fn(u16) -> Config,
) -> (u16, tokio::task::JoinHandle<()>) {
    for _ in 0..5 {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = config_fn(port);
        let factory: Arc<dyn SerialPortFactory> = Arc::new(StubSerialPortFactory);

        let handle = tokio::spawn(async move {
            let _ = ppba_driver::start_server_with_factory(config, factory).await;
        });

        // Poll until the server accepts connections or the task exits (bind failure)
        for _ in 0..30 {
            if handle.is_finished() {
                break;
            }
            if tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
                .await
                .is_ok()
            {
                return (port, handle);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        handle.abort();
    }
    panic!("Server did not start after 5 attempts");
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
    let (port, handle) = spawn_server(true, true).await;

    let switch_status = get_status(port, "/api/v1/switch/0/name").await;
    assert_eq!(switch_status, 200, "Switch name endpoint should respond");

    let oc_status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_eq!(
        oc_status, 200,
        "ObservingConditions name endpoint should respond"
    );

    handle.abort();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_starts_with_switch_only() {
    let (port, handle) = spawn_server(true, false).await;

    let switch_status = get_status(port, "/api/v1/switch/0/name").await;
    assert_eq!(switch_status, 200, "Switch name endpoint should respond");

    // OC device not registered - should return non-200
    let oc_status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_ne!(
        oc_status, 200,
        "ObservingConditions should not be registered"
    );

    handle.abort();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_starts_with_observingconditions_only() {
    let (port, handle) = spawn_server(false, true).await;

    // Switch device not registered - should return non-200
    let switch_status = get_status(port, "/api/v1/switch/0/name").await;
    assert_ne!(switch_status, 200, "Switch should not be registered");

    let oc_status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_eq!(
        oc_status, 200,
        "ObservingConditions name endpoint should respond"
    );

    handle.abort();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_starts_with_no_devices() {
    let (port, handle) = spawn_server(false, false).await;

    // Neither device registered
    let switch_status = get_status(port, "/api/v1/switch/0/name").await;
    assert_ne!(switch_status, 200, "Switch should not be registered");

    let oc_status = get_status(port, "/api/v1/observingconditions/0/name").await;
    assert_ne!(
        oc_status, 200,
        "ObservingConditions should not be registered"
    );

    handle.abort();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_returns_configured_switch_name() {
    let (port, handle) = spawn_server_with_config(|port| {
        let mut config = test_config(port, true, false);
        config.switch.name = "My Custom Switch".to_string();
        config
    })
    .await;

    let body = get_json(port, "/api/v1/switch/0/name").await;
    assert_eq!(body["Value"], "My Custom Switch");

    handle.abort();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_returns_configured_oc_name() {
    let (port, handle) = spawn_server_with_config(|port| {
        let mut config = test_config(port, false, true);
        config.observingconditions.name = "My Weather Station".to_string();
        config
    })
    .await;

    let body = get_json(port, "/api/v1/observingconditions/0/name").await;
    assert_eq!(body["Value"], "My Weather Station");

    handle.abort();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call socket syscalls
async fn test_server_binds_to_configured_port() {
    let (port, handle) = spawn_server(true, false).await;

    // Server should be listening on the configured port
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(
        stream.is_ok(),
        "Server should be reachable on configured port"
    );

    handle.abort();
}
