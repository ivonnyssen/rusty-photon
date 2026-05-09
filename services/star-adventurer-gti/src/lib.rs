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
pub use transport::Transport;
pub use transport_manager::TransportManager;

#[cfg(feature = "mock")]
pub use transport::mock::{MockMountState, MockTransport};

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
    transport: Option<Arc<dyn Transport>>,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Inject a [`Transport`] implementation. BDD tests pass
    /// [`MockTransport`]; the production path leaves this `None` and lets
    /// `build()` pick serial-or-UDP from `config.transport`.
    pub fn with_transport(mut self, transport: Arc<dyn Transport>) -> Self {
        self.transport = Some(transport);
        self
    }

    pub async fn build(self) -> std::result::Result<BoundServer, Box<dyn std::error::Error>> {
        let mut server = Server::new(CargoServerInfo!());
        server.listen_addr = SocketAddr::from(([0, 0, 0, 0], self.config.server.port));
        server.discovery_port = self.config.server.discovery_port;

        // Phase 3 wires real transports here. Until then the only path
        // that boots a server is one that injects a transport explicitly
        // (see BDD tests).
        let transport = self.transport.ok_or_else(|| {
            Box::<dyn std::error::Error>::from(
                "no transport injected — Phase 3 will pick serial/UDP from config.transport",
            )
        })?;

        let manager = Arc::new(TransportManager::new(self.config.clone(), transport));

        if self.config.mount.enabled {
            let device = MountDevice::new(self.config.mount.clone(), Arc::clone(&manager));
            server.devices.register(device);
            info!("Registered Telescope device: {}", self.config.mount.name);
        }

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
