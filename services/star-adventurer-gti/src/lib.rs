#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Star Adventurer GTi ASCOM Alpaca driver.
//!
//! See [`docs/services/star-adventurer-gti.md`](../../../docs/services/star-adventurer-gti.md)
//! for the design contract this crate implements.

pub mod codec;
pub mod config;
pub mod coordinates;
pub mod error;
pub mod manager;
pub mod mount_device;
pub mod transport;

pub use config::{
    load_config, Config, FlipPolicy, HomePose, MountConfig, ServerConfig, TrackingRateName,
    TransportConfig, UdpConfig, UsbConfig, MAX_FLIP_RANGE_HOURS,
};
pub use error::{Result, StarAdvError};
pub use manager::{MountManager, MountParameters, MountSnapshot, PollPauseGuard};
pub use mount_device::{
    canonicalise_config_path, probe_park_file_writability, warn_if_park_path_unwritable,
    MountDevice,
};
pub use rusty_photon_shared_transport::TransportFactory;
pub use transport::serial::SerialTransportFactory;
pub use transport::udp::UdpTransportFactory;

#[cfg(feature = "mock")]
pub use transport::mock::{MockMountState, MockTransportFactory};

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use rp_tls::config::TlsConfig;
use tracing::{debug, info};

/// Builder for the Alpaca server bound to a configured Transport.
///
/// Two-phase: `build()` opens the listener and constructs the device tree
/// (so `bound_addr` can be read), then `start()` actually accepts requests.
/// Same pattern as `qhy-focuser::ServerBuilder`.
#[derive(Default)]
pub struct ServerBuilder {
    config: Config,
    factory: Option<Arc<dyn TransportFactory>>,
    /// Absolute path of the JSON config file the driver was started with,
    /// if any. Passed to [`MountDevice`] so `SetPark` knows where to
    /// persist the park position. `None` when the driver runs on
    /// [`Config::default()`] — in which case `CanSetPark` returns `false`
    /// and `SetPark` returns `NOT_IMPLEMENTED`. See the design doc's
    /// [§"Park persistence"](../../../docs/services/star-adventurer-gti.md#park-persistence).
    config_file_path: Option<PathBuf>,
    /// Optional handle to a [`MockMountState`] that the build path mounts
    /// at `/debug/v1/mock-commands`. Set by mock-mode code paths
    /// (`main.rs` under `feature = "mock"`, BDD tests) so the test
    /// process can read the wire-command log out of the running service.
    /// Always `None` in production builds.
    #[cfg(feature = "mock")]
    debug_mock_state: Option<Arc<tokio::sync::Mutex<MockMountState>>>,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Set the path of the JSON config file the driver was started with.
    ///
    /// When provided, the driver advertises `CanSetPark = true` and
    /// `SetPark` writes the captured encoder pair back into the file via
    /// atomic rename. When omitted (e.g. `main.rs` ran without
    /// `--config`), `CanSetPark` is `false`.
    pub fn with_config_file_path(mut self, path: Option<PathBuf>) -> Self {
        self.config_file_path = path;
        self
    }

    /// Inject a [`TransportFactory`]. BDD tests pass
    /// [`MockTransportFactory`](transport::mock::MockTransportFactory);
    /// when omitted, [`build`] picks serial / UDP from `config.transport`.
    pub fn with_transport_factory(mut self, factory: Arc<dyn TransportFactory>) -> Self {
        self.factory = Some(factory);
        self
    }

    /// Mount the `/debug/v1/mock-commands` introspection endpoint backed
    /// by the supplied [`MockMountState`]. Only available under
    /// `feature = "mock"`; production builds cannot expose this even
    /// accidentally.
    ///
    /// Used by:
    /// * BDD tests that need to assert "the mount should have received
    ///   command :K1" from outside the service process.
    /// * `tests/test_lib.rs` for the same.
    /// * `main.rs` when run with `--features mock` so a developer can
    ///   curl the endpoint to inspect the running mock.
    #[cfg(feature = "mock")]
    pub fn with_debug_mock_state(mut self, state: Arc<tokio::sync::Mutex<MockMountState>>) -> Self {
        self.debug_mock_state = Some(state);
        self
    }

    pub async fn build(self) -> std::result::Result<BoundServer, Box<dyn std::error::Error>> {
        let mut server = Server::new(CargoServerInfo!());
        server.listen_addr = SocketAddr::from(([0, 0, 0, 0], self.config.server.port));
        server.discovery_port = self.config.server.discovery_port;

        // Default to a config-driven factory if none was injected.
        // BDD tests inject `MockTransportFactory`; production picks
        // serial vs UDP from the transport block.
        let factory: Arc<dyn TransportFactory> = match self.factory {
            Some(f) => f,
            None => match &self.config.transport {
                config::TransportConfig::Usb(usb) => {
                    Arc::new(SerialTransportFactory::new(usb.clone()))
                }
                config::TransportConfig::Udp(udp) => {
                    Arc::new(UdpTransportFactory::new(udp.clone()))
                }
            },
        };

        let manager = MountManager::new(self.config.clone(), factory);

        if self.config.mount.enabled {
            let device = MountDevice::with_config_file_path(
                self.config.mount.clone(),
                Arc::clone(&manager),
                self.config_file_path.clone(),
            );
            server.devices.register(device);
            info!("Registered Telescope device: {}", self.config.mount.name);
        }

        let tls = self.config.server.tls.clone();
        // Mount the mock-introspection endpoint first so it takes
        // priority over the Alpaca fallback service.
        let router: axum::Router = {
            let r = axum::Router::new();
            #[cfg(feature = "mock")]
            let r = if let Some(state) = self.debug_mock_state {
                r.merge(debug_mock_router(state))
            } else {
                r
            };
            r
        };
        let router = router.fallback_service(server.into_service());
        let router = match &self.config.server.auth {
            Some(auth) => {
                if self.config.server.tls.is_none() {
                    tracing::warn!(
                        "Authentication is enabled but TLS is not. \
                         Credentials will be transmitted in cleartext. \
                         Consider enabling TLS (see `rp init-tls`)."
                    );
                }
                rp_auth::layer(router, auth)
            }
            None => router,
        };

        let listener = rp_tls::server::bind_dual_stack_tokio(SocketAddr::from((
            [0, 0, 0, 0],
            self.config.server.port,
        )))
        .await?;
        let local_addr = listener.local_addr()?;

        println!("Bound Alpaca server bound_addr={}", local_addr);
        info!("Bound Alpaca server bound_addr={}", local_addr);

        Ok(BoundServer {
            listener,
            router,
            local_addr,
            tls,
        })
    }
}

pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
    tls: Option<TlsConfig>,
}

impl BoundServer {
    pub fn listen_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn start(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        match self.tls {
            Some(ref tls_config) => {
                info!("star-adventurer-gti started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(self.listener, self.router, tls_config, shutdown).await?;
            }
            None => {
                info!("star-adventurer-gti started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown).await?;
            }
        }
        debug!("star-adventurer-gti shut down");
        Ok(())
    }
}

/// HTTP router for the mock-introspection / mock-seeding endpoints.
///
/// `GET /debug/v1/mock-commands` — returns `{"commands": ["...", ...]}`,
/// one wire-frame string per element. Used by BDD step bodies that
/// assert on which commands the driver issued without reaching into
/// the mock state from outside the service process.
///
/// `POST /debug/v1/mock-state` — seeds the mock's per-axis state for
/// scenarios that need a non-default encoder position or motion flag
/// before the driver connects. Body is a JSON object whose keys match
/// the ones the BDD steps care about (any subset is allowed):
/// ```json
/// { "ra_ticks": 100000, "dec_ticks": -50000,
///   "ra_running": true, "ra_goto": true,
///   "dec_running": false }
/// ```
/// Returns `{"ok": true}` on success. Only available under
/// `feature = "mock"`.
#[cfg(feature = "mock")]
fn debug_mock_router(state: Arc<tokio::sync::Mutex<MockMountState>>) -> axum::Router {
    use axum::extract::State;
    use axum::routing::{get, post};
    use axum::Json;
    use serde_json::json;

    async fn commands_handler(
        State(state): State<Arc<tokio::sync::Mutex<MockMountState>>>,
    ) -> Json<serde_json::Value> {
        let log = &state.lock().await.command_log;
        let frames: Vec<String> = log
            .iter()
            .map(|frame| String::from_utf8_lossy(frame).into_owned())
            .collect();
        Json(json!({ "commands": frames }))
    }

    use axum::http::StatusCode;

    /// Convert a JSON value into `i32`, range-checking against the
    /// signed-24-bit encoder range the wire protocol can carry. Out-of-range
    /// seeds would later panic the mock on `encode_position(..)` during
    /// `:j` handling, so reject them here with a `400` instead. Returns
    /// `None` for non-integer or out-of-range input.
    fn parse_position_ticks(v: &serde_json::Value) -> Option<i32> {
        use skywatcher_motor_protocol::codec::{POSITION_MAX, POSITION_MIN};
        let n = v.as_i64()?;
        if (POSITION_MIN as i64..=POSITION_MAX as i64).contains(&n) {
            Some(n as i32)
        } else {
            None
        }
    }

    async fn seed_handler(
        State(state): State<Arc<tokio::sync::Mutex<MockMountState>>>,
        Json(body): Json<serde_json::Value>,
    ) -> std::result::Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
        let obj = body.as_object().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "body must be a JSON object"})),
            )
        })?;
        // Validate every present field before mutating any state so a
        // bad seed is rejected atomically.
        let mut ra_ticks: Option<i32> = None;
        let mut dec_ticks: Option<i32> = None;
        let mut ra_goto_target: Option<i32> = None;
        let mut dec_goto_target: Option<i32> = None;
        for (key, target) in [
            ("ra_ticks", &mut ra_ticks),
            ("dec_ticks", &mut dec_ticks),
            ("ra_goto_target_ticks", &mut ra_goto_target),
            ("dec_goto_target_ticks", &mut dec_goto_target),
        ] {
            if let Some(v) = obj.get(key) {
                let parsed = parse_position_ticks(v).ok_or_else(|| {
                    use skywatcher_motor_protocol::codec::{POSITION_MAX, POSITION_MIN};
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": format!(
                                "{key} must be an integer in [{POSITION_MIN}, {POSITION_MAX}] (signed 24-bit encoder range), got {v}"
                            )
                        })),
                    )
                })?;
                *target = Some(parsed);
            }
        }
        let bool_fields = [
            "ra_running",
            "ra_goto",
            "ra_initialized",
            "dec_running",
            "dec_goto",
            "dec_initialized",
        ];
        for key in bool_fields {
            if let Some(v) = obj.get(key) {
                if !v.is_boolean() {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": format!("{key} must be a boolean, got {v}")})),
                    ));
                }
            }
        }

        let mut s = state.lock().await;
        if let Some(v) = ra_ticks {
            s.ra.position_ticks = v;
        }
        if let Some(v) = dec_ticks {
            s.dec.position_ticks = v;
        }
        if let Some(v) = ra_goto_target {
            s.ra.goto_target_ticks = v;
        }
        if let Some(v) = dec_goto_target {
            s.dec.goto_target_ticks = v;
        }
        if let Some(v) = obj.get("ra_running").and_then(|v| v.as_bool()) {
            s.ra.running = v;
        }
        if let Some(v) = obj.get("ra_goto").and_then(|v| v.as_bool()) {
            s.ra.goto = v;
        }
        if let Some(v) = obj.get("ra_initialized").and_then(|v| v.as_bool()) {
            s.ra.initialized = v;
        }
        if let Some(v) = obj.get("dec_running").and_then(|v| v.as_bool()) {
            s.dec.running = v;
        }
        if let Some(v) = obj.get("dec_goto").and_then(|v| v.as_bool()) {
            s.dec.goto = v;
        }
        if let Some(v) = obj.get("dec_initialized").and_then(|v| v.as_bool()) {
            s.dec.initialized = v;
        }
        Ok(Json(json!({"ok": true})))
    }

    axum::Router::new()
        .route("/debug/v1/mock-commands", get(commands_handler))
        .route("/debug/v1/mock-state", post(seed_handler))
        .with_state(state)
}

#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::SocketAddr;

    /// Spawn `debug_mock_router` on an ephemeral port and return
    /// `(addr, state)` so the test can drive HTTP and inspect /
    /// pre-seed the same state the handler is using.
    async fn spawn_debug_router() -> (SocketAddr, Arc<tokio::sync::Mutex<MockMountState>>) {
        let state = Arc::new(tokio::sync::Mutex::new(MockMountState::default()));
        let router = debug_mock_router(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (addr, state)
    }

    #[tokio::test]
    async fn commands_handler_returns_logged_wire_frames() {
        let (addr, state) = spawn_debug_router().await;
        // Pre-seed the command log so the handler has something to return.
        state.lock().await.command_log.push(b":F1\r".to_vec());
        state.lock().await.command_log.push(b":a1\r".to_vec());
        let resp: serde_json::Value = reqwest::get(format!("http://{addr}/debug/v1/mock-commands"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let cmds = resp["commands"].as_array().unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].as_str().unwrap(), ":F1\r");
        assert_eq!(cmds[1].as_str().unwrap(), ":a1\r");
    }

    #[tokio::test]
    async fn seed_handler_accepts_valid_body_and_mutates_state() {
        let (addr, state) = spawn_debug_router().await;
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/debug/v1/mock-state"))
            .json(&json!({
                "ra_ticks": 12345,
                "dec_ticks": -67890,
                "ra_goto_target_ticks": 100000,
                "dec_goto_target_ticks": -50000,
                "ra_running": true,
                "ra_goto": true,
                "ra_initialized": true,
                "dec_running": false,
                "dec_goto": false,
                "dec_initialized": true,
            }))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["ok"].as_bool(), Some(true));
        let s = state.lock().await;
        assert_eq!(s.ra.position_ticks, 12345);
        assert_eq!(s.dec.position_ticks, -67890);
        assert_eq!(s.ra.goto_target_ticks, 100000);
        assert_eq!(s.dec.goto_target_ticks, -50000);
        assert!(s.ra.running);
        assert!(s.ra.goto);
        assert!(s.ra.initialized);
        assert!(!s.dec.running);
        assert!(!s.dec.goto);
        assert!(s.dec.initialized);
    }

    #[tokio::test]
    async fn seed_handler_partial_body_only_touches_named_fields() {
        let (addr, state) = spawn_debug_router().await;
        // Pre-set a baseline so we can detect untouched fields.
        {
            let mut s = state.lock().await;
            s.ra.position_ticks = 999;
            s.dec.position_ticks = 888;
        }
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/debug/v1/mock-state"))
            .json(&json!({"ra_ticks": 5}))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let s = state.lock().await;
        assert_eq!(s.ra.position_ticks, 5);
        assert_eq!(s.dec.position_ticks, 888);
    }

    #[tokio::test]
    async fn seed_handler_rejects_non_object_body() {
        let (addr, _state) = spawn_debug_router().await;
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/debug/v1/mock-state"))
            .json(&json!([1, 2, 3]))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 400);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("JSON object"));
    }

    #[tokio::test]
    async fn seed_handler_rejects_out_of_range_ticks() {
        let (addr, state) = spawn_debug_router().await;
        let baseline = state.lock().await.ra.position_ticks;
        // i64::MAX is well outside the signed-24-bit encoder range.
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/debug/v1/mock-state"))
            .json(&json!({"ra_ticks": i64::MAX}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 400);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("encoder range"));
        // State must not have mutated on a 400.
        assert_eq!(state.lock().await.ra.position_ticks, baseline);
    }

    #[tokio::test]
    async fn seed_handler_rejects_just_above_position_max() {
        use skywatcher_motor_protocol::codec::POSITION_MAX;
        let (addr, _state) = spawn_debug_router().await;
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/debug/v1/mock-state"))
            .json(&json!({"ra_ticks": POSITION_MAX as i64 + 1}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 400);
    }

    #[tokio::test]
    async fn seed_handler_accepts_position_max_boundary() {
        use skywatcher_motor_protocol::codec::{POSITION_MAX, POSITION_MIN};
        let (addr, state) = spawn_debug_router().await;
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/debug/v1/mock-state"))
            .json(&json!({"ra_ticks": POSITION_MAX, "dec_ticks": POSITION_MIN}))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let s = state.lock().await;
        assert_eq!(s.ra.position_ticks, POSITION_MAX);
        assert_eq!(s.dec.position_ticks, POSITION_MIN);
    }

    #[tokio::test]
    async fn seed_handler_rejects_non_boolean_flag() {
        let (addr, state) = spawn_debug_router().await;
        let baseline = state.lock().await.ra.running;
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/debug/v1/mock-state"))
            .json(&json!({"ra_running": "yes"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 400);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("boolean"));
        assert_eq!(state.lock().await.ra.running, baseline);
    }

    #[tokio::test]
    async fn seed_handler_rejects_non_integer_ticks() {
        let (addr, _state) = spawn_debug_router().await;
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/debug/v1/mock-state"))
            .json(&json!({"ra_ticks": "not a number"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 400);
    }

    #[tokio::test]
    async fn server_builder_build_binds_and_returns_addr() {
        // Smoke-tests the build() path that wires the debug router into
        // the fallback service. Just confirms the listener is bound and
        // we can fetch the bound address back.
        let mut config = config::Config::default();
        config.server.port = 0;
        config.server.discovery_port = None;
        config.mount.enabled = false;
        let bound = ServerBuilder::new()
            .with_config(config)
            .with_transport_factory(Arc::new(transport::mock::MockTransportFactory))
            .build()
            .await
            .unwrap();
        let addr = bound.listen_addr();
        assert_ne!(addr.port(), 0, "OS-assigned port should be non-zero");
    }
}
