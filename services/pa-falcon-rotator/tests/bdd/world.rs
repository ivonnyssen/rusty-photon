//! World struct for pa-falcon-rotator BDD tests
//!
//! Runs the ASCOM Alpaca server in-process on an ephemeral port so the test
//! holds the same `Arc<MockSerialPortFactory>` the SerialManager uses. That
//! gives every scenario direct access to mock device state (set the reported
//! mechanical position, voltage, motor_reverse, limit_detect flag) and to the
//! wire-level `command_log` (assert which commands were sent and in what
//! order) — both of which the feature files exercise.
//!
//! Devices are driven via Alpaca HTTP through the in-process client, so the
//! real serialisation / dispatch path is still exercised. The harness sets
//! `config.server.auth = None`, so authentication is **not** covered here —
//! cover that separately if it becomes a regression risk.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Rotator, Switch, TypedDevice};
use ascom_alpaca::Client as AlpacaClient;
use cucumber::World;
use pa_falcon_rotator::{Config, MockSerialPortFactory, SerialPortFactory, ServerBuilder};
use tokio::task::JoinHandle;

// Several `*_result` fields below are still wired up by Phase 3e step bodies
// (status switch) — they live here so the struct's shape is stable across the
// remaining sub-phases.
#[allow(dead_code)]
#[derive(Debug, Default, World)]
pub struct FalconRotatorWorld {
    /// Joined when the scenario ends to tear the in-process server down.
    pub server_handle: Option<JoinHandle<()>>,
    /// Bound address of the in-process server (ephemeral port).
    pub server_addr: Option<SocketAddr>,
    /// Mock factory shared with the SerialManager — drives device state and
    /// records the wire-level command log.
    pub mock: Option<Arc<MockSerialPortFactory>>,
    /// Alpaca HTTP client for the Rotator device.
    pub rotator: Option<Arc<dyn Rotator>>,
    /// Alpaca HTTP client for the Status Switch device.
    pub status_switch: Option<Arc<dyn Switch>>,
    /// Config used to build the server (mutated by Given/setup steps).
    pub config: Option<Config>,

    /// Captured property reads.
    pub position_result: Option<f64>,
    pub mechanical_position_result: Option<f64>,
    pub target_position_result: Option<f64>,
    pub is_moving_result: Option<bool>,
    pub reverse_result: Option<bool>,
    pub step_size_result: Option<f64>,
    pub can_reverse_result: Option<bool>,
    pub name_result: Option<String>,
    pub unique_id_result: Option<String>,
    pub interface_version_result: Option<i32>,

    /// Captured switch reads.
    pub switch_value_result: Option<f64>,
    pub switch_bool_result: Option<bool>,
    pub max_switch_result: Option<usize>,
    /// Captured `GetSwitchName` / `GetSwitchDescription` reads.
    pub switch_string_result: Option<String>,

    /// Last failure surfaced over Alpaca, if any.
    pub last_error_code: Option<u16>,
}

impl FalconRotatorWorld {
    pub fn rotator(&self) -> &Arc<dyn Rotator> {
        self.rotator.as_ref().expect("rotator not acquired")
    }

    pub fn status_switch(&self) -> &Arc<dyn Switch> {
        self.status_switch
            .as_ref()
            .expect("status switch not acquired")
    }

    pub fn mock(&self) -> &Arc<MockSerialPortFactory> {
        self.mock.as_ref().expect("mock not initialised")
    }

    /// Build a config, start the in-process Alpaca server with the mock
    /// serial factory, and acquire client proxies for both registered
    /// devices.
    pub async fn start_service(&mut self) {
        let mut config = self.config.clone().unwrap_or_default();
        // Bind on an ephemeral port so concurrent BDD scenarios don't fight
        // over a fixed port number.
        config.server.port = 0;
        // No UDP discovery service — the test resolves the device list via
        // the bound HTTP port directly.
        config.server.discovery_port = None;
        // No TLS / auth in BDD scenarios.
        config.server.tls = None;
        config.server.auth = None;

        let mock = Arc::new(MockSerialPortFactory::default());
        let factory: Arc<dyn SerialPortFactory> = Arc::clone(&mock) as _;

        let bound = ServerBuilder::new()
            .with_config(config)
            .with_factory(factory)
            .build()
            .await
            .expect("build in-process Alpaca server");
        let local_addr = bound.listen_addr();

        let server_handle = tokio::spawn(async move {
            // BoundServer::start returns `Result<(), Box<dyn Error>>`; the
            // error type is `!Send` so we collapse it to `()` here before
            // letting the spawn machinery see the output.
            if bound.start().await.is_err() {
                // tear-down on shutdown is normal; nothing to log.
            }
        });

        let http_addr = SocketAddr::from(([127, 0, 0, 1], local_addr.port()));
        let (rotator, status_switch) = acquire_devices(http_addr).await;

        self.mock = Some(mock);
        self.server_handle = Some(server_handle);
        self.server_addr = Some(http_addr);
        self.rotator = Some(rotator);
        self.status_switch = Some(status_switch);
    }

    /// Abort the in-process server task. Called from cucumber's `after` hook
    /// once per scenario so server resources are released before the next
    /// scenario binds its own ephemeral port.
    pub async fn shutdown(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
            // Drain the join result so the runtime sees the task as finished;
            // we don't care if it returned the JoinError(cancelled).
            let _ = handle.await;
        }
        self.rotator = None;
        self.status_switch = None;
        self.mock = None;
        self.server_addr = None;
    }
}

async fn acquire_devices(addr: SocketAddr) -> (Arc<dyn Rotator>, Arc<dyn Switch>) {
    let client = AlpacaClient::new_from_addr(addr);
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        if let Ok(devices) = client.get_devices().await {
            let mut rotator = None;
            let mut status_switch = None;
            for device in devices {
                #[allow(unreachable_patterns)]
                match device {
                    TypedDevice::Rotator(r) => rotator = Some(r),
                    TypedDevice::Switch(s) => status_switch = Some(s),
                    _ => {}
                }
            }
            if let (Some(r), Some(s)) = (rotator, status_switch) {
                return (r, s);
            }
        }
    }
    panic!("pa-falcon-rotator in-process server did not register both devices in 5 seconds");
}
