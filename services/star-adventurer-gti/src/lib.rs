#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Star Adventurer GTi ASCOM Alpaca driver.
//!
//! See [`docs/services/star-adventurer-gti.md`](../../../docs/services/star-adventurer-gti.md)
//! for the design contract this crate implements.

pub mod config;
pub mod coordinates;
pub mod error;
pub mod mount_device;
pub mod transport;
pub mod transport_manager;

pub use config::{
    load_config, Config, MountConfig, ServerConfig, TrackingRateName, TransportConfig, UdpConfig,
    UsbConfig,
};
pub use error::{Result, StarAdvError};
pub use mount_device::MountDevice;
pub use transport::serial::SerialTransportFactory;
pub use transport::udp::UdpTransportFactory;
pub use transport::{Transport, TransportFactory};
pub use transport_manager::TransportManager;

#[cfg(feature = "mock")]
pub use transport::mock::{MockMountState, MockTransport, MockTransportFactory};

use std::net::SocketAddr;
use std::sync::Arc;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use rp_tls::config::TlsConfig;
use tokio::signal;
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

        // Default to a config-driven factory if none was injected. Phase 3
        // fills in the per-factory `connect()` bodies; until then the
        // server still binds and serves metadata, but `Connected = true`
        // returns NOT_IMPLEMENTED.
        let factory = self
            .factory
            .unwrap_or_else(|| -> Arc<dyn TransportFactory> {
                match self.config.transport {
                    config::TransportConfig::Usb(_) => Arc::new(SerialTransportFactory),
                    config::TransportConfig::Udp(_) => Arc::new(UdpTransportFactory),
                }
            });

        let manager = Arc::new(TransportManager::new(self.config.clone(), factory));

        if self.config.mount.enabled {
            let device = MountDevice::new(self.config.mount.clone(), Arc::clone(&manager));
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

    pub async fn start(self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        match self.tls {
            Some(ref tls_config) => {
                info!("star-adventurer-gti started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(
                    self.listener,
                    self.router,
                    tls_config,
                    shutdown_signal(),
                )
                .await?;
            }
            None => {
                info!("star-adventurer-gti started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown_signal()).await?;
            }
        }
        debug!("star-adventurer-gti shut down");
        Ok(())
    }
}

/// HTTP router for the `/debug/v1/mock-commands` introspection endpoint.
///
/// Returns a JSON object `{ "commands": ["...", "..."] }` where each
/// element is the wire frame as a UTF-8-decoded string (the protocol
/// uses ASCII so the conversion is lossless). Used by BDD tests that
/// need to assert which commands the driver issued without reaching
/// into the mock state from outside the service process.
#[cfg(feature = "mock")]
fn debug_mock_router(state: Arc<tokio::sync::Mutex<MockMountState>>) -> axum::Router {
    use axum::extract::State;
    use axum::routing::get;
    use axum::Json;
    use serde_json::json;

    async fn handler(
        State(state): State<Arc<tokio::sync::Mutex<MockMountState>>>,
    ) -> Json<serde_json::Value> {
        let log = &state.lock().await.command_log;
        let frames: Vec<String> = log
            .iter()
            .map(|frame| String::from_utf8_lossy(frame).into_owned())
            .collect();
        Json(json!({ "commands": frames }))
    }

    axum::Router::new()
        .route("/debug/v1/mock-commands", get(handler))
        .with_state(state)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => debug!("received Ctrl+C"),
        () = terminate => debug!("received SIGTERM"),
    }
}
