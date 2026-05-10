//! World struct for star-adventurer-gti BDD tests.
//!
//! Drives a real `star-adventurer-gti` binary spawned via
//! `bdd_infra::ServiceHandle`. The binary runs with `--features mock`,
//! which routes its `TransportFactory` through `CapturingMockFactory`
//! and mounts `/debug/v1/mock-commands` so step assertions about wire
//! frames can fetch the mock's command log over HTTP.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::telescope::Telescope;
use ascom_alpaca::api::TypedDevice;
use ascom_alpaca::ASCOMError;
use ascom_alpaca::Client as AlpacaClient;
use bdd_infra::ServiceHandle;
use cucumber::World;
use serde_json::Value;
use star_adventurer_gti::{Config, MountConfig, ServerConfig, TransportConfig, UsbConfig};
use tempfile::TempDir;
use tokio::time::sleep;

#[derive(Debug, Default, World)]
pub struct StarAdventurerWorld {
    pub service_handle: Option<ServiceHandle>,
    pub mount: Option<Arc<dyn Telescope>>,
    pub config: Option<Config>,
    pub temp_dir: Option<TempDir>,
    pub last_error: Option<String>,
    pub last_error_code: Option<u16>,
    /// Pending state-seed body that BDD `Given` steps build up before
    /// the service is started. After `start_service` spawns the binary,
    /// `apply_pending_seed()` POSTs this to `/debug/v1/mock-state` so
    /// the mount has the desired state before the first `When I
    /// connect` step.
    ///
    /// Persists across applies so a scenario that re-spawns the
    /// binary (e.g. a `Given service configured with X` followed by
    /// a `Given a running service`) keeps the seeds for the second
    /// spawn. Each `queue_seed` call overwrites by key, so the map
    /// always carries the latest desired state.
    pub pending_seed: serde_json::Map<String, serde_json::Value>,
}

impl StarAdventurerWorld {
    pub fn mount(&self) -> &Arc<dyn Telescope> {
        self.mount
            .as_ref()
            .expect("mount client not acquired — did the service start?")
    }

    /// Mutable accessor for the in-flight [`Config`]. Step bodies that
    /// tweak knobs (`a star-adventurer service configured with site
    /// latitude N`) call this before [`start_service`].
    pub fn config_mut(&mut self) -> &mut Config {
        self.config.get_or_insert_with(default_test_config)
    }

    /// Build a JSON config from current world state, write it to a temp
    /// file, spawn the service binary via [`ServiceHandle`], poll the
    /// Alpaca client until the Telescope device is exposed, and apply
    /// any deferred state seeds that earlier `Given` steps queued up.
    pub async fn start_service(&mut self) {
        let cfg = self.config.clone().unwrap_or_else(default_test_config);
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("config.json");
        let json = serde_json::to_string_pretty(&cfg).expect("config JSON serialise");
        std::fs::write(&config_path, json).expect("write config");
        let path_str = config_path.to_str().expect("UTF-8 temp path").to_string();

        let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &path_str).await;
        let mount = acquire_mount(&handle).await;
        self.config = Some(cfg);
        self.mount = Some(mount);
        self.service_handle = Some(handle);
        self.apply_pending_seed().await;
    }

    /// POST the queued state seed to `/debug/v1/mock-state`. No-op when
    /// no seed has been queued.
    ///
    /// Does NOT clear `pending_seed` — scenarios sometimes re-spawn the
    /// binary (e.g. a `Given service configured with name X` followed
    /// by another `Given a running service`) and the new binary needs
    /// the same seed. Each `queue_seed` overwrites by key, so the map
    /// always holds the latest desired state; re-applying is
    /// idempotent.
    pub async fn apply_pending_seed(&mut self) {
        if self.pending_seed.is_empty() {
            return;
        }
        let handle = self
            .service_handle
            .as_ref()
            .expect("service not started — cannot apply seed");
        let url = format!("http://127.0.0.1:{}/debug/v1/mock-state", handle.port);
        let body = serde_json::Value::Object(self.pending_seed.clone());
        let resp = reqwest::Client::new()
            .post(&url)
            .json(&body)
            .send()
            .await
            .expect("seed endpoint reachable");
        assert!(
            resp.status().is_success(),
            "seed POST failed: {}",
            resp.status()
        );
    }

    /// Queue a single seed value to be POSTed to `/debug/v1/mock-state`.
    ///
    /// If the service is already running, the seed is applied
    /// immediately — this lets `Given` steps that follow "a running
    /// star-adventurer service" still pre-set mock state before the
    /// next `When I connect` runs. Otherwise the seed accumulates and
    /// gets flushed on the next `start_service`.
    pub async fn queue_seed(&mut self, key: &str, value: serde_json::Value) {
        self.pending_seed.insert(key.to_string(), value);
        if self.service_handle.is_some() {
            self.apply_pending_seed().await;
        }
    }

    /// Fetch the mock-mode wire-command log from the running service's
    /// `/debug/v1/mock-commands` endpoint. Returns each frame as a
    /// `String` (the wire protocol is ASCII, so the conversion is
    /// lossless).
    pub async fn command_log(&self) -> Vec<String> {
        let handle = self
            .service_handle
            .as_ref()
            .expect("service not started — call start_service first");
        let url = format!("http://127.0.0.1:{}/debug/v1/mock-commands", handle.port);
        let body: Value = reqwest::get(&url)
            .await
            .expect("debug endpoint reachable")
            .json()
            .await
            .expect("debug endpoint returns JSON");
        body["commands"]
            .as_array()
            .expect("commands is an array")
            .iter()
            .map(|v| v.as_str().expect("frame is a string").to_string())
            .collect()
    }

    pub fn clear_error(&mut self) {
        self.last_error = None;
        self.last_error_code = None;
    }

    pub fn record_error(&mut self, e: ASCOMError) {
        self.last_error_code = Some(e.code.raw());
        self.last_error = Some(e.message.to_string());
    }
}

/// Reasonable defaults for BDD scenarios: USB transport with a mock
/// path (the `mock` feature replaces the factory anyway), discovery
/// disabled, server bound to port 0 so each test gets an ephemeral
/// port, and a tight `settle_after_slew` so the slew-completion
/// watcher resolves quickly.
fn default_test_config() -> Config {
    Config {
        transport: TransportConfig::Usb(UsbConfig {
            port: "/dev/mock".to_string(),
            baud_rate: 115_200,
            command_timeout: Duration::from_secs(2),
            polling_interval: Duration::from_millis(50),
        }),
        server: ServerConfig {
            port: 0,
            discovery_port: None,
            tls: None,
            auth: None,
        },
        mount: MountConfig {
            settle_after_slew: Duration::from_millis(0),
            ..MountConfig::default()
        },
    }
}

/// Poll the Alpaca client until a Telescope device appears. The service
/// announces itself to mDNS / discovery once the binary's listener is
/// up; we keep retrying until then.
async fn acquire_mount(handle: &ServiceHandle) -> Arc<dyn Telescope> {
    let addr = SocketAddr::from(([127, 0, 0, 1], handle.port));
    let client = AlpacaClient::new_from_addr(addr);
    for _ in 0..60 {
        sleep(Duration::from_millis(500)).await;
        if let Ok(mut devices) = client.get_devices().await {
            if let Some(TypedDevice::Telescope(mount)) = devices.next() {
                return mount;
            }
        }
    }
    panic!("star-adventurer-gti did not become healthy within 30 seconds");
}
