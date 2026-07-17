//! World struct for pa-falcon-rotator BDD tests
//!
//! Runs the ASCOM Alpaca server in-process on an ephemeral port so the
//! test holds the same `Arc<MockFalconTransportFactory>` the FalconManager
//! drives. That gives every scenario direct access to mock device state
//! (set the reported mechanical position, voltage, motor_reverse,
//! limit_detect flag) and to the wire-level `command_log` (assert which
//! commands were sent and in what order) — both of which the feature
//! files exercise.
//!
//! Devices are driven via Alpaca HTTP through the in-process client, so
//! the real serialisation / dispatch path is still exercised. The
//! `start_service` harness sets `config.server.auth = None`; the TLS +
//! auth credential gate is covered by the shared smoke scenario in
//! `auth.feature` via the `TlsAuthSmokeWorld` impl below.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Rotator, Switch, TypedDevice};
use ascom_alpaca::Client as AlpacaClient;
use bdd_infra::tls_auth::{TlsAuthSmokeWorld, TlsAuthState};
use cucumber::World;
use pa_falcon_rotator::config::CliOverrides;
use pa_falcon_rotator::{Config, MockFalconTransportFactory, ServerBuilder};
use rusty_photon_service_lifecycle::ReloadSignal;
use rusty_photon_shared_transport::TransportFactory;
use tempfile::TempDir;
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
    /// Mock factory shared with the FalconManager — drives device state
    /// and records the wire-level command log.
    pub mock: Option<Arc<MockFalconTransportFactory>>,
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

    /// Config-action test state: the temp dir + path config.apply persists to,
    /// the reload signal both devices fire, and the last parsed action response.
    pub config_temp_dir: Option<TempDir>,
    pub config_path: Option<PathBuf>,
    pub reload: Option<ReloadSignal>,
    pub last_response: Option<serde_json::Value>,
    pub last_supported_actions: Option<Vec<String>>,

    /// State for the shared TLS + auth smoke steps (`auth.feature`).
    pub tls_auth: TlsAuthState,
}

impl TlsAuthSmokeWorld for FalconRotatorWorld {
    fn tls_auth(&mut self) -> &mut TlsAuthState {
        &mut self.tls_auth
    }

    fn base_test_config(&self) -> serde_json::Value {
        // The library defaults with the mock serial port; the shared
        // configure step replaces `server` with the fixture's block.
        let mut config = serde_json::to_value(Config::default()).unwrap();
        config["serial"]["port"] = serde_json::json!("/dev/mock");
        config
    }

    async fn start_with_tls_auth(&mut self, config: serde_json::Value) {
        // Deserialize through the typed `Config` so the smoke exercises the
        // real config-load path: the shared `server` block (port 0 + `tls` +
        // `auth`) maps onto `AlpacaServerConfig`.
        let config: Config = serde_json::from_value(config).unwrap();

        let mock = Arc::new(MockFalconTransportFactory::default());
        let factory: Arc<dyn TransportFactory> = Arc::clone(&mock) as _;
        let bound = ServerBuilder::new()
            .with_config(config)
            .with_factory(factory)
            .build()
            .await
            .expect("build in-process Alpaca server with TLS + auth");
        let local_addr = bound.listen_addr();

        let server_handle = tokio::spawn(async move {
            let _ = bound.start(std::future::pending::<()>()).await;
        });
        self.mock = Some(mock);
        // Torn down by `World::shutdown` from the cucumber `after` hook.
        self.server_handle = Some(server_handle);
        self.server_addr = Some(SocketAddr::from(([127, 0, 0, 1], local_addr.port())));
        self.tls_auth.port = Some(local_addr.port());
    }
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

    pub fn mock(&self) -> &Arc<MockFalconTransportFactory> {
        self.mock.as_ref().expect("mock not initialised")
    }

    /// Build a config, start the in-process Alpaca server with the mock
    /// serial factory, and acquire client proxies for both registered
    /// devices.
    pub async fn start_service(&mut self) {
        let mut config = self.config.clone().unwrap_or_default();
        // The in-process world bypasses main.rs's first-run identity
        // materialization, so seed stable UniqueIDs here (mirroring what the
        // binary persists on first run) for deterministic metadata assertions.
        if config.rotator.unique_id.is_empty() {
            config.rotator.unique_id = "pa-falcon-rotator-001".to_string();
        }
        if config.switch.unique_id.is_empty() {
            config.switch.unique_id = "pa-falcon-rotator-status-001".to_string();
        }
        // Bind on an ephemeral port so concurrent BDD scenarios don't fight
        // over a fixed port number.
        config.server.port = 0;
        // No UDP discovery service — the test resolves the device list via
        // the bound HTTP port directly.
        config.server.discovery_port = None;
        // No TLS / auth in BDD scenarios.
        config.server.tls = None;
        config.server.auth = None;

        let mock = Arc::new(MockFalconTransportFactory::default());
        let factory: Arc<dyn TransportFactory> = Arc::clone(&mock) as _;

        // Persist the config to a temp file so config.apply has a real target,
        // and wire a reload signal both devices fire — exercising the same
        // config-action context the binary's `main` builds.
        let dir = TempDir::new().expect("failed to create temp dir");
        let config_path = dir.path().join("config.json");
        std::fs::write(
            &config_path,
            serde_json::to_string(&config).expect("serialize config"),
        )
        .expect("write config file");
        let reload = ReloadSignal::new();

        let bound = ServerBuilder::new()
            .with_config(config)
            .with_factory(factory)
            .with_config_source(config_path.clone(), CliOverrides::default())
            .with_reload_signal(reload.clone())
            .build()
            .await
            .expect("build in-process Alpaca server");
        let local_addr = bound.listen_addr();

        self.config_temp_dir = Some(dir);
        self.config_path = Some(config_path);
        self.reload = Some(reload);

        let server_handle = tokio::spawn(async move {
            // BoundServer::start returns `Result<(), Box<dyn Error>>`; the
            // error type is `!Send` so we collapse it to `()` here before
            // letting the spawn machinery see the output. The shutdown
            // future never resolves; the test tears the task down by
            // aborting the JoinHandle in `World::shutdown`.
            if bound.start(std::future::pending::<()>()).await.is_err() {
                // tear-down on shutdown is normal; nothing to log.
            }
        });

        // Park the spawned task on `self` immediately so a panic inside
        // `acquire_devices` (it currently `panic!`s after a 5s timeout) does
        // not drop the `JoinHandle` and detach the in-process server. The
        // cucumber `after` hook calls `World::shutdown`, which aborts the
        // handle — but only if it's visible on the world.
        self.mock = Some(mock);
        self.server_handle = Some(server_handle);
        let http_addr = SocketAddr::from(([127, 0, 0, 1], local_addr.port()));
        self.server_addr = Some(http_addr);

        let (rotator, status_switch) = acquire_devices(http_addr).await;
        self.rotator = Some(rotator);
        self.status_switch = Some(status_switch);
    }

    /// Call `config.get` on the rotator device, stash the parsed response, and
    /// return the `config` object so a When step can edit it and re-apply.
    pub async fn current_config(&mut self) -> serde_json::Value {
        let rotator = Arc::clone(self.rotator());
        let body = rotator
            .action("config.get".to_string(), String::new())
            .await
            .expect("config.get failed");
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("config.get returned invalid JSON");
        let config = parsed
            .get("config")
            .cloned()
            .expect("config.get response missing `config`");
        self.last_response = Some(parsed);
        config
    }

    /// Call `config.apply` on the rotator device with `params`, stashing the
    /// parsed response.
    pub async fn call_config_apply(&mut self, params: serde_json::Value) {
        let rotator = Arc::clone(self.rotator());
        let body = rotator
            .action("config.apply".to_string(), params.to_string())
            .await
            .expect("config.apply failed");
        self.last_response =
            Some(serde_json::from_str(&body).expect("config.apply returned invalid JSON"));
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
