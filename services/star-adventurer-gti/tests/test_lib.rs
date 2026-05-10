//! Tests for `lib.rs` — server startup and device registration.
//!
//! Verifies that `ServerBuilder` correctly registers the telescope device
//! based on configuration flags and starts the ASCOM Alpaca server.
//!
//! Requires the `mock` feature; all tests are skipped under Miri because it
//! cannot call socket syscalls. Tests run sequentially because the ASCOM
//! Alpaca discovery service binds to a fixed address, so only one server
//! can run at a time.
#![allow(clippy::await_holding_lock)]
#![cfg(feature = "mock")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use star_adventurer_gti::{
    Config, MockTransportFactory, MountConfig, ServerBuilder, ServerConfig, TransportFactory,
};

static SERVER_LOCK: Mutex<()> = Mutex::new(());

fn test_config(mount_enabled: bool) -> Config {
    let mut cfg = Config::default();
    cfg.server = ServerConfig {
        port: 0,
        discovery_port: None,
        tls: None,
        auth: None,
    };
    cfg.mount = MountConfig {
        enabled: mount_enabled,
        ..cfg.mount
    };
    cfg
}

async fn spawn_server(config: Config) -> (u16, tokio::task::JoinHandle<()>) {
    let factory: Arc<dyn TransportFactory> = Arc::new(MockTransportFactory);
    let bound = ServerBuilder::new()
        .with_config(config)
        .with_transport_factory(factory)
        .build()
        .await
        .expect("server failed to bind");

    let port = bound.listen_addr().port();
    let handle = tokio::spawn(async move {
        let _ = bound.start().await;
    });
    (port, handle)
}

async fn get_status(port: u16, path: &str) -> u16 {
    let url = format!("http://127.0.0.1:{port}{path}");
    reqwest::get(&url).await.unwrap().status().as_u16()
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_server_starts_with_mount_enabled() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let (port, handle) = spawn_server(test_config(true)).await;
    // Give the server a moment to be ready to accept HTTP.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let status = get_status(port, "/api/v1/telescope/0/name").await;
    assert_eq!(status, 200, "Telescope name endpoint should respond");
    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_server_starts_with_mount_disabled() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let (port, handle) = spawn_server(test_config(false)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let status = get_status(port, "/api/v1/telescope/0/name").await;
    assert_ne!(status, 200, "Telescope should not be registered");
    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_server_binds_to_os_assigned_port() {
    let _lock = SERVER_LOCK.lock().unwrap();
    let (port, handle) = spawn_server(test_config(true)).await;
    assert_ne!(port, 0, "OS should have assigned a real port");
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
    assert!(stream.is_ok(), "Server should be reachable on bound port");
    handle.abort();
    let _ = handle.await;
}
