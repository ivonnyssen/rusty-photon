#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Pegasus Falcon Rotator Driver
//!
//! ASCOM Alpaca driver exposing the Pegasus Astro Falcon Rotator
//! (firmware 1.3 or newer) as two devices on one server: an `IRotatorV4`
//! for motion and an `ISwitchV3` for status (raw input voltage +
//! limit-detect flag).
//!
//! See `docs/services/falcon-rotator.md` for the behavioural contract
//! and `docs/plans/shared-transport-extraction.md` for the lifecycle
//! scaffolding the service shares with `qhy-focuser`, `ppba-driver`, and
//! `star-adventurer-gti` via
//! [`rusty_photon_shared_transport::SharedTransport`].

pub mod codec;
pub mod config;
pub mod error;
pub mod manager;
#[cfg(feature = "mock")]
pub mod mock;
pub mod protocol;
pub mod rotator_device;
pub mod serial;
pub mod switch_device;

pub use codec::{FalconCodec, FalconCodecError, FalconResponse};
pub use config::{load_config, Config, RotatorConfig, SerialConfig, ServerConfig, SwitchConfig};
pub use error::{FalconRotatorError, Result};
pub use manager::FalconManager;
pub use rotator_device::FalconRotatorDevice;
pub use serial::FalconTransportFactory;
pub use switch_device::FalconStatusSwitchDevice;

#[cfg(feature = "mock")]
pub use mock::MockFalconTransportFactory;

use std::net::SocketAddr;
use std::sync::Arc;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use rp_tls::config::TlsConfig;
use rusty_photon_shared_transport::TransportFactory;
use tokio::signal;
use tracing::{debug, info};

/// Builder for the ASCOM Alpaca server.
///
/// Wires the Rotator and Status Switch devices through a single
/// [`FalconManager`] so they share one
/// [`rusty_photon_shared_transport::SharedTransport`] and therefore one
/// physical serial connection.
pub struct ServerBuilder {
    config: Config,
    factory: Arc<dyn TransportFactory>,
}

impl Default for ServerBuilder {
    fn default() -> Self {
        let factory = FalconTransportFactory::from_config(&Config::default().serial);
        Self {
            config: Config::default(),
            factory: Arc::new(factory),
        }
    }
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: Config) -> Self {
        // Rebuild the default factory from the new config so the
        // builder's factory always reflects the configured serial port
        // when the caller doesn't supply one explicitly.
        let factory = FalconTransportFactory::from_config(&config.serial);
        self.factory = Arc::new(factory);
        self.config = config;
        self
    }

    pub fn with_factory(mut self, factory: Arc<dyn TransportFactory>) -> Self {
        self.factory = factory;
        self
    }

    pub async fn build(self) -> std::result::Result<BoundServer, Box<dyn std::error::Error>> {
        let mut server = Server::new(CargoServerInfo!());
        server.listen_addr = SocketAddr::from(([0, 0, 0, 0], self.config.server.port));
        server.discovery_port = self.config.server.discovery_port;

        let manager = FalconManager::new(self.factory);

        if self.config.rotator.enabled {
            let rotator_device =
                FalconRotatorDevice::new(self.config.rotator.clone(), Arc::clone(&manager));
            server.devices.register(rotator_device);
            info!("Registered Rotator device: {}", self.config.rotator.name);
        }

        if self.config.switch.enabled {
            let switch_device =
                FalconStatusSwitchDevice::new(self.config.switch.clone(), Arc::clone(&manager));
            server.devices.register(switch_device);
            info!(
                "Registered Status Switch device: {}",
                self.config.switch.name
            );
        }

        info!("Serial port: {}", self.config.serial.port);

        let tls = self.config.server.tls.clone();
        let router = axum::Router::new().fallback_service(server.into_service());

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

/// A fully bound pa-falcon-rotator server ready to accept connections.
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

    /// Coverage off: `start` blocks on the long-running axum loop and the
    /// `serve_tls` / `serve_plain` helpers — neither is reachable from a
    /// `cargo test` shape that completes within the test's lifetime.
    /// `tests/test_lib.rs` covers the bind contract (everything `build`
    /// returns) but stops short of running this loop.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn start(self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        match self.tls {
            Some(ref tls_config) => {
                info!("pa-falcon-rotator started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(
                    self.listener,
                    self.router,
                    tls_config,
                    shutdown_signal(),
                )
                .await?;
            }
            None => {
                info!("pa-falcon-rotator started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown_signal()).await?;
            }
        }
        debug!("pa-falcon-rotator shut down");
        Ok(())
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
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
